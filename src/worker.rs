//! Design B's worker (docs/milestone-2-plan.md D6, D10; MUST-M14): one per home, started by hooks,
//! it runs this milestone's consumers over `raw.db` in seq order and exits when idle.

use crate::curate::Phase;
use crate::knowledge::checkpoint;
use crate::raw::Raw;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant};

/// One derived view of raw.db. `step` processes what `device` holds after `after` and returns its
/// new checkpoint; `rewind` deletes its output above `to`. Both run inside the knowledge.db
/// transaction that also moves the checkpoint (D10). A consumer of raw's records reads this
/// device's (other devices' come with sync, milestone 6); a consumer of the op log (milestone 3
/// D4) reads every device that has ops, and counts in op_seqs.
pub trait Consumer {
    fn name(&self) -> &'static str;
    /// The devices `step` is called for.
    fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
        Ok(vec![raw.device().to_owned()])
    }
    /// The highest checkpoint `device` allows: a checkpoint above it lost its commits (MUST-M14).
    fn top(&self, raw: &Raw, device: &str) -> Result<i64> {
        raw.max_seq_of(device)
    }
    /// Where its checkpoints are kept: `checkpoint::SEQS`, or `checkpoint::OPS` for op_seqs.
    fn checkpoints(&self) -> &'static str {
        checkpoint::SEQS
    }
    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64>;
    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()>;
}

/// This milestone's consumers, in order.
pub fn consumers(home: &Path) -> Vec<Box<dyn Consumer>> {
    // Compression last: it waits for every consumer before it.
    vec![
        // The rescan first: the tombstones it appends are in raw before the others read a record.
        Box::new(crate::consumer::rescan::Rescan::new(home)),
        Box::new(crate::consumer::fts::Fts),
        // The op log's claims (milestone 3 D4), in op_seqs of every device that has ops.
        Box::new(crate::consumer::claims::Claims),
        // Claims whose quote a later tombstone masked or removed.
        Box::new(crate::consumer::claims::Anchors),
        // The op log's digests (Task 9), shown while every claim they cite is current.
        Box::new(crate::consumer::digest::Digests),
        Box::new(crate::consumer::manifest::Manifest::new(home)),
        Box::new(crate::consumer::gaps::Gaps::new(home)),
        Box::new(crate::consumer::compress::Compress),
    ]
}

/// Runs each consumer from its checkpoint until none moves; each step and its checkpoint move
/// share one knowledge.db transaction. A step may move its checkpoint back (the rescan starting
/// again under new rules).
#[cfg(test)] // the worker checks the backup deadline between passes
pub fn drain(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<()> {
    while pass(raw, k, consumers)? {}
    Ok(())
}

/// One batch for each consumer: whether any checkpoint moved.
fn pass(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<bool> {
    let mut advanced = false;
    for c in consumers.iter_mut() {
        for device in c.devices(raw)? {
            // Immediate: a step reads before it writes, and a deferred transaction whose snapshot
            // another writer moved meanwhile (a search creating its table in a fresh knowledge.db)
            // fails its first write with SQLITE_BUSY at once, which stopped the worker.
            let tx = k.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let at = checkpoint::get_in(&tx, c.checkpoints(), c.name(), &device)?;
            let next = c.step(raw, &tx, &device, at)?;
            if next != at {
                checkpoint::set_in(&tx, c.checkpoints(), c.name(), &device, next)?;
                advanced = true;
            }
            tx.commit()?;
        }
    }
    Ok(advanced)
}

/// The per-home worker lock, `<home>/state/worker.lock`; released when dropped. Each taking of it
/// has the next number of `state/worker-gen`, so a run's outcome is ordered against a later run's.
pub struct Lock(#[allow(dead_code)] std::fs::File, u64);

/// The lock, or `None` when another process holds it. Hooks try it too, and start a worker only
/// when they get it (dropping it at once).
pub fn lock(home: &Path) -> Result<Option<Lock>> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("worker.lock"))?;
    match f.try_lock() {
        Ok(()) => {
            // Replaced whole, never rewritten in place: a number cut off by a crash or a full disk
            // would start the count again below the last recorded outcome's.
            // One that is unreadable anyway goes on from the last recorded outcome's.
            let gen_file = state.join("worker-gen");
            let taken = std::fs::read_to_string(&gen_file)
                .ok()
                .and_then(|g| g.trim().parse::<u64>().ok())
                .or_else(|| outcome(home).map(|(g, _)| g))
                .unwrap_or(0)
                + 1;
            let next = state.join("worker-gen.next");
            std::fs::write(&next, taken.to_string())?;
            std::fs::rename(&next, &gen_file)?;
            Ok(Some(Lock(f, taken)))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// How often a worker looks for new records while it waits.
const POLL: Duration = Duration::from_millis(200);

/// Whether any consumer's checkpoint for this device is below raw's highest seq.
fn behind(raw: &Raw, k: &Connection, consumers: &[Box<dyn Consumer>]) -> Result<bool> {
    for c in consumers {
        for device in c.devices(raw)? {
            if checkpoint::get_in(k, c.checkpoints(), c.name(), &device)? < c.top(raw, &device)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Take the lock (or return at once), check both stores, rewind, drain, then wait for new records
/// until `idle_ms` passes with none. At idle the lock is released first and raw is checked once
/// more (D6): a hook that appended before the release saw the lock held and started nothing, so
/// this last check is what finds its record; one that appends after the release gets the lock and
/// starts a worker itself. `before_exit` runs between the release and that check (a test seam).
#[cfg(test)] // `run` takes the lock itself, to know whether this run did the work
pub fn run_with(
    home: &Path,
    idle_ms: u64,
    consumers: Vec<Box<dyn Consumer>>,
    before_exit: impl FnMut(),
) -> Result<()> {
    run_holding(home, idle_ms, consumers, before_exit, None, None)
}

/// The curation phase a worker runs after its consumers have drained (milestone 3 D3).
pub type CurationPhase<'a> = dyn FnMut(&mut Raw, &Connection) -> Result<Phase> + 'a;

fn run_holding(
    home: &Path,
    idle_ms: u64,
    mut consumers: Vec<Box<dyn Consumer>>,
    mut before_exit: impl FnMut(),
    taken: Option<Lock>,
    mut phase: Option<&mut CurationPhase>,
) -> Result<()> {
    let mut holding = Holding::default();
    if let Some(l) = taken {
        take(home, l, &mut holding);
    }
    let result = serve_until_done(
        home,
        idle_ms,
        &mut consumers,
        &mut before_exit,
        &mut holding,
        &mut phase,
    );
    // Released first: a hook that finds the lock free starts a worker for what it appended.
    holding.lock = None;
    // A run that never took the lock did no work: another worker's outcome stands.
    if holding.last > 0 {
        record(home, holding.last, &result);
    }
    result
}

/// Every lock this run takes is noted as a run that has not ended, until `record` replaces the
/// note: a worker killed or crashed while it holds any of them is reported (doctor).
fn take(home: &Path, l: Lock, holding: &mut Holding) {
    holding.last = l.1;
    note(home, l.1, STOPPED);
    holding.lock = Some(l);
}

/// The worker lock while this run holds it, and the number of its last taking.
#[derive(Default)]
struct Holding {
    lock: Option<Lock>,
    last: u64,
}

fn serve_until_done(
    home: &Path,
    idle_ms: u64,
    consumers: &mut [Box<dyn Consumer>],
    before_exit: &mut impl FnMut(),
    holding: &mut Holding,
    phase: &mut Option<&mut CurationPhase>,
) -> Result<()> {
    // ponytail: D11's 30-minute deadline in memory; every idle exit backs up too, so a lost
    // deadline only brings the next backup forward. It is checked between batches and while
    // idle, so neither a long backlog nor a long idle wait puts it off.
    let mut next_backup = Instant::now() + crate::backup::EVERY;
    // Task 8: the stores are closed and opened again, which restores a damaged raw.db, when a
    // hook asks (it found raw.db damaged while this worker held the lock) or when one of this
    // worker's own reads finds it damaged. Twice at most for the second: a damage that opening
    // does not see would otherwise loop.
    let mut damaged = 0;
    loop {
        match serve(
            home,
            idle_ms,
            consumers,
            holding,
            &mut next_backup,
            before_exit,
            phase,
        ) {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(e) if crate::backup::corrupt(&e) && damaged < 2 => {
                damaged += 1;
                eprintln!("oboete: {e:#}; opening the stores again");
            }
            Err(e) => return Err(e),
        }
    }
}

/// One run of the worker over the stores it opens: `Ok(true)` when a restore was asked for and
/// they must be opened again, `Ok(false)` when it is done.
fn serve(
    home: &Path,
    idle_ms: u64,
    consumers: &mut [Box<dyn Consumer>],
    holding: &mut Holding,
    next_backup: &mut Instant,
    before_exit: &mut impl FnMut(),
    phase: &mut Option<&mut CurationPhase>,
) -> Result<bool> {
    if holding.lock.is_none() {
        match lock(home)? {
            Some(l) => take(home, l, holding),
            None => return Ok(false),
        }
    }
    // Taken before the open, so a request a hook makes while it runs is still seen below; put
    // back when the open fails (no segment, a reader holding raw.lock), for the next worker,
    // which the next hook starts.
    let asked = crate::backup::take_restore_request(home);
    // Task 8: a damaged raw.db is restored from the backups, a damaged knowledge.db rebuilt.
    let mut raw = crate::backup::open_raw(home).inspect_err(|_| {
        if asked {
            crate::backup::request_restore(home);
        }
    })?;
    let mut k = crate::backup::open_knowledge(home)?;
    checkpoint::rewind(&raw, &k, consumers)?;
    crate::backup::check(home, &raw);
    let mut due = |raw: &Raw| {
        if Instant::now() >= *next_backup {
            crate::backup::run(home, raw);
            *next_backup = Instant::now() + crate::backup::EVERY;
        }
    };
    loop {
        while pass(&raw, &mut k, consumers)? {
            due(&raw);
        }
        due(&raw);
        // D3: one window once the consumers have drained. A window that waits only on time, and
        // within D10's 30 minutes, keeps the worker up until then.
        let mut stay = None;
        if let Some(phase) = phase.as_mut() {
            match phase(&mut raw, &k)? {
                Phase::Covered if crate::backup::restore_requested(home) => return Ok(true),
                Phase::Covered => continue,
                Phase::Waiting { until, up: true } => stay = Some(until),
                Phase::Waiting { .. } | Phase::Idle => {}
            }
        }
        // New records, or new ops of this device (an owner's correction appends only an op).
        let seen = (raw.max_seq()?, raw.max_op_seq_of(raw.device())?);
        // A window's time replaces the idle wait: the phase runs again then, and the idle wait
        // starts once it has nothing left to wait for.
        let wait = stay.map_or(idle_ms, |until| {
            u64::try_from(until - crate::db::now_ms()).unwrap_or(0)
        });
        let deadline = Instant::now() + Duration::from_millis(wait);
        let mut more = false;
        while Instant::now() < deadline {
            std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            if crate::backup::restore_requested(home) {
                return Ok(true);
            }
            if (raw.max_seq()?, raw.max_op_seq_of(raw.device())?) != seen {
                more = true;
                break;
            }
            due(&raw);
        }
        if more || stay.is_some() {
            continue;
        }
        // Under the lock: a worker started after the release cannot export the same seqs.
        crate::backup::run(home, &raw);
        crate::hookstate::prune(home, crate::hookstate::KEEP);
        holding.lock = None;
        before_exit();
        // A hook that asked before the release saw the lock held and started nothing.
        let wanted = crate::backup::restore_requested(home);
        if !wanted && !behind(&raw, &k, consumers)? {
            return Ok(false);
        }
        match lock(home)? {
            Some(l) => take(home, l, holding),
            // Another worker took the lock after the release: the records are its now.
            None => return Ok(false),
        }
        if wanted {
            return Ok(true);
        }
    }
}

pub fn run(home: &Path, idle_ms: u64) -> Result<()> {
    // Another worker holds the lock: its run, not this one, says how the work went.
    let Some(held) = lock(home)? else {
        return Ok(());
    };
    let mut phase = curation(home);
    run_holding(
        home,
        idle_ms,
        consumers(home),
        || {},
        Some(held),
        Some(&mut *phase),
    )
}

#[cfg(test)]
fn run_consumers(home: &Path, idle_ms: u64, consumers: Vec<Box<dyn Consumer>>) -> Result<()> {
    let Some(held) = lock(home)? else {
        return Ok(());
    };
    run_holding(home, idle_ms, consumers, || {}, Some(held), None)
}

/// The curation phase: it curates while `[summary] curate` asks for it, off until the cut-over
/// (spec 7.5).
fn curation(home: &Path) -> Box<CurationPhase<'static>> {
    let home = home.to_owned();
    // Opened once curation is on: a home that never asks for it gets no providers.db.
    let mut db = None;
    Box::new(move |raw: &mut Raw, k: &Connection| {
        // Read again for each window: a worker that stays up follows the owner's edits (turning
        // curation on or off, a provider removed, a lower cap). A file that no longer loads (one
        // the owner is still editing) stops curation until it loads again, not the worker.
        // The rules are built only when curation is on: a worker with it off pays one read.
        let loaded = crate::config::load(&home).and_then(|cfg| {
            if !cfg.summary.curate {
                return Ok(None);
            }
            Ok(Some((crate::capture::Settings::load(&home)?.rules, cfg)))
        });
        let (rules, cfg) = match loaded {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(Phase::Idle),
            Err(e) => {
                eprintln!("oboete: no curation for now: {e:#}");
                return Ok(Phase::Idle);
            }
        };
        let db = match &mut db {
            Some(db) => db,
            None => match crate::providers_db::open(&home) {
                Ok(opened) => db.insert(opened),
                Err(e) => {
                    eprintln!("oboete: no curation for now: {e:#}");
                    return Ok(Phase::Idle);
                }
            },
        };
        let mut curator = |span: &str,
                           prompt: &str,
                           working: &dyn Fn() -> Option<i64>,
                           check: &crate::provider::AnswerCheck| {
            crate::provider::Chain::new(&cfg.providers, db)
                .paid_cap(cfg.paid_usd_per_month)
                .idle_gate(working)
                .check(check)
                .run("curator", span, prompt, &crate::curate::schema())
        };
        // Who is asked and within what caps: a window held under other ones is tried again now.
        let chain = format!("{:?} {}", cfg.providers, cfg.paid_usd_per_month);
        crate::curate::run_phase(raw, k, db, &rules, &cfg.summary, &chain, &mut curator)
    })
}

/// A run's outcome until it ends.
const STOPPED: &str = "it stopped before it finished (killed or crashed); the next worker a hook \
                       starts goes on from where it stopped";

/// A worker a hook started writes its stderr nowhere: its last failure is kept for doctor, and a
/// good run clears it. `last` numbers the run's last taking of the worker lock: an outcome is
/// recorded unless a later run's already is. Under a lock of its
/// own, not the worker lock: a hook that finds the worker lock taken starts no worker, and this
/// run no longer reads new records.
fn record(home: &Path, last: u64, result: &Result<()>) {
    let why = result
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    note(home, last, &why);
}

fn note(home: &Path, last: u64, why: &str) {
    let state = home.join("state");
    let Ok(guard) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("worker-note.lock"))
    else {
        return;
    };
    if guard.lock().is_err() {
        return;
    }
    if outcome(home).is_some_and(|(g, _)| g > last) {
        return;
    }
    // The number and the outcome in one file, replaced whole: a run stopped halfway leaves the
    // last outcome as it was.
    let next = state.join("worker-outcome.next");
    if std::fs::write(&next, format!("{last}\n{why}")).is_ok() {
        let _ = std::fs::rename(&next, state.join("worker-outcome"));
    }
}

/// `<home>/state/worker-outcome`: the lock number of the last run that recorded its outcome, and
/// why it stopped with an error (empty when it ended well).
fn outcome(home: &Path) -> Option<(u64, String)> {
    let text = std::fs::read_to_string(home.join("state").join("worker-outcome")).ok()?;
    let (number, why) = text.split_once('\n').unwrap_or((&text, ""));
    Some((number.trim().parse().ok()?, why.to_owned()))
}

/// Why the last `oboete worker` stopped with an error, if it did.
pub fn last_failure(home: &Path) -> Option<String> {
    let (_, why) = outcome(home)?;
    // A run still going has not stopped.
    if why == STOPPED && running(home) {
        return None;
    }
    Some(why).filter(|why| !why.is_empty())
}

/// Whether a process holds the worker lock now. It takes no number: this runs nothing.
pub fn running(home: &Path) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("state").join("worker.lock"))
        .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// `oboete rebuild` (spec 1.7): under the worker lock, knowledge.db is moved aside and every
/// consumer runs from zero over raw.db and the op log, with no curation phase, so no provider is
/// called. The old file is removed once the new one is complete; a rebuild that fails keeps it,
/// named in the error.
pub fn rebuild(home: &Path) -> Result<()> {
    use anyhow::Context;
    let held = lock(home)?
        .ok_or_else(|| anyhow::anyhow!("a worker is running; try again when it has exited"))?;
    let name = format!("knowledge.db.rebuilding-{}", crate::db::now_ms());
    let aside = set_aside(home, &name)?;
    let kept = home.join(&name);
    run_holding(home, 0, consumers(home), || {}, Some(held), None).with_context(|| {
        // A home with no knowledge.db yet set nothing aside.
        if kept.exists() {
            format!(
                "rebuild; the old knowledge.db is kept as {}",
                kept.display()
            )
        } else {
            "rebuild".to_owned()
        }
    })?;
    // The rebuild is complete: an old file that will not go is left and named, not a failure.
    for f in aside {
        if let Err(e) = std::fs::remove_file(&f) {
            eprintln!(
                "oboete: rebuilt; {} is left ({e}): delete it by hand",
                f.display()
            );
        }
    }
    Ok(())
}

/// knowledge.db moved aside as `name`, its sidecars with it under the names SQLite looks for
/// beside that file (`...-wal`, `...-shm`), so the kept file opens with its last commits. Under
/// raw.lock held exclusively, as a restore moves it: every reader of knowledge.db holds raw.db
/// open (a shared hold) while it reads. The sidecars first: never the file's name free with an
/// old WAL beside it that SQLite would replay into the new file. A move that fails puts back the
/// ones before it, so the file never stays without its WAL.
fn set_aside(home: &Path, name: &str) -> Result<Vec<std::path::PathBuf>> {
    use anyhow::Context;
    let _swap = crate::raw::lock_for_swap(home)?;
    // Names joined to `home`, never through its display form: a home path need not be UTF-8.
    let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    for ext in ["-wal", "-shm", ""] {
        let from = home.join(format!("knowledge.db{ext}"));
        if !from.exists() {
            continue;
        }
        let to = home.join(format!("{name}{ext}"));
        if let Err(e) = std::fs::rename(&from, &to) {
            // One that cannot go back is named, so the owner can put it back by hand.
            let mut why = format!("move {}", from.display());
            for (from, to) in moved.iter().rev() {
                if let Err(back) = std::fs::rename(to, from) {
                    why += &format!("; {} stays as {} ({back})", from.display(), to.display());
                }
            }
            return Err(e).context(why);
        }
        moved.push((from, to));
    }
    Ok(moved.into_iter().map(|(_, to)| to).collect())
}

/// One run now, for `oboete restore` and tests. It waits up to 2 s for the lock rather than
/// return at once: a worker a hook started holds it only while it drains, and a lock just
/// released can still be held for a moment by a child another thread forked (it keeps the open
/// file until it execs), which made hook tests run nothing under a parallel suite.
#[allow(dead_code)] // Task 12's replay drains without waiting.
pub fn run_once(home: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut held = lock(home)?;
    while held.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        held = lock(home)?;
    }
    run_holding(home, 0, consumers(home), || {}, held, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge;
    use crate::raw;

    /// A consumer that writes each seq it sees into knowledge.db, so these tests need no index
    /// (Task 6).
    struct Seen;
    impl Consumer for Seen {
        fn name(&self) -> &'static str {
            "seen"
        }
        fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
            k.execute(
                "CREATE TABLE IF NOT EXISTS seen(device TEXT, seq INTEGER)",
                [],
            )?;
            let recs = raw.after(raw.device(), after, 100)?;
            for r in &recs {
                k.execute("INSERT INTO seen VALUES (?1, ?2)", (&r.device, r.seq))?;
            }
            Ok(recs.last().map_or(after, |r| r.seq))
        }
        fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
            k.execute(
                "DELETE FROM seen WHERE device = ?1 AND seq > ?2",
                (device, to),
            )?;
            Ok(())
        }
    }

    /// A consumer of the op log: it counts in op_seqs, for every device with ops.
    struct OpsSeen;
    impl Consumer for OpsSeen {
        fn name(&self) -> &'static str {
            "ops-seen"
        }
        fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
            raw.op_devices()
        }
        fn top(&self, raw: &Raw, device: &str) -> Result<i64> {
            raw.max_op_seq_of(device)
        }
        fn checkpoints(&self) -> &'static str {
            checkpoint::OPS
        }
        fn step(&mut self, raw: &Raw, _: &Connection, device: &str, after: i64) -> Result<i64> {
            Ok(raw
                .ops_after(device, after, 100)?
                .last()
                .map_or(after, |o| o.op_seq))
        }
        fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
    }

    fn two_ops(raw: &mut Raw) {
        let op = || (raw::OpKind::Claim, serde_json::json!({}));
        raw.append_ops(&[op(), op()]).unwrap();
    }

    /// A rebuild keeps the old knowledge.db under names SQLite opens as one database: a commit
    /// still in its WAL reads there.
    #[test]
    fn the_file_a_rebuild_sets_aside_opens_with_its_wal() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        {
            let k = knowledge::open(p).unwrap();
            k.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(7);")
                .unwrap();
            // A copy of the file and its WAL while the commit is only in the WAL (the last
            // close would checkpoint it), put back in place once the connection is gone.
            for ext in ["", "-wal"] {
                std::fs::copy(
                    p.join(format!("knowledge.db{ext}")),
                    p.join(format!("copy{ext}")),
                )
                .unwrap();
            }
        }
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(p.join(format!("knowledge.db{ext}")));
        }
        for ext in ["", "-wal"] {
            std::fs::rename(
                p.join(format!("copy{ext}")),
                p.join(format!("knowledge.db{ext}")),
            )
            .unwrap();
        }
        let aside = set_aside(p, "kept").unwrap();
        let kept = p.join("kept");
        assert!(!p.join("knowledge.db").exists() && aside.contains(&kept));
        let x: i64 = Connection::open(&kept)
            .unwrap()
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, 7);
    }

    /// A home whose path is not UTF-8 (valid on Linux; macOS refuses such a name) keeps its file
    /// under the right name.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_home_path_that_is_not_utf8_is_set_aside_in_place() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(std::ffi::OsStr::from_bytes(b"home-\xff"));
        std::fs::create_dir(&home).unwrap();
        drop(knowledge::open(&home).unwrap());
        let aside = set_aside(&home, "kept").unwrap();
        assert!(aside.contains(&home.join("kept")));
        assert!(
            aside
                .iter()
                .all(|f| f.parent() == Some(home.as_path()) && f.exists())
        );
    }

    /// A move that fails puts back the ones before it: the file never stays without its WAL.
    #[test]
    fn a_set_aside_that_fails_puts_the_wal_back() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("knowledge.db"), b"db").unwrap();
        std::fs::write(p.join("knowledge.db-wal"), b"wal").unwrap();
        // The file's new name is a directory's, so its move fails after the WAL's.
        std::fs::create_dir(p.join("kept")).unwrap();
        std::fs::write(p.join("kept").join("x"), b"").unwrap();
        assert!(set_aside(p, "kept").is_err());
        assert_eq!(std::fs::read(p.join("knowledge.db-wal")).unwrap(), b"wal");
        assert!(!p.join("kept-wal").exists() && p.join("knowledge.db").exists());
    }

    /// MUST-M14 for the op log: a restore that lost ops moves an op consumer back to what raw
    /// holds, even once the device has no op left, and leaves the record consumers where they
    /// were.
    #[test]
    fn a_restore_that_loses_ops_rewinds_the_op_consumers_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&raw::test_event("a")).unwrap();
        two_ops(&mut raw);
        let mut k = knowledge::open(home.path()).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen), Box::new(OpsSeen)];
        drain(&raw, &mut k, &mut consumers).unwrap();
        let device = raw.device().to_owned();
        let ops_at = |k: &Connection| checkpoint::get_in(k, checkpoint::OPS, "ops-seen", &device);
        assert_eq!(ops_at(&k).unwrap(), 2);
        Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops", [])
            .unwrap();
        let moved = checkpoint::rewind(&raw, &k, &mut consumers).unwrap();
        assert_eq!(moved, [("ops-seen".to_owned(), 2, 0)]);
        assert_eq!(ops_at(&k).unwrap(), 0);
        assert_eq!(checkpoint::get(&k, "seen", &device).unwrap(), 1);
    }

    /// Compression waits for what every record consumer has passed: an op consumer's checkpoint
    /// is an op_seq, kept apart, and does not hold it back.
    #[test]
    fn compression_waits_for_the_record_consumers_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        for body in ["a", "b", "c"] {
            raw.append(&raw::test_event(body)).unwrap();
        }
        two_ops(&mut raw);
        let mut k = knowledge::open(home.path()).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![
            Box::new(Seen),
            Box::new(OpsSeen),
            Box::new(crate::consumer::compress::Compress),
        ];
        drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(checkpoint::get(&k, "compress", raw.device()).unwrap(), 3);
    }

    /// A consumer whose step reads, lets another connection write to knowledge.db (as a search
    /// does when it creates its table in a fresh file), then writes itself. Once. The other
    /// writer's thread is kept for the test to join.
    type Contender = std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>;
    struct Raced(std::path::PathBuf, bool, Contender);
    impl Consumer for Raced {
        fn name(&self) -> &'static str {
            "raced"
        }
        fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
            if std::mem::replace(&mut self.1, true) {
                return Ok(after.max(raw.max_seq()?));
            }
            let _: i64 = k.query_row("SELECT COUNT(*) FROM checkpoints", [], |r| r.get(0))?;
            let (home, (done, wait)) = (self.0.clone(), std::sync::mpsc::channel());
            *self.2.lock().unwrap() = Some(std::thread::spawn(move || {
                let other = knowledge::open(&home).unwrap();
                other
                    .execute_batch("CREATE TABLE IF NOT EXISTS other(x)")
                    .unwrap();
                done.send(()).ok();
            }));
            // The other write lands first unless this step already holds the write lock.
            let _ = wait.recv_timeout(Duration::from_millis(500));
            k.execute("CREATE TABLE IF NOT EXISTS raced(x)", [])?;
            raw.max_seq()
        }
        fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_run_leaves_its_reason_and_the_next_good_run_clears_it() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("raw.db"), b"not a database at all").unwrap();
        assert!(run(home.path(), 0).is_err()); // damaged, and no backup to restore from
        let why = last_failure(home.path()).unwrap();
        assert!(why.contains("backup segment"), "{why}");
        std::fs::remove_file(home.path().join("raw.db")).unwrap();
        // A run that finds another worker holding the lock did nothing: the note stays.
        let other = lock(home.path()).unwrap();
        run(home.path(), 0).unwrap();
        assert!(last_failure(home.path()).is_some());
        drop(other);
        run(home.path(), 0).unwrap();
        assert!(last_failure(home.path()).is_none());
    }

    /// A run's outcome is recorded unless a later run's already is, whichever order the two end
    /// in, and recording never holds the worker lock.
    #[test]
    fn a_runs_outcome_is_not_recorded_over_a_later_workers() {
        let home = tempfile::tempdir().unwrap();
        let failed = || Err(anyhow::anyhow!("a failure"));
        let taken = |h: &Path| lock(h).unwrap().unwrap().1;
        // A lets go; B takes the lock, fails and records; then A ends well.
        let (a, b) = (taken(home.path()), taken(home.path()));
        record(home.path(), b, &failed());
        record(home.path(), a, &Ok(()));
        assert!(last_failure(home.path()).is_some(), "A cleared B's failure");
        // B ends well; then A fails.
        let (a, b) = (taken(home.path()), taken(home.path()));
        record(home.path(), b, &Ok(()));
        record(home.path(), a, &failed());
        assert!(last_failure(home.path()).is_none(), "A recorded over B");
        // A records before B, which fails: B's outcome is the last one.
        let (a, b) = (taken(home.path()), taken(home.path()));
        record(home.path(), a, &Ok(()));
        record(home.path(), b, &failed());
        let why = last_failure(home.path()).unwrap();
        assert_eq!(why, "a failure");
        // A lock number lost to a damaged file goes on from the last recorded outcome's.
        std::fs::write(home.path().join("state").join("worker-gen"), "").unwrap();
        let after_damage = taken(home.path());
        record(home.path(), after_damage, &Ok(()));
        assert!(
            last_failure(home.path()).is_none(),
            "an outcome after the damage was refused"
        );
        // A run stopped between writing its outcome and putting it in place changes nothing.
        record(home.path(), taken(home.path()), &failed());
        let state = home.path().join("state");
        std::fs::write(state.join("worker-outcome.next"), "1\nhalf written").unwrap();
        assert_eq!(last_failure(home.path()).as_deref(), Some("a failure"));
        record(home.path(), taken(home.path()), &Ok(()));
        // A worker holding the lock does not keep an outcome from being recorded.
        let running = lock(home.path()).unwrap().unwrap();
        record(home.path(), running.1, &Ok(()));
        assert!(last_failure(home.path()).is_none());
        assert!(lock(home.path()).unwrap().is_none());
    }

    /// A step that panics once it reaches its seq.
    struct PanicsAt(i64);
    impl Consumer for PanicsAt {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn step(&mut self, raw: &Raw, _: &Connection, _device: &str, after: i64) -> Result<i64> {
            let top = raw.max_seq()?;
            assert!(top < self.0, "a crash in the middle of a run");
            Ok(after.max(top))
        }
        fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
    }

    /// A worker killed or crashed after it took the lock never records how its run ended: doctor
    /// says it stopped once no process holds the lock, and not while one does.
    #[test]
    fn a_run_that_never_ends_is_reported_once_its_lock_is_free() {
        let home = tempfile::tempdir().unwrap();
        run(home.path(), 0).unwrap();
        raw::open(home.path())
            .unwrap()
            .append(&raw::test_event("a"))
            .unwrap();
        let crashed =
            std::panic::catch_unwind(|| run_consumers(home.path(), 0, vec![Box::new(PanicsAt(1))]));
        assert!(crashed.is_err());
        let why = last_failure(home.path()).expect("the stopped run was not reported");
        assert!(why.starts_with("it stopped before it finished"), "{why}");
        let held = lock(home.path()).unwrap().unwrap();
        note(home.path(), held.1, STOPPED);
        assert!(
            last_failure(home.path()).is_none(),
            "a live run was reported"
        );
        drop(held);
        assert!(last_failure(home.path()).is_some());
        run(home.path(), 0).unwrap();
        assert!(last_failure(home.path()).is_none());
    }

    /// A run that takes the lock again at its idle exit notes that taking too: B takes and
    /// releases the lock in A's exit window and records its success, then A takes it again and
    /// crashes. Doctor reports A, not B's success.
    #[test]
    fn a_run_that_crashes_after_taking_the_lock_again_is_reported() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&raw::test_event("a")).unwrap();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut once = true;
            run_with(home.path(), 0, vec![Box::new(PanicsAt(2))], || {
                if std::mem::take(&mut once) {
                    // B: a hook's worker, run to its end while A has let go.
                    raw.append(&raw::test_event("b")).unwrap();
                    run_consumers(home.path(), 0, vec![Box::new(Seen)]).unwrap();
                    assert!(last_failure(home.path()).is_none());
                }
            })
        }));
        assert!(crashed.is_err(), "A did not take the lock again");
        let why = last_failure(home.path()).expect("A's crash was not reported");
        assert!(why.starts_with("it stopped before it finished"), "{why}");
    }

    /// D10: a window that waits only on time keeps the worker up until then, and the phase runs
    /// again; a wait beyond 30 minutes, or on the owner or a budget, does not.
    #[test]
    fn the_worker_sleeps_until_the_gate_opens_and_exits_when_the_wait_is_longer() {
        let home = tempfile::tempdir().unwrap();
        raw::open(home.path())
            .unwrap()
            .append(&raw::test_event("a"))
            .unwrap();
        let calls = std::cell::Cell::new(0);
        let (started, again) = (Instant::now(), std::cell::Cell::new(None));
        let until = crate::db::now_ms() + 400;
        let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
            calls.set(calls.get() + 1);
            Ok(match calls.get() {
                1 => Phase::Waiting { until, up: true },
                2 => {
                    again.set(Some(started.elapsed()));
                    Phase::Covered
                }
                _ => Phase::Idle,
            })
        };
        let p: &mut CurationPhase = &mut phase;
        // The window's time, not the longer idle wait, says when the phase runs again.
        run_holding(
            home.path(),
            1_500,
            vec![Box::new(Seen)],
            || {},
            None,
            Some(p),
        )
        .unwrap();
        let again = again.get().expect("the phase did not run again");
        assert!(again >= Duration::from_millis(350), "{again:?}");
        assert!(again < Duration::from_millis(1_200), "{again:?}");
        assert_eq!(calls.get(), 3);

        calls.set(0);
        let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
            calls.set(calls.get() + 1);
            Ok(Phase::Waiting {
                until: crate::db::now_ms() + 3_600_000,
                up: false,
            })
        };
        let started = Instant::now();
        let p: &mut CurationPhase = &mut phase;
        run_holding(home.path(), 0, vec![Box::new(Seen)], || {}, None, Some(p)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(calls.get(), 1);
    }

    /// Curation runs only in a home that asks for it: by default nothing reaches providers.db.
    /// With `[summary] curate = true` the phase runs the configured chain; here its one entry has
    /// no budget left, so the window waits for the next day and nothing leaves the machine.
    #[test]
    fn a_home_curates_only_when_it_asks_to() {
        let home = tempfile::tempdir().unwrap();
        let device = {
            let mut raw = raw::open(home.path()).unwrap();
            raw.append(&raw::test_event("a")).unwrap();
            raw.device().to_owned()
        };
        run(home.path(), 0).unwrap();
        assert!(!home.path().join("providers.db").exists());
        let config = "[summary]\ncurate = true\n[[providers]]\nkind = \"openai\"\n\
                      name = \"spent\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                      daily_budget = 0\n";
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        raw::open(home.path())
            .unwrap()
            .append(&raw::test_event("b"))
            .unwrap();
        run(home.path(), 0).unwrap();
        let db = crate::providers_db::open(home.path()).unwrap();
        let p = crate::providers_db::pending_of(&db, &device)
            .unwrap()
            .expect("the phase did not run");
        assert_eq!((p.hold.as_str(), p.from_seq, p.to_seq), ("budget", 1, 2));
        assert!(p.reason.contains("spent: 0/0 calls today"), "{}", p.reason);
    }

    /// A worker that stays up reads the config again for each window: turning curation on or
    /// off takes effect without a new worker, and a config that no longer loads turns it off.
    #[test]
    fn a_worker_that_stays_up_follows_the_owners_config() {
        let home = tempfile::tempdir().unwrap();
        let config = |curate: bool| {
            let text = format!(
                "[summary]\ncurate = {curate}\n[[providers]]\nkind = \"openai\"\n\
                 name = \"spent\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                 daily_budget = 0\n"
            );
            std::fs::write(home.path().join("config.toml"), text).unwrap();
        };
        config(false);
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&raw::test_event("a")).unwrap();
        let mut phase = curation(home.path());
        assert_eq!(
            phase(&mut raw, &Connection::open_in_memory().unwrap()).unwrap(),
            Phase::Idle
        );
        config(true);
        assert!(matches!(
            phase(&mut raw, &Connection::open_in_memory().unwrap()).unwrap(),
            Phase::Waiting { up: false, .. }
        ));
        config(false);
        assert_eq!(
            phase(&mut raw, &Connection::open_in_memory().unwrap()).unwrap(),
            Phase::Idle
        );
        // A file the owner is still editing stops curation for this run, not the worker.
        std::fs::write(home.path().join("config.toml"), "[summary\n").unwrap();
        assert_eq!(
            phase(&mut raw, &Connection::open_in_memory().unwrap()).unwrap(),
            Phase::Idle
        );
    }

    /// The Windows runner's worker stopped with "database is locked" after a restore: a search
    /// created its table in the fresh knowledge.db between a step's read and its write.
    #[test]
    fn a_reader_writing_knowledge_db_mid_step_does_not_stop_the_pass() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&raw::test_event("a")).unwrap();
        let mut k = knowledge::open(home.path()).unwrap();
        let contender = Contender::default();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Raced(
            home.path().to_path_buf(),
            false,
            contender.clone(),
        ))];
        drain(&raw, &mut k, &mut consumers).unwrap();
        // The other writer ran, and its write went through once the step's had.
        let other = contender.lock().unwrap().take().expect("the step ran");
        other.join().unwrap();
    }

    fn seen(k: &Connection) -> Vec<i64> {
        let mut st = k.prepare("SELECT seq FROM seen ORDER BY seq").unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// raw.db as if its commits above `seq` had never reached the disk (MUST-M14).
    fn lose_after(home: &Path, seq: i64) -> Raw {
        let c = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > ?1", [seq])
            .unwrap();
        drop(c);
        raw::open(home).unwrap()
    }

    #[test]
    fn a_checkpoint_above_raw_is_rewound_with_its_output_and_later_events_are_not_skipped() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let mut k = knowledge::open(home.path()).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen)];
        for i in 0..8 {
            raw.append(&raw::test_event(&i.to_string())).unwrap();
        }
        drain(&raw, &mut k, &mut consumers).unwrap(); // output and checkpoint at 8
        drop(raw);
        let mut raw = lose_after(home.path(), 5);
        assert_eq!(
            checkpoint::rewind(&raw, &k, &mut consumers).unwrap(),
            vec![("seen".into(), 8, 5)]
        );
        assert_eq!(seen(&k), vec![1, 2, 3, 4, 5]); // no output left for the lost 6-8
        let logged: (String, i64, i64) = k
            .query_row("SELECT consumer, was, now FROM rewinds", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(logged, ("seen".into(), 8, 5));
        raw.append(&raw::test_event("new")).unwrap(); // seq 6 again, a different event
        drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(seen(&k), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn an_event_that_arrives_while_the_worker_decides_to_exit_is_still_processed() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut late = true;
        // `before_exit` runs after the lock is released and before the last check: a hook
        // appending here finds the lock free, but this test starts no second worker, so only the
        // last check can pick the event up.
        run_with(p, 0, vec![Box::new(Seen)], || {
            if std::mem::take(&mut late) {
                raw::open(p)
                    .unwrap()
                    .append(&raw::test_event("late"))
                    .unwrap();
            }
        })
        .unwrap();
        let device = raw::open(p).unwrap().device().to_owned();
        let k = knowledge::open(p).unwrap();
        assert_eq!(checkpoint::get(&k, "seen", &device).unwrap(), 2);
    }

    #[test]
    fn a_restore_asked_for_while_the_worker_waits_opens_the_stores_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let worker = {
            let p = p.clone();
            std::thread::spawn(move || run_with(&p, 2_000, vec![Box::new(Seen)], || {}))
        };
        std::thread::sleep(Duration::from_millis(300));
        crate::backup::request_restore(&p);
        let t = Instant::now();
        while crate::backup::restore_requested(&p) {
            assert!(
                t.elapsed() < Duration::from_secs(1),
                "not taken while waiting"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn a_restore_that_fails_keeps_the_request() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(raw::open(p).unwrap());
        std::fs::write(p.join("raw.db"), vec![b'x'; 4096]).unwrap();
        crate::backup::request_restore(p);
        // No backup segment: the restore fails, and so does the worker.
        assert!(run_with(p, 0, vec![Box::new(Seen)], || {}).is_err());
        assert!(crate::backup::restore_requested(p));
    }

    #[test]
    fn a_restore_asked_for_as_the_worker_exits_is_not_lost() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut asked = false;
        // After the release: the hook that asked saw the lock still held and started nothing.
        run_with(p, 0, vec![Box::new(Seen)], || {
            if !std::mem::replace(&mut asked, true) {
                crate::backup::request_restore(p);
            }
        })
        .unwrap();
        assert!(!crate::backup::restore_requested(p));
    }

    #[test]
    fn a_second_worker_exits_at_once() {
        let home = tempfile::tempdir().unwrap();
        let _held = lock(home.path()).unwrap().expect("the first lock");
        assert!(lock(home.path()).unwrap().is_none());
        let t = Instant::now();
        run(home.path(), 60_000).unwrap(); // returns at once: the lock is held
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    /// A waiting worker wakes for a new op of its device as for a record (an owner's correction
    /// appends only an op), so it is applied before the wait ends, not after.
    #[test]
    fn a_worker_picks_up_ops_that_arrive_while_it_waits() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let device = {
            let mut raw = raw::open(&p).unwrap();
            raw.append(&raw::test_event("first")).unwrap();
            raw.device().to_owned()
        };
        let writer = {
            let p = p.clone();
            let device = device.clone();
            std::thread::spawn(move || {
                // Once the worker has drained what was there and waits.
                while knowledge::open(&p)
                    .and_then(|k| checkpoint::get(&k, "seen", &device))
                    .unwrap_or(0)
                    < 1
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                std::thread::sleep(Duration::from_millis(300));
                let mut raw = raw::open(&p).unwrap();
                raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({}))])
                    .unwrap();
            })
        };
        // What the op consumer had passed when the first wait ended (a worker that finds more
        // after it released the lock takes it again and waits once more).
        let at_exit = std::cell::Cell::new(None);
        let consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen), Box::new(OpsSeen)];
        run_with(&p, 2_000, consumers, || {
            let k = knowledge::open(&p).unwrap();
            let passed = checkpoint::get_in(&k, checkpoint::OPS, "ops-seen", &device).unwrap();
            at_exit.set(at_exit.get().or(Some(passed)));
        })
        .unwrap();
        writer.join().unwrap();
        assert_eq!(at_exit.get(), Some(1));
    }

    #[test]
    fn a_worker_picks_up_records_that_arrive_while_it_waits() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let writer = {
            let p = p.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                raw::open(&p)
                    .unwrap()
                    .append(&raw::test_event("second"))
                    .unwrap();
            })
        };
        run_with(&p, 1_500, vec![Box::new(Seen)], || {}).unwrap();
        writer.join().unwrap();
        let device = raw::open(&p).unwrap().device().to_owned();
        let k = knowledge::open(&p).unwrap();
        assert_eq!(checkpoint::get(&k, "seen", &device).unwrap(), 2);
        assert_eq!(seen(&k), vec![1, 2]);
    }
}
