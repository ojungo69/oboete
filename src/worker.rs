//! Design B's worker (docs/milestone-2-plan.md D6, D10; MUST-M14): one per home, started by hooks,
//! it runs this milestone's consumers over `raw.db` in seq order and exits when idle.

use crate::curate::Phase;
use crate::knowledge::checkpoint;
use crate::raw::Raw;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckpointUnit {
    Records,
    Ops,
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum MaintenanceCommit {
    Effect {
        stage: &'static str,
    },
    Index {
        consumer: &'static str,
        unit: CheckpointUnit,
        checkpoint: i64,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndexState {
    #[default]
    NotStarted,
    Complete,
    Failed,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct IndexReceipt {
    pub state: IndexState,
    pub consumer_commits: u64,
    pub stores_changed: bool,
    pub raw_recovery: Option<crate::backup::RestoreReceipt>,
    pub backup_files_written: u64,
    pub backup_files_quarantined: u64,
    pub backup_warnings: u64,
    pub forget_log_warnings: u64,
    pub forget_requests_applied: u64,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct MaintenanceOutcome {
    pub operation: crate::backup::MaintenanceOperation,
    pub effects: crate::backup::Effects,
    pub restore: Option<crate::backup::RestoreReceipt>,
    pub index: IndexReceipt,
    pub cached_vectors_carried: u64,
    pub old_knowledge_kept: bool,
    pub cleanup_warnings: u64,
    pub hybrid_ready: bool,
}
impl MaintenanceOutcome {
    fn new(operation: crate::backup::MaintenanceOperation) -> Self {
        Self {
            operation,
            effects: Default::default(),
            restore: None,
            index: Default::default(),
            cached_vectors_carried: 0,
            old_knowledge_kept: false,
            cleanup_warnings: 0,
            hybrid_ready: false,
        }
    }
    pub(crate) fn committed(&self) -> bool {
        self.effects.committed()
            || self.restore.as_ref().is_some_and(|r| r.effects.committed())
            || self.index.consumer_commits != 0
            || self.index.backup_files_written != 0
            || self.index.stores_changed
            || self.index.backup_files_quarantined != 0
            || self
                .index
                .raw_recovery
                .as_ref()
                .is_some_and(|r| r.effects.committed())
    }
}
#[derive(Debug)]
pub(crate) struct MaintenanceFailure {
    pub outcome: Box<MaintenanceOutcome>,
    pub code: crate::backup::MaintenanceCode,
    pub cause: anyhow::Error,
    pub index_cause: Option<anyhow::Error>,
}
impl MaintenanceFailure {
    fn into_cause(self) -> anyhow::Error {
        debug_assert!(
            self.code != crate::backup::MaintenanceCode::Stale || !self.outcome.committed(),
            "stale consent precedes data effects"
        );
        debug_assert!(self.index_cause.is_none() || self.outcome.index.state == IndexState::Failed);
        self.cause
    }
}
fn maintenance_failure(
    outcome: MaintenanceOutcome,
    cause: anyhow::Error,
    fallback: crate::backup::MaintenanceCode,
) -> MaintenanceFailure {
    let code = cause
        .downcast_ref::<crate::backup::MaintenanceCode>()
        .copied()
        .unwrap_or(fallback);
    MaintenanceFailure {
        outcome: Box::new(outcome),
        code,
        cause,
        index_cause: None,
    }
}

struct IndexReport<'a> {
    receipt: &'a mut IndexReceipt,
    committed: &'a mut dyn FnMut(&MaintenanceCommit),
}
fn report_backup(home: &Path, raw: &Raw, report: &mut Option<&mut IndexReport<'_>>, check: bool) {
    if let Some(report) = report.as_deref_mut() {
        let warnings = if check {
            crate::backup::check_report(home, raw, &mut |stage| report.backup(stage))
        } else {
            crate::backup::run_report(home, raw, &mut |stage| report.backup(stage))
        };
        report.receipt.backup_warnings += warnings;
    } else if check {
        crate::backup::check(home, raw);
    } else {
        crate::backup::run(home, raw);
    }
}
impl IndexReport<'_> {
    fn changed(&mut self, before: &str, after: &str, stage: &'static str) {
        if before != after {
            self.receipt.stores_changed = true;
            (self.committed)(&MaintenanceCommit::Effect { stage });
        }
    }
    fn backup(&mut self, stage: &'static str) {
        if stage == "backup_written" {
            self.receipt.backup_files_written += 1;
        } else {
            self.receipt.backup_files_quarantined += 1;
        }
        (self.committed)(&MaintenanceCommit::Effect { stage });
    }
}
fn store_stamp(home: &Path) -> Result<String> {
    let mut states = Vec::new();
    for name in [
        "raw.db",
        "raw.db.restored",
        "knowledge.db",
        "raw.db-wal",
        "knowledge.db-wal",
    ] {
        let path = home.join(name);
        match std::fs::metadata(&path) {
            Ok(metadata) if !name.ends_with("-wal") || metadata.len() > 32 => {
                anyhow::ensure!(
                    metadata.is_file(),
                    "maintenance store is not a regular file"
                );
                #[cfg(test)]
                if name.ends_with("-wal")
                    && let Some(before) = BEFORE_WAL_HASH.take()
                {
                    before(&path);
                }
                let identity = crate::db::store_file(&path);
                let version = match crate::migrate::file_version(&path) {
                    Ok(version) => version,
                    Err(error)
                        if name.ends_with("-wal")
                            && error
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                states.push((name, identity, version));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(crate::forget::hash(&serde_json::to_vec(&states)?))
}
fn log_stamp(home: &Path) -> String {
    let mut logs = vec![home.join("forget.log")];
    if let Ok(dir) = crate::backup::dir(home) {
        logs.push(dir.join("forget.log"));
    }
    let versions: Vec<_> = logs
        .into_iter()
        .map(|path| crate::migrate::file_version(&path).ok())
        .collect();
    crate::forget::hash(&serde_json::to_vec(&versions).expect("log versions serialize"))
}

/// One derived view of raw.db. `step` processes what `device` holds after `after` and returns its
/// new checkpoint; `rewind` deletes its output above `to`. Both run inside the knowledge.db
/// transaction that also moves the checkpoint (D10). A consumer of raw's records reads this
/// device's (other devices' come with sync, milestone 6); a consumer of the op log (milestone 3
/// D4) reads every device that has ops, and counts in op_seqs.
pub trait Consumer {
    fn name(&self) -> &'static str;
    /// Whether it consumes the op log (milestone 3 D4) rather than this device's records: the
    /// three methods below follow from it.
    fn reads_ops(&self) -> bool {
        false
    }
    /// The devices `step` is called for.
    fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
        if self.reads_ops() {
            raw.op_devices()
        } else {
            Ok(vec![raw.device().to_owned()])
        }
    }
    /// The highest checkpoint `device` allows: a checkpoint above it lost its commits (MUST-M14).
    fn top(&self, raw: &Raw, device: &str) -> Result<i64> {
        if self.reads_ops() {
            raw.max_op_seq_of(device)
        } else {
            raw.max_seq_of(device)
        }
    }
    /// Where its checkpoints are kept: `checkpoint::OPS` for op_seqs, else `checkpoint::SEQS`.
    fn checkpoints(&self) -> &'static str {
        if self.reads_ops() {
            checkpoint::OPS
        } else {
            checkpoint::SEQS
        }
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
        // The op log's cards (docs/cards.md): what each curated window was.
        Box::new(crate::consumer::cards::Cards),
        // The op log's session summaries (docs/summaries.md): what each turn was.
        Box::new(crate::consumer::turns::Turns),
        // Imported documents (milestone 4 D5), for search only.
        Box::new(crate::consumer::imported::Imported),
        Box::new(crate::consumer::manifest::Manifest::new(home)),
        Box::new(crate::consumer::gaps::Gaps::new(home)),
        Box::new(crate::consumer::compress::Compress),
    ]
}

/// Runs each consumer from its checkpoint until none moves; each step and its checkpoint move
/// share one knowledge.db transaction. A step may move its checkpoint back (the rescan starting
/// again under new rules). The worker's own loop runs `pass` itself: it checks the backup
/// deadline between passes.
pub fn drain(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<()> {
    while pass(raw, k, consumers)? {}
    Ok(())
}

/// One batch for each consumer: whether any checkpoint moved.
fn pass(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<bool> {
    pass_report(raw, k, consumers, &mut None)
}
fn pass_report(
    raw: &Raw,
    k: &mut Connection,
    consumers: &mut [Box<dyn Consumer>],
    report: &mut Option<&mut IndexReport<'_>>,
) -> Result<bool> {
    let mut advanced = false;
    for c in consumers.iter_mut() {
        for device in c.devices(raw)? {
            // Immediate: a step reads before it writes, and a deferred transaction whose snapshot
            // another writer moved meanwhile (a search creating its table in a fresh knowledge.db)
            // fails its first write with SQLITE_BUSY at once, which stopped the worker.
            let tx = k.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changes = tx.total_changes();
            let schema: i64 = if report.is_some() {
                tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?
            } else {
                0
            };
            let at = checkpoint::get_in(&tx, c.checkpoints(), c.name(), &device)?;
            let next = c.step(raw, &tx, &device, at)?;
            if next != at {
                checkpoint::set_in(&tx, c.checkpoints(), c.name(), &device, next)?;
                advanced = true;
            }
            let changed = report.is_some()
                && (tx.total_changes() != changes
                    || tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))?
                        != schema);
            tx.commit()?;
            if changed && let Some(report) = report.as_deref_mut() {
                report.receipt.consumer_commits += 1;
                (report.committed)(&MaintenanceCommit::Index {
                    consumer: c.name(),
                    unit: if c.reads_ops() {
                        CheckpointUnit::Ops
                    } else {
                        CheckpointUnit::Records
                    },
                    checkpoint: next,
                });
                crate::backup::fail_after("index_commit")?;
            }
        }
    }
    Ok(advanced)
}

/// The home this run locked was removed, or removed and made again (docs/resident.md R3).
#[derive(Debug)]
pub struct Gone;

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the home was removed or replaced while this ran: nothing more is written to it",
        )
    }
}

impl std::error::Error for Gone {}

/// The per-home worker lock, `<home>/state/worker.lock`; released when dropped. Each taking of it
/// has the next number of `state/worker-gen`, so a run's outcome is ordered against a later run's.
pub struct Lock(
    #[allow(dead_code)] std::fs::File,
    u64,
    Option<crate::executable::CommandHome>,
);

/// Which file a lock file is: its device and inode. `None` where the system has none to give, and
/// for a file that cannot be read.
pub(crate) type FileId = Option<(u64, u64)>;

pub(crate) fn file_id(file: std::io::Result<std::fs::Metadata>) -> FileId {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file.ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        None
    }
}

/// The lock, or `None` when another process holds it. Hooks try it too, and start a worker only
/// when they get it (dropping it at once).
pub fn lock(home: &Path) -> Result<Option<Lock>> {
    lock_for(home, None)
}
fn lock_for(home: &Path, command: Option<&crate::executable::CommandHome>) -> Result<Option<Lock>> {
    if let Some(command) = command {
        command.check(home)?;
    } else {
        crate::executable::check_home(home, Some(crate::executable::Role::Worker))?;
    }
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("worker.lock"))?;
    match try_lock(&f) {
        Ok(()) => {
            if command.is_none() {
                crate::executable::check_lock(
                    Some(crate::executable::Role::Worker),
                    file_id(f.metadata()),
                )?;
            }
            #[cfg(test)]
            if let Some(after) = AFTER_OPEN.get() {
                after(home);
            }
            let command = command
                .map(|command| command.locked(home, file_id(f.metadata())))
                .transpose()?;
            // The home was replaced since the file was opened: the lock is the old one's, and
            // nothing is written into the new one by path (R3, Codex on #359). A command asking
            // for the lock takes the new home's at its next try.
            if file_id(std::fs::metadata(state.join("worker.lock"))) != file_id(f.metadata()) {
                return Ok(None);
            }
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
            Ok(Some(Lock(f, taken, command)))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Where commands that need the worker lock ask the worker that holds it to step aside, a file
/// each, so one that gives up takes only its own request away (docs/resident.md R12).
fn yield_requests(home: &Path) -> std::path::PathBuf {
    home.join("state").join("worker-yield")
}

/// A request older than this was left by a command that died: a command waits half of it.
const STALE: Duration = Duration::from_secs(60);

/// How long ago a request was made; none when that cannot be read or is after now.
fn age(request: &std::fs::DirEntry) -> Option<Duration> {
    request.metadata().ok()?.modified().ok()?.elapsed().ok()
}

/// Whether a command asked within the last minute. One dated after now (the clock went back) is
/// not obeyed either: a worker that works on is the smaller harm than one that never works.
pub(crate) fn asked_aside(home: &Path) -> bool {
    std::fs::read_dir(yield_requests(home))
        .into_iter()
        .flatten()
        .flatten()
        .any(|r| age(&r).is_some_and(|age| age < STALE))
}

/// R12, the worker's side: when a command asked, what is new is backed up (a restore reads the
/// backups) and the loop ends. Whether it does. The lock goes once the loop's stores are closed
/// (`run_holding`): the command that takes it may swap them at once.
fn steps_aside(home: &Path, raw: &Raw, holding: &Holding) -> bool {
    if !asked_aside(home) || gone(home, holding) {
        return false;
    }
    crate::backup::run(home, raw);
    #[cfg(test)]
    if let Some(after) = AFTER_BACKUP.get() {
        after(home);
    }
    // A command that gave up meanwhile took its request away: the worker goes on, since a hook
    // that appended during the backup found the lock held and started none (Codex on #359).
    asked_aside(home)
}

#[cfg(test)]
thread_local! {
    /// A test seam: what happens while the step-aside backup runs, as a command giving up.
    static AFTER_BACKUP: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A test seam: what happens once the lock file is open and locked, as a home replaced.
    static AFTER_OPEN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A one-shot test seam: the pathname changes after SQLite opened raw.db.
    static AFTER_RAW_OPEN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A one-shot test seam: the pathname changes between the last check and the next round.
    static BEFORE_ROUND: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A synchronous one-shot fixture action after a rebuild meets a shared status probe.
    static REBUILD_STATUS_BLOCKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CARRY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_WAL_HASH: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
}

/// How long a command waits for a worker to step aside.
const ASK: Duration = Duration::from_secs(if cfg!(test) { 3 } else { 30 });

/// R12, the command's side: the lock for a command that cannot run beside a worker (`oboete
/// restore`, `rebuild`, `recurate --yes`). A worker that holds it is asked to step aside, which
/// it does between rounds and while it waits, not during a call to a provider.
pub fn lock_asking(home: &Path) -> Result<Lock> {
    lock_asking_for(home, None)
}
fn lock_asking_for(home: &Path, command: Option<&crate::executable::CommandHome>) -> Result<Lock> {
    if let Some(held) = lock_for(home, command)? {
        return Ok(held);
    }
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = yield_requests(home);
    std::fs::create_dir_all(&dir)?;
    for old in std::fs::read_dir(&dir)?.flatten() {
        if age(&old).is_some_and(|age| age >= STALE) {
            let _ = std::fs::remove_file(old.path());
        }
    }
    let n = ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ask = dir.join(format!("{}.{n}", std::process::id()));
    std::fs::write(&ask, "")?;
    let until = Instant::now() + ASK;
    let held = loop {
        match lock_for(home, command) {
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
            tried => break tried,
        }
    };
    let _ = std::fs::remove_file(&ask);
    held?.ok_or_else(|| {
        anyhow::anyhow!(
            "the worker is busy and did not step aside within {} s (a call to a provider can \
             take minutes); try again later",
            ASK.as_secs()
        )
    })
}

/// The lock for a command that plans from knowledge.db and then appends to raw (`oboete
/// recurate`), with the consumers drained under it first as a worker drains them: the plan reads
/// what raw holds, and no worker or other command moves it until the lock is dropped. `None` when
/// another process holds it and the command did not `ask` for it.
pub fn drained(home: &Path, ask: bool) -> Result<Option<Lock>> {
    let held = if ask {
        Some(lock_asking(home)?)
    } else {
        lock(home)?
    };
    let Some(held) = held else {
        return Ok(None);
    };
    let raw = crate::backup::open_raw(home)?;
    let mut k = crate::backup::open_knowledge(home)?;
    let mut consumers = consumers(home);
    checkpoint::rewind(&raw, &k, &mut consumers)?;
    drain(&raw, &mut k, &mut consumers)?;
    Ok(Some(held))
}

/// How often a worker looks for new records while it waits.
const POLL: Duration = Duration::from_millis(200);

/// The most records a consumer has yet to read on one device: what a drain has before it
/// (milestone 4 D17).
pub fn backlog(home: &Path) -> Result<i64> {
    let raw = crate::backup::open_raw(home)?;
    let k = crate::backup::open_knowledge(home)?;
    let mut most = 0;
    for c in consumers(home) {
        for device in c.devices(&raw)? {
            let read = checkpoint::get_in(&k, c.checkpoints(), c.name(), &device)?;
            most = most.max(c.top(&raw, &device)? - read);
        }
    }
    Ok(most)
}

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
    run_holding(
        home,
        idle_ms,
        consumers,
        before_exit,
        None,
        Phases::default(),
    )
}

/// The curation phase a worker runs after its consumers have drained (milestone 3 D3).
pub type CurationPhase<'a> = dyn FnMut(&mut Raw, &Connection) -> Result<Phase> + 'a;

/// The phases a worker runs after its consumers have drained, in this order (milestone 4 D8,
/// D9): embedding, the shortlists, then curation. `rebuild`, `run_once` and `drained` run none.
#[derive(Default)]
pub struct Phases<'a, 'f> {
    pub embed: Option<&'a mut crate::embed_phase::Phase>,
    pub shortlist: Option<&'a mut crate::shortlist::Builder>,
    pub curation: Option<&'a mut CurationPhase<'f>>,
    /// `oboete worker` itself, not a command that borrows its loop: it steps aside for a command
    /// that asks for the lock (docs/resident.md R12).
    pub yields: bool,
    /// It stays when it is idle (R2, R3). Only `run_default` sets it, from `[worker] resident`.
    pub resident: bool,
    /// What starts the resident viewer, for a resident worker (docs/resident.md R4).
    pub viewer: Option<&'a mut crate::view::Starter>,
}

pub(crate) fn run_holding(
    home: &Path,
    idle_ms: u64,
    consumers: Vec<Box<dyn Consumer>>,
    before_exit: impl FnMut(),
    taken: Option<Lock>,
    phases: Phases,
) -> Result<()> {
    run_holding_report(
        home,
        idle_ms,
        consumers,
        before_exit,
        taken,
        phases,
        &mut None,
    )
}
fn run_holding_report(
    home: &Path,
    idle_ms: u64,
    mut consumers: Vec<Box<dyn Consumer>>,
    mut before_exit: impl FnMut(),
    taken: Option<Lock>,
    mut phases: Phases,
    report: &mut Option<&mut IndexReport<'_>>,
) -> Result<()> {
    let command = taken.as_ref().and_then(|held| held.2.clone());
    let initial = if let Some(command) = &command {
        command.check(home)?;
        None
    } else {
        crate::executable::expected(Some(crate::executable::Role::Worker))?
    };
    let mut holding = Holding {
        home: initial,
        command,
        ..Holding::default()
    };
    if let Some(l) = taken {
        take(home, l, &mut holding);
    }
    let result = serve_until_done(
        home,
        idle_ms,
        &mut consumers,
        &mut before_exit,
        &mut holding,
        &mut phases,
        report,
    );
    // Released first: a hook that finds the lock free starts a worker for what it appended.
    holding.lock = None;
    // A run that never took the lock did no work: another worker's outcome stands. And no
    // outcome goes into a home that is not this run's (R3).
    if holding.last > 0 && !gone(home, &holding) {
        record(home, holding.last, &result);
    }
    result
}

/// config.toml as a resident worker last saw it (R3): its time and size, `None` with no file.
fn config_stamp(home: &Path) -> Option<(std::time::SystemTime, u64)> {
    let file = std::fs::metadata(home.join("config.toml")).ok()?;
    Some((file.modified().ok()?, file.len()))
}

/// R3: whether the home is no longer the one this run locked: its lock file is missing, or is
/// another file than the one this run took first. It is asked before every write the loop makes
/// by path (a backup, the prune, an outcome, the lock taken again), which would go into another
/// home, and it holds after the lock is released too.
fn gone(home: &Path, holding: &Holding) -> bool {
    (holding.home.is_some()
        && file_id(std::fs::metadata(home.join("state").join("worker.lock"))) != holding.home)
        || holding
            .command
            .as_ref()
            .is_some_and(|command| command.check(home).is_err())
}

/// Every lock this run takes is noted as a run that has not ended, until `record` replaces the
/// note: a worker killed or crashed while it holds any of them is reported (doctor).
fn take(home: &Path, l: Lock, holding: &mut Holding) {
    // The first lock this run takes says which home is its own.
    if holding.home.is_none() {
        holding.home = file_id(l.0.metadata());
    }
    holding.last = l.1;
    // Not into a home replaced since the lock was opened; the run stops at its next check (R3,
    // Codex on #359).
    if !gone(home, holding) {
        note(home, l.1, STOPPED);
    }
    holding.lock = Some(l);
}

/// The worker lock while this run holds it, the number of its last taking, and the lock file of
/// its first one.
#[derive(Default)]
struct Holding {
    lock: Option<Lock>,
    last: u64,
    home: FileId,
    command: Option<crate::executable::CommandHome>,
    executable: crate::executable::Watch,
}

fn serve_until_done(
    home: &Path,
    idle_ms: u64,
    consumers: &mut [Box<dyn Consumer>],
    before_exit: &mut impl FnMut(),
    holding: &mut Holding,
    phases: &mut Phases,
    report: &mut Option<&mut IndexReport<'_>>,
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
            (holding, report),
            &mut next_backup,
            before_exit,
            phases,
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
    state: (&mut Holding, &mut Option<&mut IndexReport<'_>>),
    next_backup: &mut Instant,
    before_exit: &mut impl FnMut(),
    phases: &mut Phases,
) -> Result<bool> {
    let (holding, report) = state;
    let reopened = holding.lock.is_some();
    if !reopened {
        match lock_for(home, holding.command.as_ref())? {
            Some(l) => take(home, l, holding),
            None => return Ok(false),
        }
    }
    // The stores are opened by path too: after a restore, in a home that may be another by now,
    // which is checked before anything is written to it (Codex on #359).
    if gone(home, holding) {
        return Err(Gone.into());
    }
    if reopened {
        // Opened again (a restore was asked for, or a store read as damaged), maybe from a wait
        // whose outcome said all is well: this is work, and a kill during it is reported (R10,
        // Codex on #359).
        note(home, holding.last, STOPPED);
    }
    // Taken before the open, so a request a hook makes while it runs is still seen below; put
    // back when the open fails (no segment, a reader holding raw.lock), for the next worker,
    // which the next hook starts.
    let asked = crate::backup::take_restore_request(home);
    let before = file_id(std::fs::metadata(home.join("raw.db")));
    // Task 8: a damaged raw.db is restored from the backups, a damaged knowledge.db rebuilt.
    let opened = (|| {
        let stamp = if report.is_some() {
            Some(store_stamp(home)?)
        } else {
            None
        };
        let opened = if let Some(report) = report.as_deref_mut() {
            crate::backup::open_raw_report(
                home,
                &mut report.receipt.raw_recovery,
                holding.command.as_ref(),
                &mut |stage| (report.committed)(&MaintenanceCommit::Effect { stage }),
            )
        } else {
            crate::backup::open_raw(home)
        };
        if gone(home, holding) {
            return Err(Gone.into());
        }
        if let Some(report) = report.as_deref_mut() {
            report.changed(
                stamp.as_deref().expect("reported stamp"),
                &store_stamp(home)?,
                "stores_changed",
            );
        }
        opened
    })();
    let mut raw = opened.inspect_err(|_| {
        if asked && !gone(home, holding) {
            crate::backup::request_restore(home);
        }
    })?;
    #[cfg(test)]
    if let Some(after) = AFTER_RAW_OPEN.take() {
        after(home);
    }
    if gone(home, holding) {
        return Err(Gone.into());
    }
    // Milestone 5: the raw.db this run holds. Another file in its place (an old copy put back by
    // hand, docs/milestone-5-plan.md F2) is opened again at the next check, as a restore is, so
    // the forget logs are reconciled into it and its new records are read.
    let held = file_id(Ok(
        std::fs::metadata(home.join("raw.db")).context("inspect raw.db after opening")?
    ));
    // A replacement during open cannot become the old connection's held identity. A fresh
    // None -> Some open is verified by opening it again once the pathname has an identity.
    // Where FileId is unavailable, both observations remain None as before.
    if before != held {
        return Ok(true);
    }
    let replaced = || {
        let now = file_id(std::fs::metadata(home.join("raw.db")));
        held.is_some() && now.is_some() && now != held
    };
    let stamp = if report.is_some() {
        Some(store_stamp(home)?)
    } else {
        None
    };
    let opened = crate::backup::open_knowledge(home);
    if let Some(report) = report.as_deref_mut() {
        report.changed(
            stamp.as_deref().expect("reported stamp"),
            &store_stamp(home)?,
            "stores_changed",
        );
    }
    let mut k = opened?;
    let stamp = if report.is_some() {
        Some(store_stamp(home)?)
    } else {
        None
    };
    let rewound = checkpoint::rewind(&raw, &k, consumers);
    if let Some(report) = report.as_deref_mut() {
        report.changed(
            stamp.as_deref().expect("reported stamp"),
            &store_stamp(home)?,
            "stores_changed",
        );
        if let Ok(moved) = &rewound {
            for (name, _, to) in moved {
                if let Some(consumer) = consumers.iter().find(|c| c.name() == name) {
                    report.receipt.consumer_commits += 1;
                    (report.committed)(&MaintenanceCommit::Index {
                        consumer: consumer.name(),
                        unit: if consumer.reads_ops() {
                            CheckpointUnit::Ops
                        } else {
                            CheckpointUnit::Records
                        },
                        checkpoint: *to,
                    });
                }
            }
        }
    }
    rewound?;
    report_backup(home, &raw, report, true);
    // Milestone 5 D1 rule 7: the forget request logs after the segments are checked, before any
    // consumer runs and before the export.
    if let Some(report) = report.as_deref_mut() {
        let before = (store_stamp(home)?, log_stamp(home));
        let reconciled = crate::forget::reconcile(home, &mut raw);
        let after = (store_stamp(home)?, log_stamp(home));
        report.changed(
            &format!("{}:{}", before.0, before.1),
            &format!("{}:{}", after.0, after.1),
            "forget_reconciled",
        );
        let reconciled = reconciled?;
        report.receipt.forget_requests_applied += reconciled.applied as u64;
        report.receipt.forget_log_warnings += reconciled.problems.len() as u64;
        for problem in reconciled.problems {
            eprintln!("oboete: forget request log: {problem}");
        }
    } else {
        crate::forget::reconcile_or_say(home, &mut raw)?;
    }
    // The resident viewer is started where the backup deadline is looked at (R4). True means
    // the scheduled export first needs the stores reopened.
    let (resident, yields) = (phases.resident, phases.yields);
    let mut due = |raw: &Raw,
                   holding: &Holding,
                   viewer: Option<&mut crate::view::Starter>,
                   report: &mut Option<&mut IndexReport<'_>>| {
        if gone(home, holding) {
            return false;
        }
        if Instant::now() >= *next_backup {
            if replaced() {
                return true;
            }
            report_backup(home, raw, report, false);
            *next_backup = Instant::now() + crate::backup::EVERY;
        }
        if let Some(viewer) = viewer {
            if resident {
                viewer.due(home);
            } else {
                viewer.reap();
            }
        }
        false
    };
    let mut config = config_stamp(home);
    // Whether a round ran since a resident worker's last idle step (R3).
    let mut ran;
    // R10: a resident worker is killed at every shutdown of the PC. While it waits its outcome
    // says all is well, so doctor reports a kill during a round and none during a wait.
    let mut waited = false;
    // R3: told to leave while a call is out, it settles that call and leaves, and starts no other
    // in between (Codex on #359).
    let mut departing = false;
    loop {
        #[cfg(test)]
        if let Some(before) = BEFORE_ROUND.take() {
            before(home);
        }
        if gone(home, holding) {
            return Err(Gone.into());
        }
        // The next phase round uses the pathname's current store. An end-of-round check
        // cannot cover a replacement that happened after it and before this first pass.
        if crate::backup::restore_requested(home) || replaced() {
            return Ok(true);
        }
        // An embedding call that is out is work in flight: its answer is settled before the
        // worker steps aside (R12) and before its outcome says all is well (R10).
        let calling = |phases: &Phases| phases.embed.as_ref().is_some_and(|e| e.busy());
        if yields && !calling(phases) {
            match holding.executable.change() {
                crate::executable::Change::Replaced if crate::executable::startup_ready(home) => {
                    let _ = crate::executable::exec(
                        crate::executable::Role::Worker,
                        holding.home,
                        phases
                            .viewer
                            .as_deref()
                            .and_then(crate::view::Starter::child_id),
                    );
                }
                crate::executable::Change::Missing => return Ok(false),
                crate::executable::Change::Stay | crate::executable::Change::Replaced => {}
            }
        }
        // And once a command has asked, or the worker is to leave, no other call is sent, by the
        // embedding phase or by curation: a backlog would keep the command waiting for as long as
        // it lasts.
        let asked = yields && asked_aside(home);
        let hold = asked || departing;
        if let Some(e) = phases.embed.as_mut() {
            e.hold(hold);
        }
        if asked && !calling(phases) && steps_aside(home, &raw, holding) {
            return Ok(false);
        }
        ran = true;
        if std::mem::take(&mut waited) {
            note(home, holding.last, STOPPED);
        }
        // What the pass below reads up to, new records or new ops of this device (an owner's
        // correction appends only an op): one that lands after it, even before the wait, wakes it.
        let seen = (raw.max_seq()?, raw.max_op_seq_of(raw.device())?);
        while pass_report(&raw, &mut k, consumers, report)? {
            // Between two batches too: a consumer opens the home's files by path (the rescan).
            if gone(home, holding) {
                return Err(Gone.into());
            }
            if due(&raw, holding, phases.viewer.as_deref_mut(), report) {
                return Ok(true);
            }
        }
        if due(&raw, holding, phases.viewer.as_deref_mut(), report) {
            return Ok(true);
        }
        // D3 and milestone 4's D8 and D9: once the consumers have drained, the embedding phase, the
        // shortlists, then one window. A call in flight, or a window that waits only on time
        // within D10's 30 minutes, keeps the worker up until then; a phase's progress starts the
        // next round. A resident worker is up for every wait (R3): no hook starts it again.
        let (mut stay, mut again) = (None, false);
        if let Some(embed) = phases.embed.as_mut() {
            match embed.poll(&raw, &k)? {
                Phase::Covered => again = true,
                Phase::Waiting { until, up } if up || resident => stay = Some(until),
                Phase::Waiting { .. } | Phase::Idle => {}
            }
        }
        if let Some(shortlist) = phases.shortlist.as_mut()
            && shortlist.run(
                &raw,
                &mut k,
                phases.embed.as_deref_mut(),
                crate::db::now_ms(),
            )? == Phase::Covered
        {
            again = true;
        }
        if !hold && let Some(phase) = phases.curation.as_mut() {
            match phase(&mut raw, &k)? {
                Phase::Covered => again = true,
                Phase::Waiting { until, up } if up || resident => {
                    stay = Some(stay.map_or(until, |s: i64| s.min(until)));
                }
                Phase::Waiting { .. } | Phase::Idle => {}
            }
        }
        // Every phase can start another round. A progressed embed or shortlist must not keep
        // the old raw handle while curation is idle or waiting (M5 F2).
        if crate::backup::restore_requested(home) || replaced() {
            return Ok(true);
        }
        if again {
            continue;
        }
        if resident && !calling(phases) {
            if gone(home, holding) {
                return Err(Gone.into());
            }
            note(home, holding.last, "");
            waited = true;
        }
        // A window's time replaces the idle wait: the phase runs again then, and the idle wait
        // starts once it has nothing left to wait for. A resident worker (R3) waits again after
        // each idle time that passes with nothing new, also within a longer wait for a phase.
        let leaving = loop {
            if departing && !calling(phases) {
                break true;
            }
            let phase = stay.map(|until| u64::try_from(until - crate::db::now_ms()).unwrap_or(0));
            let wait = match phase {
                Some(phase) if resident => phase.min(idle_ms),
                Some(phase) => phase,
                None => idle_ms,
            };
            let deadline = Instant::now() + Duration::from_millis(wait);
            let mut more = false;
            while Instant::now() < deadline {
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
                // R3 takes precedence over R9: exec must not recreate a removed/replaced home.
                if gone(home, holding) {
                    return Err(Gone.into());
                }
                if yields && !calling(phases) {
                    match holding.executable.change() {
                        crate::executable::Change::Replaced
                            if crate::executable::startup_ready(home) =>
                        {
                            let _ = crate::executable::exec(
                                crate::executable::Role::Worker,
                                holding.home,
                                phases
                                    .viewer
                                    .as_deref()
                                    .and_then(crate::view::Starter::child_id),
                            );
                        }
                        crate::executable::Change::Missing => return Ok(false),
                        crate::executable::Change::Stay | crate::executable::Change::Replaced => {}
                    }
                }
                if crate::backup::restore_requested(home) || replaced() {
                    return Ok(true);
                }
                if yields && !calling(phases) && steps_aside(home, &raw, holding) {
                    return Ok(false);
                }
                // A call that came back is written at once. And a request that went away (its
                // command gave up) starts a round, which lets the embedding go on: a resident
                // worker starts none at its idle time.
                if (raw.max_seq()?, raw.max_op_seq_of(raw.device())?) != seen
                    || phases.embed.as_ref().is_some_and(|e| e.done())
                    || (asked && !asked_aside(home))
                {
                    more = true;
                    break;
                }
                if due(&raw, holding, phases.viewer.as_deref_mut(), report) {
                    return Ok(true);
                }
            }
            if more || phase == Some(wait) {
                break false;
            }
            // The idle time passed with nothing new: a backup and a prune follow, here or below.
            if gone(home, holding) {
                return Err(Gone.into());
            }
            if !resident {
                break true;
            }
            // Its idle step: what an exit does, once for the rounds since the last one.
            if std::mem::take(&mut ran) {
                report_backup(home, &raw, report, false);
                crate::hookstate::prune(home, crate::hookstate::KEEP);
            }
            // Only a config.toml that loads and does not say `resident = true` ends it: one the
            // owner is still editing leaves it as it is. A call that is out is settled first.
            if crate::config::worker(home).is_ok_and(|w| !w.resident) {
                if !calling(phases) {
                    break true;
                }
                departing = true;
            }
            // A setting the owner changed is followed without a new record.
            let now = config_stamp(home);
            if now != config {
                config = now;
                break false;
            }
        };
        if !leaving {
            continue;
        }
        // A zero-length wait has no polling check, and the file may have changed since the
        // last tick of a longer one. Reopen before backing up through the old connection.
        if replaced() {
            return Ok(true);
        }
        // Under the lock: a worker started after the release cannot export the same seqs.
        report_backup(home, &raw, report, false);
        crate::hookstate::prune(home, crate::hookstate::KEEP);
        holding.lock = None;
        before_exit();
        // Before the lock is taken again, which writes its number by path.
        if gone(home, holding) {
            return Err(Gone.into());
        }
        // A hook that asked before the release saw the lock held and started nothing.
        let wanted = crate::backup::restore_requested(home) || replaced();
        if !wanted && !behind(&raw, &k, consumers)? {
            return Ok(false);
        }
        match lock_for(home, holding.command.as_ref())? {
            Some(l) => take(home, l, holding),
            // Another worker took the lock after the release: the records are its now.
            None => return Ok(false),
        }
        if wanted {
            return Ok(true);
        }
    }
}

/// `oboete worker --idle-ms N`: it exits after that long with no new record, whatever
/// config.toml says (docs/resident.md R2).
pub fn run(home: &Path, idle_ms: u64) -> Result<()> {
    run_as(home, idle_ms, false)
}

/// The idle time of a worker started with none.
const IDLE_MS: u64 = 60_000;

/// `oboete worker`, as a hook starts it: in a home whose config.toml says `[worker] resident =
/// true` it stays when idle (R2).
pub fn run_default(home: &Path) -> Result<()> {
    run_as(home, IDLE_MS, true)
}

/// `follow`: whether `[worker] resident` is read. Only here, never in the loop: `run_once`,
/// `rebuild` and the other commands that borrow it exit at idle in every home. A file that does
/// not load makes no resident worker, as before; Linux only until the other systems' unit.
fn run_as(home: &Path, idle_ms: u64, follow: bool) -> Result<()> {
    let resident = follow
        && cfg!(target_os = "linux")
        && crate::config::worker(home).is_ok_and(|w| w.resident);
    // Another worker holds the lock: its run, not this one, says how the work went.
    let Some(held) = lock(home)? else {
        return Ok(());
    };
    let mut curation = curation(home);
    let mut embed = crate::embed_phase::Phase::new(home);
    let mut shortlist = crate::shortlist::Builder::new(home);
    let mut viewer = crate::view::Starter::new();
    // The viewer before the stores: one that does not open still leaves the page up (Codex on
    // #378).
    if resident {
        viewer.due(home);
    }
    let phases = Phases {
        embed: Some(&mut embed),
        shortlist: Some(&mut shortlist),
        curation: Some(&mut *curation),
        yields: true,
        resident,
        viewer: Some(&mut viewer),
    };
    run_holding(home, idle_ms, consumers(home), || {}, Some(held), phases)
}

#[cfg(test)]
fn run_consumers(home: &Path, idle_ms: u64, consumers: Vec<Box<dyn Consumer>>) -> Result<()> {
    let Some(held) = lock(home)? else {
        return Ok(());
    };
    run_holding(
        home,
        idle_ms,
        consumers,
        || {},
        Some(held),
        Phases::default(),
    )
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
        // Without the entries turned off: none of the chains below calls them.
        let loaded = crate::config::load_chain(&home).and_then(|cfg| {
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
                           check: &crate::provider::AnswerCheck,
                           gate: &crate::provider::Gate| {
            crate::provider::Chain::new(&cfg.providers, db)
                .paid_cap(cfg.paid_usd_per_month)
                .check(check)
                .gate(gate)
                .run("curator", span, prompt, &crate::curate::schema())
        };
        // Who is asked and within what caps: a window held under other ones is tried again now.
        // That includes the key an entry's budget comes from (#238): another has its own limit.
        let chain = format!(
            "{:?} {} {:?}",
            cfg.providers,
            cfg.paid_usd_per_month,
            crate::provider::budget_keys(&cfg.providers)
        );
        let windows =
            crate::curate::run_phase(raw, k, db, &rules, &cfg.summary, &chain, &mut curator)?;
        // The same chain, as the summary role (docs/summaries.md T1), in the digest's place: spec
        // 1.4 lets each role have its own, and one list serves until measurement asks for two.
        let mut summarizer = |span: &str,
                              prompt: &str,
                              check: &crate::provider::AnswerCheck,
                              gate: &crate::provider::Gate| {
            crate::provider::Chain::new(&cfg.providers, db)
                .paid_cap(cfg.paid_usd_per_month)
                .check(check)
                .gate(gate)
                .run("summary", span, prompt, &crate::turns::answer_schema())
        };
        crate::turns::phase(
            raw,
            k,
            db,
            &rules,
            &cfg.summary,
            &chain,
            &mut summarizer,
            windows,
        )
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
    lock_held(&home.join("state").join("worker.lock"))
}

/// Read-only status readers share an existing regular lock; only the exclusive producer
/// prevents them from taking it. Links and special files are never followed or waited on.
pub(crate) fn lock_held(path: &Path) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let mut file = std::fs::OpenOptions::new();
    file.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut file, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    file.open(path).is_ok_and(|file| {
        file.metadata().is_ok_and(|metadata| metadata.is_file())
            && matches!(
                try_lock_with(&file, std::fs::File::try_lock_shared),
                Err(std::fs::TryLockError::WouldBlock)
            )
    })
}

/// `File::try_lock`, which under `cargo test` waits up to 200 ms for a lock just released: a
/// child that another test thread forked holds the open file until it execs, so a lock a test
/// dropped can still be held for a moment (about one CI run in twelve failed on it). A thread that
/// contends for the lock on purpose (`contending`) gets its answer at once. No oboete process
/// forks from another thread while it takes or releases the lock, so outside tests nothing waits.
pub(crate) fn try_lock(f: &std::fs::File) -> Result<(), std::fs::TryLockError> {
    try_lock_with(f, std::fs::File::try_lock)
}

fn try_lock_with(
    f: &std::fs::File,
    attempt: fn(&std::fs::File) -> Result<(), std::fs::TryLockError>,
) -> Result<(), std::fs::TryLockError> {
    #[cfg(test)]
    if !CONTENDING.get() {
        for _ in 0..20 {
            match attempt(f) {
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                tried => return tried,
            }
        }
    }
    attempt(f)
}

#[cfg(test)]
thread_local! {
    static CONTENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// For a test thread that contends for the worker lock with other threads on purpose: until the
/// guard is dropped, the lock is taken or found held at once, so the threads still overlap.
#[cfg(test)]
pub(crate) fn contending() -> impl Drop {
    struct Contending;
    impl Drop for Contending {
        fn drop(&mut self) {
            CONTENDING.set(false);
        }
    }
    CONTENDING.set(true);
    Contending
}

/// `oboete rebuild` (spec 1.7): under the worker lock, knowledge.db is moved aside and every
/// consumer runs from zero over raw.db and the op log, with no curation phase, so no provider is
/// called. The old file is removed once the new one is complete; a rebuild that fails keeps it,
/// named in the error.
pub(crate) fn preview_rebuild(home: &Path) -> Result<crate::backup::MaintenancePreview> {
    crate::backup::preview_maintenance(home, crate::backup::MaintenanceOperation::Rebuild)
}
fn check_preview(
    home: &Path,
    operation: crate::backup::MaintenanceOperation,
    expected: Option<&str>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let shown = match operation {
        crate::backup::MaintenanceOperation::Rebuild => preview_rebuild(home),
        crate::backup::MaintenanceOperation::Restore => crate::backup::preview_restore(home),
    }
    .map_err(|cause| cause.context(crate::backup::MaintenanceCode::Stale))?;
    anyhow::ensure!(shown.key == expected, crate::backup::MaintenanceCode::Stale);
    Ok(())
}

pub fn rebuild(home: &Path) -> Result<()> {
    rebuild_report(
        home,
        None,
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .map(|_| ())
    .map_err(MaintenanceFailure::into_cause)
}

pub(crate) fn rebuild_report(
    home: &Path,
    expected: Option<&str>,
    caller: crate::executable::CommandCaller,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> std::result::Result<MaintenanceOutcome, MaintenanceFailure> {
    use crate::backup::{MaintenanceCode as Code, MaintenanceOperation as Operation};
    let mut outcome = MaintenanceOutcome::new(Operation::Rebuild);
    let mut code = Code::Stale;
    let result = (|| {
        code = Code::Busy;
        let command = crate::executable::CommandHome::new(home, caller)?;
        code = Code::Stale;
        check_preview(home, Operation::Rebuild, expected)?;
        code = Code::Busy;
        let held = lock_asking_for(home, Some(&command))?;
        let proof = held.2.clone().expect("reported command retains its home");
        proof.check(home)?;
        let _config = crate::settings::config_lock(home)?;
        code = Code::Stale;
        let consent = crate::backup::consent(home, Operation::Rebuild, expected)?;
        code = Code::Busy;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let _rebuilding = options.open(home.join("state/rebuild.lock"))?;
        let start = Instant::now();
        loop {
            match _rebuilding.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock)
                    if start.elapsed() < Duration::from_millis(200) =>
                {
                    #[cfg(test)]
                    if let Some(release) =
                        REBUILD_STATUS_BLOCKED.with(|hook| hook.borrow_mut().take())
                    {
                        release();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    anyhow::bail!("the rebuild status lock is in use")
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        code = Code::Failed;
        let name = crate::backup::fresh_name(home, "knowledge.db.rebuilding-")?;
        let kept = home.join(&name);
        let rebuilding = (|| {
            set_aside_report(
                home,
                &name,
                consent.as_ref(),
                Some(&proof),
                &mut outcome.effects,
                &mut |stage| committed(&MaintenanceCommit::Effect { stage }),
            )?;
            proof.check(home)?;
            outcome.old_knowledge_kept = kept.exists();
            if kept.exists() {
                let carried =
                    crate::knowledge::open(home).and_then(|k| crate::embed_phase::carry(&k, &kept));
                #[cfg(test)]
                if let Some(after) = AFTER_CARRY.with(|hook| hook.borrow_mut().take()) {
                    after();
                }
                match carried {
                    Ok(count) => {
                        outcome.cached_vectors_carried = count as u64;
                        crate::backup::effect("vectors_carried", &mut |stage| {
                            committed(&MaintenanceCommit::Effect { stage })
                        })?;
                    }
                    Err(error) => {
                        match put_back(home, &name, &proof) {
                            Ok(()) => {
                                outcome.effects.knowledge_put_back = true;
                                outcome.old_knowledge_kept = false;
                                crate::backup::effect("knowledge_put_back", &mut |stage| {
                                    committed(&MaintenanceCommit::Effect { stage })
                                })?;
                            }
                            Err(back) => {
                                return Err(error)
                                    .context(format!("rebuild put-back also failed: {back:#}"));
                            }
                        }
                        return Err(error).context("rebuild: the vectors of knowledge.db could not be read; nothing was changed");
                    }
                }
            }
            let (index, result) = run_once_holding_report(home, held, committed);
            outcome.index = index;
            result.with_context(|| {
                if kept.exists() {
                    format!(
                        "rebuild; the old knowledge.db is kept as {}",
                        kept.display()
                    )
                } else {
                    "rebuild".into()
                }
            })?;
            for ext in ["", "-wal", "-shm"] {
                proof.check(home)?;
                let file = home.join(format!("{name}{ext}"));
                if file.exists()
                    && let Err(error) = crate::backup::fail_after("old_cleanup")
                        .and_then(|()| std::fs::remove_file(&file).map_err(anyhow::Error::from))
                {
                    outcome.cleanup_warnings += 1;
                    eprintln!(
                        "oboete: rebuilt; {} is left ({error}): delete it by hand",
                        file.display()
                    );
                }
            }
            Ok(())
        })();
        outcome.old_knowledge_kept = if proof.check(home).is_ok() {
            kept.exists()
        } else {
            outcome.effects.knowledge_files_moved != 0 && !outcome.effects.knowledge_put_back
        };
        rebuilding
    })();
    match result {
        Ok(()) => Ok(outcome),
        Err(cause) => Err(maintenance_failure(outcome, cause, code)),
    }
}

/// The CLI's complete restore: one admission, mandatory drain even on restore failure.
pub fn restore(home: &Path) -> Result<String> {
    restore_report(
        home,
        None,
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .map(|outcome| {
        outcome
            .restore
            .expect("successful restore has a receipt")
            .note
    })
    .map_err(MaintenanceFailure::into_cause)
}
pub(crate) fn restore_report(
    home: &Path,
    expected: Option<&str>,
    caller: crate::executable::CommandCaller,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> std::result::Result<MaintenanceOutcome, MaintenanceFailure> {
    use crate::backup::{MaintenanceCode as Code, MaintenanceOperation as Operation};
    let mut outcome = MaintenanceOutcome::new(Operation::Restore);
    let command = crate::executable::CommandHome::new(home, caller)
        .map_err(|cause| maintenance_failure(outcome.clone(), cause.into(), Code::Busy))?;
    if let Err(cause) = check_preview(home, Operation::Restore, expected) {
        return Err(maintenance_failure(outcome, cause, Code::Stale));
    }
    let held = lock_asking_for(home, Some(&command))
        .map_err(|cause| maintenance_failure(outcome.clone(), cause, Code::Busy))?;
    let _config = crate::settings::config_lock(home)
        .map_err(|cause| maintenance_failure(outcome.clone(), cause.into(), Code::Busy))?;
    let restored =
        crate::backup::restore_report_holding(home, expected, held.2.as_ref(), &mut |stage| {
            committed(&MaintenanceCommit::Effect { stage })
        });
    let failure = match restored {
        Ok(receipt) => {
            outcome.restore = Some(receipt);
            None
        }
        Err(failure) => {
            outcome.restore = Some(*failure.receipt);
            Some((failure.code, failure.cause))
        }
    };
    // A stale confirmation never admits a drain's writes. Ordinary native restore errors keep
    // the CLI guarantee: hooks that appended under the admission are indexed before release.
    if let Some((Code::Stale, cause)) = failure {
        return Err(maintenance_failure(outcome, cause, Code::Stale));
    }
    let staged = outcome
        .restore
        .as_ref()
        .filter(|r| r.effects.staging_complete)
        .map(|_| crate::db::store_file(&home.join("raw.db.restored")))
        .filter(|identity| !identity.is_empty());
    let (index, ran) = run_once_holding_report(home, held, committed);
    if staged.is_some()
        && staged.as_deref() == Some(crate::db::store_file(&home.join("raw.db")).as_str())
        && index
            .raw_recovery
            .as_ref()
            .is_some_and(|r| r.effects.stopped_restore_finished)
        && let Some(restored) = outcome.restore.as_mut()
    {
        restored.effects.raw_swapped = true;
        restored.effects.stopped_restore_finished = true;
    }
    outcome.index = index;
    match (failure, ran) {
        (None, Ok(())) => Ok(outcome),
        (Some((code, cause)), ran) => Err(MaintenanceFailure {
            outcome: Box::new(outcome),
            code,
            cause,
            index_cause: ran.err(),
        }),
        (None, Err(cause)) => Err(maintenance_failure(outcome, cause, Code::Failed)),
    }
}

/// knowledge.db moved aside as `name`, its sidecars with it under the names SQLite looks for
/// beside that file (`...-wal`, `...-shm`), so the kept file opens with its last commits. Under
/// raw.lock held exclusively, as a restore moves it: every reader of knowledge.db holds raw.db
/// open (a shared hold) while it reads. The sidecars first: never the file's name free with an
/// old WAL beside it that SQLite would replay into the new file. A move that fails puts back the
/// ones before it, so the file never stays without its WAL.
#[cfg(test)]
fn set_aside(home: &Path, name: &str) -> Result<Vec<std::path::PathBuf>> {
    set_aside_report(
        home,
        name,
        None,
        None,
        &mut crate::backup::Effects::default(),
        &mut |_| {},
    )
}
fn set_aside_report(
    home: &Path,
    name: &str,
    consent: Option<&crate::backup::Consent>,
    guard: Option<&crate::executable::CommandHome>,
    effects: &mut crate::backup::Effects,
    committed: &mut impl FnMut(&'static str),
) -> Result<Vec<std::path::PathBuf>> {
    use anyhow::Context;
    crate::backup::before_swap();
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    if let Some(consent) = consent {
        consent.check_logs()?;
    }
    let _swap = crate::raw::lock_for_swap(home)?;
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    if let Some(consent) = consent {
        consent.check_locked(home)?;
    }
    for ext in ["", "-wal", "-shm"] {
        anyhow::ensure!(
            std::fs::symlink_metadata(home.join(format!("{name}{ext}")))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "rebuild recovery target exists or is unreadable"
        );
    }
    // Names joined to `home`, never through its display form: a home path need not be UTF-8.
    let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    for ext in ["-wal", "-shm", ""] {
        let from = home.join(format!("knowledge.db{ext}"));
        if !from.exists() {
            continue;
        }
        let to = home.join(format!("{name}{ext}"));
        let result = std::fs::rename(&from, &to)
            .map_err(anyhow::Error::from)
            .and_then(|()| {
                moved.push((from.clone(), to.clone()));
                effects.knowledge_files_moved += 1;
                crate::backup::effect("knowledge_set_aside", committed)
            });
        if let Err(e) = result {
            // One that cannot go back is named, so the owner can put it back by hand.
            let mut why = format!("move {}", from.display());
            for (from, to) in moved.iter().rev() {
                if let Err(back) = std::fs::rename(to, from) {
                    why += &format!("; {} stays as {} ({back})", from.display(), to.display());
                }
            }
            effects.knowledge_put_back =
                moved.iter().all(|(from, to)| from.exists() && !to.exists());
            if effects.knowledge_put_back && !moved.is_empty() {
                committed("knowledge_put_back");
            }
            return Err(e).context(why);
        }
    }
    Ok(moved.into_iter().map(|(_, to)| to).collect())
}

/// `set_aside` undone: the new knowledge.db removed, and the one set aside as `name` back, the
/// file before its sidecars, under raw.lock as it was moved.
fn put_back(home: &Path, name: &str, guard: &crate::executable::CommandHome) -> Result<()> {
    use anyhow::Context;
    guard.check(home)?;
    let _swap = crate::raw::lock_for_swap(home)?;
    guard.check(home)?;
    for ext in ["", "-wal", "-shm"] {
        guard.check(home)?;
        let new = home.join(format!("knowledge.db{ext}"));
        if new.exists() {
            std::fs::remove_file(&new).with_context(|| format!("remove {}", new.display()))?;
        }
    }
    for ext in ["", "-wal", "-shm"] {
        guard.check(home)?;
        let kept = home.join(format!("{name}{ext}"));
        if kept.exists() {
            std::fs::rename(&kept, home.join(format!("knowledge.db{ext}")))
                .with_context(|| format!("put {} back", kept.display()))?;
        }
    }
    Ok(())
}

/// One run now, for `oboete restore` and tests. It waits up to 2 s for the lock rather than
/// return at once: a worker a hook started holds it only while it drains, and a lock just
/// released can still be held for a moment by a child another thread forked (it keeps the open
/// file until it execs), which made hook tests run nothing under a parallel suite.
#[allow(dead_code)] // Task 12's replay drains without waiting.
/// `run_once` under a lock the caller holds, which it releases when the consumers have run.
pub fn run_once_holding(home: &Path, held: Lock) -> Result<()> {
    run_holding(
        home,
        0,
        consumers(home),
        || {},
        Some(held),
        Phases::default(),
    )
}
fn run_once_holding_report(
    home: &Path,
    held: Lock,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> (IndexReceipt, Result<()>) {
    let mut receipt = IndexReceipt::default();
    let mut report = IndexReport {
        receipt: &mut receipt,
        committed,
    };
    let result = run_holding_report(
        home,
        0,
        consumers(home),
        || {},
        Some(held),
        Phases::default(),
        &mut Some(&mut report),
    );
    receipt.state = if result.is_ok() {
        IndexState::Complete
    } else {
        IndexState::Failed
    };
    (receipt, result)
}

pub fn run_once(home: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut held = lock(home)?;
    while held.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        held = lock(home)?;
    }
    run_holding(home, 0, consumers(home), || {}, held, Phases::default())
}

#[cfg(test)]
mod tests {
    /// A worker's phases with only `p`, the curation phase.
    fn curating<'a, 'f>(p: &'a mut CurationPhase<'f>) -> Phases<'a, 'f> {
        Phases {
            curation: Some(p),
            ..Phases::default()
        }
    }

    use super::*;
    use crate::knowledge;
    use crate::raw;

    #[test]
    fn w5b_a_failed_store_inspection_keeps_the_restore_request() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(raw::open(p).unwrap());
        std::fs::create_dir(p.join("knowledge.db")).unwrap();
        crate::backup::request_restore(p);
        let failure = restore_report(
            p,
            None,
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(failure.outcome.index.state, IndexState::Failed);
        assert!(failure.index_cause.is_some());
        assert!(
            crate::backup::restore_requested(p),
            "inspection lost the pending restore request"
        );
    }

    #[cfg(unix)]
    #[test]
    fn w5b_a_rejected_home_does_not_receive_a_restore_retry() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("memory");
        let replacement = home.with_extension("replacement");
        std::fs::create_dir(&home).unwrap();
        let mut raw = raw::open(&home).unwrap();
        raw.append(&raw::test_event("synthetic restore retry"))
            .unwrap();
        let wal = std::fs::read(home.join("raw.db-wal")).unwrap();
        drop(raw);
        std::fs::write(home.join("raw.db-wal"), wal).unwrap();
        crate::backup::request_restore(&home);
        std::fs::create_dir_all(replacement.join("state")).unwrap();
        std::fs::write(replacement.join("state/worker.lock"), "other home").unwrap();
        std::fs::write(replacement.join("canary"), "synthetic unrelated home").unwrap();
        BEFORE_WAL_HASH.set(Some(|path| {
            let home = path.parent().unwrap().to_owned();
            let held = raw::lock_for_swap(&home).unwrap();
            raw::SWAP_BLOCKED.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    std::fs::rename(&home, home.with_extension("retired")).unwrap();
                    std::fs::rename(home.with_extension("replacement"), &home).unwrap();
                    drop(held);
                }));
            });
        }));
        let failure = restore_report(
            &home,
            None,
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        BEFORE_WAL_HASH.set(None);
        assert_eq!(failure.outcome.index.state, IndexState::Failed);
        assert!(
            !crate::backup::restore_requested(&home),
            "the rejected home got a retry marker"
        );
        assert!(!home.join("raw.lock").exists());
        assert!(!home.join("raw.db").exists());
        assert_eq!(
            std::fs::read(home.join("canary")).unwrap(),
            b"synthetic unrelated home"
        );
    }

    #[test]
    fn w5b_consumer_progress_counts_durable_changes_and_not_empty_transactions() {
        struct ReceiptConsumer(&'static str);
        impl Consumer for ReceiptConsumer {
            fn name(&self) -> &'static str {
                "receipt-fixture"
            }
            fn top(&self, _: &Raw, _: &str) -> Result<i64> {
                Ok(i64::from(self.0 == "checkpoint"))
            }
            fn step(&mut self, _: &Raw, k: &Connection, _: &str, after: i64) -> Result<i64> {
                match self.0 {
                    "checkpoint" => return Ok(1),
                    "row" => {
                        k.execute("INSERT INTO receipt_rows VALUES(73)", [])?;
                    }
                    "schema" => {
                        k.execute("CREATE TABLE receipt_schema(value)", [])?;
                    }
                    _ => {}
                }
                Ok(after)
            }
            fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
                Ok(())
            }
        }
        for (mode, expected) in [("noop", 0), ("checkpoint", 1), ("row", 1), ("schema", 1)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            raw::open(p)
                .unwrap()
                .append(&raw::test_event("synthetic receipt checkpoint"))
                .unwrap();
            knowledge::open(p)
                .unwrap()
                .execute("CREATE TABLE receipt_rows(value)", [])
                .unwrap();
            let mut receipt = IndexReceipt::default();
            let mut commits = Vec::new();
            {
                let mut callback = |event: &MaintenanceCommit| commits.push(event.clone());
                let mut report = IndexReport {
                    receipt: &mut receipt,
                    committed: &mut callback,
                };
                run_holding_report(
                    p,
                    0,
                    vec![Box::new(ReceiptConsumer(mode))],
                    || {},
                    None,
                    Phases::default(),
                    &mut Some(&mut report),
                )
                .unwrap();
            }
            assert_eq!(receipt.consumer_commits, expected, "{mode}");
            assert_eq!(
                commits
                    .iter()
                    .filter(|event| matches!(event, MaintenanceCommit::Index { .. }))
                    .count(),
                expected as usize,
                "{mode}"
            );
            let k = knowledge::open(p).unwrap();
            if mode == "checkpoint" {
                let raw = raw::open(p).unwrap();
                assert_eq!(
                    checkpoint::get_in(&k, checkpoint::SEQS, "receipt-fixture", raw.device())
                        .unwrap(),
                    1
                );
            } else if mode == "row" {
                assert_eq!(
                    k.query_row("SELECT value FROM receipt_rows", [], |r| r.get::<_, i64>(0))
                        .unwrap(),
                    73
                );
            } else if mode == "schema" {
                assert_eq!(
                    k.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='receipt_schema'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
            }
        }
    }

    #[test]
    fn w5b_a_disappearing_wal_does_not_abort_the_required_restore_drain() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("synthetic disappearing WAL"))
            .unwrap();
        let wal = std::fs::read(p.join("raw.db-wal")).unwrap();
        assert!(wal.len() > 32);
        drop(raw);
        std::fs::write(p.join("raw.db-wal"), wal).unwrap();
        BEFORE_WAL_HASH.set(Some(|path| std::fs::remove_file(path).unwrap()));
        let failure = restore_report(
            p,
            None,
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        BEFORE_WAL_HASH.set(None);
        assert_eq!(failure.outcome.index.state, IndexState::Complete);
        assert!(failure.index_cause.is_none());
        assert_eq!(
            crate::search::raw(p, "disappearing WAL", None, 5)
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn w5b_viewer_commands_require_a_readable_original_view_lock() {
        for operation in ["rebuild", "restore"] {
            for unusable in ["missing", "directory", "unreadable"] {
                let home = tempfile::tempdir().unwrap();
                let p = home.path();
                drop(raw::open(p).unwrap());
                run_once(p).unwrap();
                let lock = p.join("state/view.lock");
                let viewer = std::fs::File::create(&lock).unwrap();
                let identity = file_id(viewer.metadata()).unwrap();
                if unusable != "unreadable" {
                    std::fs::remove_file(&lock).unwrap();
                }
                if unusable == "directory" {
                    std::fs::create_dir(&lock).unwrap();
                } else if unusable == "unreadable" {
                    std::fs::write(&lock, "").unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o0))
                            .unwrap();
                    }
                }
                let generation = std::fs::read(p.join("state/worker-gen")).unwrap();
                let result = if operation == "rebuild" {
                    rebuild_report(
                        p,
                        None,
                        crate::executable::CommandCaller::Viewer(identity),
                        &mut |_| {},
                    )
                } else {
                    restore_report(
                        p,
                        None,
                        crate::executable::CommandCaller::Viewer(identity),
                        &mut |_| {},
                    )
                };
                let failure = result.expect_err("viewer without its home proof was admitted");
                assert!(!failure.outcome.committed(), "{operation}: {unusable}");
                assert_eq!(
                    std::fs::read(p.join("state/worker-gen")).unwrap(),
                    generation,
                    "{operation}: {unusable} borrowed the worker lock"
                );
            }
        }
    }

    // This R3/R9 proof needs Unix device/inode identity and renaming a home with open locks.
    // Windows rejects that fixture's rename before the rollback guard is reached.
    #[cfg(unix)]
    #[test]
    fn w5b_a_replaced_home_is_not_touched_by_a_failed_carry_put_back() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("memory");
        let retired = root.path().join("retired");
        std::fs::create_dir(&home).unwrap();
        drop(raw::open(&home).unwrap());
        std::fs::write(home.join("knowledge.db"), "synthetic unreadable old cache").unwrap();
        let shown = preview_rebuild(&home).unwrap();
        let replacement = home.clone();
        let kept_home = retired.clone();
        AFTER_CARRY.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                std::fs::rename(&replacement, &kept_home).unwrap();
                std::fs::create_dir_all(replacement.join("state")).unwrap();
                std::fs::write(replacement.join("state/worker.lock"), "different home").unwrap();
                let knowledge = knowledge::open(&replacement).unwrap();
                knowledge
                    .execute_batch(
                        "CREATE TABLE unrelated(value); INSERT INTO unrelated VALUES(73)",
                    )
                    .unwrap();
            }))
        });
        let failed = rebuild_report(
            &home,
            Some(&shown.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            home.join("knowledge.db").exists(),
            "rollback deleted the replacement home's unrelated store"
        );
        assert!(!home.join("raw.lock").exists());
        let replacement = Connection::open_with_flags(
            home.join("knowledge.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            replacement
                .query_row("SELECT value FROM unrelated", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            73
        );
        assert!(!failed.outcome.effects.knowledge_put_back);
        assert!(failed.outcome.old_knowledge_kept);
        assert!(std::fs::read_dir(&retired).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("knowledge.db.rebuilding-")
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5b_exec_viewer_probe() {
        let Some(home) = std::env::var_os("OBOETE_TEST_MAINT_HOME") else {
            return;
        };
        let home = std::path::PathBuf::from(home);
        let original = crate::executable::expected(Some(crate::executable::Role::Viewer)).unwrap();
        assert!(original.is_some());
        assert!(crate::executable::expected(Some(crate::executable::Role::Worker)).is_err());
        let shown = preview_rebuild(&home).unwrap();
        let result = rebuild_report(
            &home,
            Some(&shown.key),
            crate::executable::CommandCaller::Viewer(original.unwrap()),
            &mut |_| {},
        );
        assert!(result.is_ok(), "borrowed viewer command: {result:?}");
        assert_eq!(result.unwrap().index.state, IndexState::Complete);
        assert_eq!(
            crate::executable::expected(Some(crate::executable::Role::Viewer)).unwrap(),
            original
        );
        let shown = preview_rebuild(&home).unwrap();
        AFTER_OPEN.set(Some(|home| {
            std::fs::remove_file(home.join("state/view.lock")).unwrap();
            std::fs::write(home.join("state/view.lock"), "new viewer home authority").unwrap();
        }));
        let failed = rebuild_report(
            &home,
            Some(&shown.key),
            crate::executable::CommandCaller::Viewer(original.unwrap()),
            &mut |_| {},
        )
        .unwrap_err();
        AFTER_OPEN.set(None);
        assert!(!failed.outcome.committed());
        assert!(crate::executable::expected(Some(crate::executable::Role::Worker)).is_err());
        assert_eq!(
            crate::executable::expected(Some(crate::executable::Role::Viewer)).unwrap(),
            original
        );
        assert!(!std::fs::read_dir(&home).unwrap().flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("knowledge.db.rebuilding-")
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5b_a_real_exec_viewer_can_borrow_worker_authority_and_keeps_both_home_checks() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("synthetic viewer maintenance"))
            .unwrap();
        run_once(p).unwrap();
        let viewer = std::fs::File::create(p.join("state/view.lock")).unwrap();
        let (device, inode) = file_id(viewer.metadata()).unwrap();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "export OBOETE_EXEC_HOME=\"$$:view.lock:$1:$2\"; exec \"$3\" --exact worker::tests::w5b_exec_viewer_probe --nocapture", "private-viewer-fixture"])
            .arg(device.to_string()).arg(inode.to_string()).arg(std::env::current_exe().unwrap())
            .env("OBOETE_TEST_MAINT_HOME", p).stdin(std::process::Stdio::null());
        for (key, _) in std::env::vars_os() {
            if ["KEY", "TOKEN", "SECRET", "PASSWORD"]
                .iter()
                .any(|part| key.to_string_lossy().to_ascii_uppercase().contains(part))
            {
                command.env_remove(key);
            }
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn w5b_a_failed_native_restore_still_drains_and_reports_store_commits() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("synthetic pending FTS record"))
            .unwrap();
        let mut commits = Vec::new();
        let failure = restore_report(
            p,
            None,
            crate::executable::CommandCaller::Worker,
            &mut |event| commits.push(event.clone()),
        )
        .unwrap_err();
        assert!(
            failure
                .cause
                .to_string()
                .contains("no usable backup segment")
        );
        assert_eq!(failure.outcome.index.state, IndexState::Complete);
        assert!(failure.outcome.index.stores_changed);
        assert!(failure.outcome.committed());
        assert!(failure.outcome.index.consumer_commits > 0);
        assert!(commits.iter().any(|event| matches!(
            event,
            MaintenanceCommit::Index {
                consumer: "fts",
                unit: CheckpointUnit::Records,
                checkpoint: 1
            }
        )));
        assert_eq!(
            crate::search::raw(p, "pending FTS", None, 5).unwrap().len(),
            1
        );
        assert!(!p.join("providers.db").exists());
        assert!(!failure.outcome.hybrid_ready);
    }

    #[test]
    fn w5b_restore_preserves_both_native_failures_and_committed_consumer_progress() {
        let home = tempfile::tempdir().unwrap();
        raw::open(home.path())
            .unwrap()
            .append(&raw::test_event("synthetic dual failure"))
            .unwrap();
        crate::backup::FAIL_AFTER.set(Some("index_commit"));
        let failure = restore_report(
            home.path(),
            None,
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        crate::backup::FAIL_AFTER.set(None);
        assert!(
            failure
                .cause
                .to_string()
                .contains("no usable backup segment")
        );
        assert!(
            failure
                .index_cause
                .as_ref()
                .unwrap()
                .to_string()
                .contains("index_commit")
        );
        assert_eq!(failure.outcome.index.state, IndexState::Failed);
        assert_eq!(failure.outcome.index.consumer_commits, 1);
        assert!(failure.outcome.committed());
    }

    #[test]
    fn w5b_a_zero_payload_rebuild_retains_carry_and_cleanup_failure_receipts() {
        for stage in ["knowledge_set_aside", "vectors_carried", "old_cleanup"] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            drop(raw::open(p).unwrap());
            drop(knowledge::open(p).unwrap());
            let shown = preview_rebuild(p).unwrap();
            crate::backup::FAIL_AFTER.set(Some(stage));
            let result = rebuild_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |_| {},
            );
            crate::backup::FAIL_AFTER.set(None);
            let outcome = if stage == "old_cleanup" {
                result.unwrap()
            } else {
                *result.unwrap_err().outcome
            };
            assert!(outcome.committed(), "zero payload {stage}");
            assert_eq!(outcome.cached_vectors_carried, 0);
            assert!(!outcome.hybrid_ready);
            match stage {
                "knowledge_set_aside" => assert!(outcome.effects.knowledge_put_back),
                "vectors_carried" => assert!(outcome.old_knowledge_kept),
                "old_cleanup" => {
                    assert_eq!(outcome.cleanup_warnings, 1);
                    assert_eq!(outcome.index.state, IndexState::Complete);
                    assert!(outcome.old_knowledge_kept);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn w5b_source_change_while_worker_admission_runs_refuses_before_rebuild() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("synthetic admission fixture"))
            .unwrap();
        drop(knowledge::open(p).unwrap());
        let shown = preview_rebuild(p).unwrap();
        AFTER_OPEN.set(Some(|home| {
            std::fs::write(
                home.join("config.toml"),
                "[capture]\nstore_prompts = false\n",
            )
            .unwrap()
        }));
        let failed = rebuild_report(
            p,
            Some(&shown.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        AFTER_OPEN.set(None);
        assert_eq!(failed.code, crate::backup::MaintenanceCode::Stale);
        assert!(!failed.outcome.committed());
        assert!(p.join("knowledge.db").exists());
        assert!(!std::fs::read_dir(p).unwrap().flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("knowledge.db.rebuilding-")
        }));
    }

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
        fn reads_ops(&self) -> bool {
            true
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
            curating(p),
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
        run_holding(
            home.path(),
            0,
            vec![Box::new(Seen)],
            || {},
            None,
            curating(p),
        )
        .unwrap();
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
        assert!(
            p.reason.contains("spent: 0/0 calls in 24 hours"),
            "{}",
            p.reason
        );
    }

    /// #94: the worker's chain leaves out an entry `[chain] off` turns off, and a window held under
    /// that chain is tried again once the entry is on.
    #[test]
    fn the_workers_chain_leaves_out_an_entry_turned_off() {
        let home = tempfile::tempdir().unwrap();
        let device = {
            let mut raw = raw::open(home.path()).unwrap();
            raw.append(&raw::test_event("a")).unwrap();
            raw.device().to_owned()
        };
        let config = |off: &str| {
            let entry = |name: &str| {
                format!(
                    "[[providers]]\nkind = \"openai\"\nname = \"{name}\"\n\
                     base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\ndaily_budget = 0\n"
                )
            };
            let text = format!(
                "[summary]\ncurate = true\n{}{}[chain]\noff = [{off}]\n",
                entry("first"),
                entry("second")
            );
            std::fs::write(home.path().join("config.toml"), text).unwrap();
        };
        let held = || {
            let db = crate::providers_db::open(home.path()).unwrap();
            let p = crate::providers_db::pending_of(&db, &device).unwrap();
            p.expect("the phase did not run").reason
        };
        config("\"first\"");
        run(home.path(), 0).unwrap();
        let without = held();
        assert!(
            without.contains("second: 0/0") && !without.contains("first"),
            "{without}"
        );
        config("");
        run(home.path(), 0).unwrap();
        let with = held();
        assert!(
            with.contains("first: 0/0") && with.contains("second: 0/0"),
            "{with}"
        );
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

    /// Milestone 5 F2: the zero-length idle wait still checks which raw.db it holds before
    /// exporting it, and opens the replacement to reconcile forgets and drain new records.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_nonresident_worker_reopens_a_replacement_before_its_final_backup() {
        replacement_at_idle_exit(true);
    }

    /// The same check is needed after releasing the lock: a replacement in the exit seam has
    /// no new records in the old connection to wake the worker through `behind`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_nonresident_worker_reopens_a_replacement_while_it_decides_to_exit() {
        replacement_at_idle_exit(false);
    }

    /// The held file must not become the replacement pathname while SQLite still holds the
    /// file it opened: its new input must be consumed and its forget logs reconciled.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_replacement_after_sqlite_open_is_not_adopted_as_the_held_file() {
        replacement_after_open(false);
    }

    /// An absent path creates a store, but a replacement before its first identity capture
    /// must still be opened rather than leave its new input unconsumed.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_fresh_raw_replaced_after_open_is_read_before_the_worker_exits() {
        replacement_after_open(true);
    }

    #[cfg(target_os = "linux")]
    fn pending_forget_copy(p: &Path) -> (String, i64) {
        let mut raw = raw::open(p).unwrap();
        let mut event = raw::test_event("opened-file-forget-canary-8351");
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "opened-file-import"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        let device = raw.device().to_owned();
        drop(raw);
        std::fs::copy(p.join("raw.db"), p.join("raw-before-forget.db")).unwrap();
        let preview = crate::forget::preview(
            p,
            crate::forget::Target::Record {
                device: device.clone(),
                seq,
            },
        )
        .unwrap();
        crate::forget::start(p, &preview).unwrap();
        (device, seq)
    }

    #[cfg(target_os = "linux")]
    fn replace_raw_at_boundary(p: &Path) {
        let old = p.join("raw-before-forget.db");
        if !old.exists() {
            // The fresh-open case copies the just-created schema, including its WAL commits.
            Connection::open(p.join("raw.db"))
                .unwrap()
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            std::fs::copy(p.join("raw.db"), &old).unwrap();
        }
        let staged = p.join("raw.db.copy");
        std::fs::copy(old, &staged).unwrap();
        for file in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(file));
        }
        std::fs::rename(staged, p.join("raw.db")).unwrap();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("opened-file-new-input-8352"))
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    fn replacement_after_open(fresh: bool) {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let forgotten = (!fresh).then(|| pending_forget_copy(p));
        AFTER_RAW_OPEN.set(Some(replace_raw_at_boundary));
        run_with(p, 0, vec![Box::new(Seen)], || {}).unwrap();
        let raw = raw::open(p).unwrap();
        let k = knowledge::open(p).unwrap();
        assert_eq!(
            k.query_row(
                "SELECT COUNT(*) FROM seen WHERE device=?1 AND seq=1",
                [raw.device()],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            1,
            "the replacement was adopted as held while its new input was never read"
        );
        if let Some((device, seq)) = forgotten {
            assert!(matches!(
                raw.after(&device, seq - 1, 1).unwrap()[0].item,
                raw::Item::Removed
            ));
        }
    }

    /// A copy put back after the previous round's final check must be reconciled before the
    /// next consumer sees a raw handle, even when another phase immediately starts that round.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_replacement_at_round_entry_is_opened_before_the_first_consumer() {
        struct CurrentFile(std::path::PathBuf);
        impl Consumer for CurrentFile {
            fn name(&self) -> &'static str {
                "current-file"
            }
            fn step(&mut self, raw: &Raw, _: &Connection, _: &str, _: i64) -> Result<i64> {
                assert_eq!(
                    raw.device(),
                    raw::open(&self.0)?.device(),
                    "a new round sent the replaced file's old handle to a consumer"
                );
                raw.max_seq()
            }
            fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
                Ok(())
            }
        }
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let (device, seq) = pending_forget_copy(p);
        let mut first = true;
        let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
            if std::mem::take(&mut first) {
                BEFORE_ROUND.set(Some(replace_raw_at_boundary));
                Ok(Phase::Covered)
            } else {
                Ok(Phase::Idle)
            }
        };
        run_holding(
            p,
            0,
            vec![Box::new(CurrentFile(p.into()))],
            || {},
            None,
            curating(&mut phase),
        )
        .unwrap();
        assert!(matches!(
            raw::open(p).unwrap().after(&device, seq - 1, 1).unwrap()[0].item,
            raw::Item::Removed
        ));
    }

    /// A replacement during a batch cannot be exported through that batch's old connection
    /// at the scheduled backup boundary. The deadline is injected, never waited for.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_scheduled_backup_reopens_a_replacement_instead_of_exporting_the_old_file() {
        struct ReplacingBatch(std::path::PathBuf, bool);
        impl Consumer for ReplacingBatch {
            fn name(&self) -> &'static str {
                "replacing-batch"
            }
            fn step(&mut self, raw: &Raw, _: &Connection, _: &str, _: i64) -> Result<i64> {
                let through = raw.max_seq()?;
                if std::mem::take(&mut self.1) {
                    replace_raw_at_boundary(&self.0);
                }
                Ok(through)
            }
            fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
                Ok(())
            }
        }
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let (device, _) = pending_forget_copy(p);
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(ReplacingBatch(p.into(), true))];
        let mut holding = Holding::default();
        let mut deadline = Instant::now();
        assert!(
            serve(
                p,
                0,
                &mut consumers,
                (&mut holding, &mut None),
                &mut deadline,
                &mut || {},
                &mut Phases::default(),
            )
            .unwrap()
        );
        let stale = std::fs::read_dir(p.join("backups"))
            .unwrap()
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with(&device) && name.ends_with(".seg.zst")
            })
            .count();
        assert_eq!(stale, 0, "the scheduled backup exported the replaced file");
    }

    /// M5 F2: an embedding reply starts another round even when curation is idle. The next
    /// round must reconcile the replacement before a consumer or provider sees it.
    #[cfg(target_os = "linux")]
    #[test]
    fn embedding_progress_does_not_skip_reopening_raw_while_curation_is_idle() {
        let _contending = contending();
        let mut s = crate::search::b::fixture::Store::new();
        s.decided(
            "github.com/example/replacement",
            1_000,
            "The parser reads one line at a time.",
            &[],
        );
        s.run();
        let stub = crate::embed::stub::Stub::start();
        crate::embed_phase::fixture::config(&s, &stub);
        let p = s.home.path();
        let mut event = raw::test_event("embedding-reopen-forget-canary-6831");
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "embedding-reopen-import"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = s
            .raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                std::slice::from_ref(&identity),
                "",
                None,
            )
            .unwrap()[0];
        let device = s.raw.device().to_owned();
        let request = crate::forget::previous_transcript_request(&s.raw, seq, &identity);
        drop(s.raw);
        let old = p.join("raw-before-forget.db");
        std::fs::copy(p.join("raw.db"), &old).unwrap();
        let mut replay = raw::open(p).unwrap();
        assert_eq!(replay.forget_apply(&[request]).unwrap(), 1);
        crate::forget::reconcile(p, &mut replay).unwrap();
        let raw = raw::open(p).unwrap();
        let mut k = knowledge::open(p).unwrap();
        drain(&raw, &mut k, &mut consumers(p)).unwrap();
        let mut embed = crate::embed_phase::Phase::new(p);
        assert!(matches!(
            embed.poll(&raw, &k).unwrap(),
            Phase::Waiting { .. }
        ));
        until("the stub embedding reply", || embed.done());
        drop((raw, k));
        let mut calls = 0;
        let mut phase = |raw: &mut Raw, _: &Connection| -> Result<Phase> {
            calls += 1;
            if calls == 1 {
                for file in ["raw.db-wal", "raw.db-shm"] {
                    let _ = std::fs::remove_file(p.join(file));
                }
                let staged = p.join("raw.db.copy");
                std::fs::copy(&old, &staged).unwrap();
                std::fs::rename(staged, p.join("raw.db")).unwrap();
                std::fs::write(p.join("config.toml"), "[embedding]\nprovider = 'none'\n").unwrap();
            } else {
                assert_ne!(
                    raw.device(),
                    device,
                    "embedding progress started another round over the old raw.db"
                );
                assert!(matches!(
                    raw.after(&device, seq - 1, 1).unwrap()[0].item,
                    raw::Item::Removed
                ));
            }
            Ok(Phase::Idle)
        };
        let phases = Phases {
            embed: Some(&mut embed),
            curation: Some(&mut phase),
            ..Phases::default()
        };
        run_holding(p, 0, consumers(p), || {}, None, phases).unwrap();
        assert_eq!(calls, 2);
    }

    #[cfg(target_os = "linux")]
    fn replacement_at_idle_exit(before_release: bool) {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let mut event = raw::test_event("idle-exit-forget-canary-5821");
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "idle-exit-import"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        let device = raw.device().to_owned();
        drop(raw);
        let old = p.join("raw-before-forget.db");
        std::fs::copy(p.join("raw.db"), &old).unwrap();
        let preview = crate::forget::preview(
            p,
            crate::forget::Target::Record {
                device: device.clone(),
                seq,
            },
        )
        .unwrap();
        crate::forget::start(p, &preview).unwrap();
        // Both existing seams run synchronously: no timing or scheduling decides when the
        // replacement happens, and it happens only once even after the worker opens it again.
        let copied = std::cell::RefCell::new(None);
        let replace = || {
            if copied.borrow().is_some() {
                return;
            }
            for file in ["raw.db-wal", "raw.db-shm"] {
                let _ = std::fs::remove_file(p.join(file));
            }
            let staged = p.join("raw.db.copy");
            std::fs::copy(&old, &staged).unwrap();
            std::fs::rename(staged, p.join("raw.db")).unwrap();
            let mut raw = raw::open(p).unwrap();
            let fresh = raw
                .append(&raw::test_event("idle-exit-new-record-5822"))
                .unwrap();
            copied.replace(Some((raw.device().to_owned(), fresh)));
        };
        let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
            if before_release {
                replace();
            }
            Ok(Phase::Idle)
        };
        run_holding(
            p,
            0,
            vec![Box::new(Seen)],
            || {
                if before_release {
                    let stale = std::fs::read_dir(p.join("backups"))
                        .unwrap()
                        .flatten()
                        .any(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            name.starts_with(&device) && name.ends_with(".seg.zst")
                        });
                    assert!(!stale, "the replaced raw.db was exported before reopening");
                } else {
                    replace();
                }
            },
            None,
            curating(&mut phase),
        )
        .unwrap();
        let (copy, fresh) = copied.into_inner().unwrap();
        let k = knowledge::open(p).unwrap();
        assert_eq!(
            k.query_row(
                "SELECT COUNT(*) FROM seen WHERE device = ?1 AND seq = ?2",
                (&copy, fresh),
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            1,
            "the replacement's new record was skipped at idle exit"
        );
        let raw = raw::open(p).unwrap();
        assert!(matches!(
            raw.after(&device, seq - 1, 1).unwrap()[0].item,
            raw::Item::Removed
        ));
        assert!(lock(p).unwrap().is_some());
    }

    #[test]
    fn a_restore_asked_for_while_the_worker_waits_opens_the_stores_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let device = {
            let mut raw = raw::open(&p).unwrap();
            raw.append(&raw::test_event("first")).unwrap();
            raw.device().to_owned()
        };
        // A worker that only took the request on its way out would take it after its 10 s wait,
        // past the 8 s this test allows; a slow runner still gets its worker into the wait (a
        // Windows runner took over 1 s, #199).
        let worker = {
            let p = p.clone();
            std::thread::spawn(move || run_with(&p, 10_000, vec![Box::new(Seen)], || {}))
        };
        // Asked only once the worker has read the event: past its start, where a request would be
        // taken too, so only the wait can take it.
        let t = Instant::now();
        while knowledge::open(&p)
            .ok()
            .and_then(|k| checkpoint::get(&k, "seen", &device).ok())
            != Some(1)
        {
            assert!(
                t.elapsed() < Duration::from_secs(30),
                "the worker never read"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        crate::backup::request_restore(&p);
        let t = Instant::now();
        while crate::backup::restore_requested(&p) {
            assert!(
                t.elapsed() < Duration::from_secs(8),
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

    /// Forks can retain the dropped OFD until exec. Once every owned spawn round completes,
    /// the lock is strictly available again; no scheduler-duration promise is involved.
    #[cfg(unix)]
    #[test]
    fn a_lock_just_dropped_is_taken_again_while_other_threads_spawn() {
        const ISOLATED: &str = "OBOETE_TEST_LOCK_SPAWN_ROUNDS";
        if std::env::var_os(ISOLATED).is_none() {
            // Other libtest tests also fork. Isolate only this fixture, not the entire suite.
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "worker::tests::a_lock_just_dropped_is_taken_again_while_other_threads_spawn",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env_clear()
                .envs(std::env::vars_os().filter(|(key, _)| {
                    let key = key.to_string_lossy().to_ascii_uppercase();
                    !["KEY", "TOKEN", "SECRET", "PASSWORD"]
                        .iter()
                        .any(|word| key.contains(word))
                }))
                .env(ISOLATED, "1")
                .stdin(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let start = Instant::now();
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "isolated lock fixture failed");
                    return;
                }
                if start.elapsed() >= Duration::from_secs(120) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("isolated lock fixture stuck");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _strict = contending(); // Existing test helper: every taking uses File::try_lock once.
        let home = tempfile::tempdir().unwrap();
        let taken = std::thread::scope(|scope| {
            let (completed, results) = std::sync::mpsc::channel();
            let mut rounds = Vec::new();
            let mut spawners = Vec::new();
            for spawner in 0..4 {
                let (release, round) = std::sync::mpsc::channel();
                rounds.push(release);
                let completed = completed.clone();
                spawners.push(scope.spawn(move || {
                    while let Ok(round) = round.recv() {
                        let status = std::process::Command::new("true").env_clear().status();
                        completed.send((spawner, round, status)).unwrap();
                    }
                }));
            }
            let mut taken = 0;
            for round in 0..100 {
                let held = lock(home.path()).unwrap().expect("initial round taking");
                for release in &rounds {
                    release.send(round).unwrap();
                }
                drop(held);
                let mut finished = [false; 4];
                for _ in 0..4 {
                    let (spawner, completed_round, status) = results
                        .recv_timeout(Duration::from_secs(30))
                        .expect("owned spawn round stuck");
                    assert_eq!(completed_round, round);
                    assert!(!finished[spawner], "duplicate spawn completion");
                    finished[spawner] = true;
                    assert!(status.unwrap().success(), "owned child failed");
                }
                assert!(finished.into_iter().all(|done| done));
                drop(
                    lock(home.path())
                        .unwrap()
                        .expect("a completed spawn round retained the dropped lock"),
                );
                taken += 1;
            }
            drop(rounds); // Stop every owned receiver before joining its spawner.
            for spawner in spawners {
                spawner.join().unwrap();
            }
            taken
        });
        assert_eq!(taken, 100, "a dropped lock was still held");
    }

    #[test]
    fn a_second_worker_exits_at_once() {
        // The lock is held on purpose: found held at once, not after the wait for one just dropped
        // (its 20 sleeps ran past the bound below on a loaded macOS runner).
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let _held = lock(home.path()).unwrap().expect("the first lock");
        assert!(lock(home.path()).unwrap().is_none());
        let t = Instant::now();
        run(home.path(), 60_000).unwrap(); // returns at once: the lock is held
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn status_probe_detects_an_exclusive_read_only_lock_without_changing_it() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("read-only.lock");
        std::fs::write(&path, "synthetic read-only status canary").unwrap();
        let producer = std::fs::File::open(&path).unwrap();
        producer.lock().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            lock_held(&path),
            "read-only exclusive holder was reported absent"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[test]
    fn status_probe_does_not_create_missing_files_or_conflict_with_shared_readers() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("reader.lock");
        assert!(!lock_held(&path));
        assert!(!path.exists(), "status created a missing lock");
        std::fs::write(&path, "synthetic shared status canary").unwrap();
        let reader = std::fs::File::open(&path).unwrap();
        reader.lock_shared().unwrap();
        assert!(
            !lock_held(&path),
            "shared readers were mistaken for an exclusive producer"
        );
        let another = std::fs::File::open(&path).unwrap();
        another.try_lock_shared().unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"synthetic shared status canary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn status_probe_rejects_links_and_non_regular_files() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("unrelated-held-file");
        std::fs::write(&target, "synthetic unrelated canary").unwrap();
        let held = std::fs::File::open(&target).unwrap();
        held.lock().unwrap();
        let link = home.path().join("linked.lock");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(
            !lock_held(&link),
            "status followed an unrelated held target"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"synthetic unrelated canary"
        );
        assert!(!lock_held(home.path()));
        let fifo = home.path().join("fifo.lock");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path in a private synthetic directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(!lock_held(&fifo));
        use std::os::unix::fs::FileTypeExt;
        assert!(
            std::fs::symlink_metadata(fifo)
                .unwrap()
                .file_type()
                .is_fifo()
        );
    }

    #[test]
    fn rebuild_retries_a_shared_status_probe_at_the_actual_blocked_attempt() {
        struct ClearHook;
        impl Drop for ClearHook {
            fn drop(&mut self) {
                REBUILD_STATUS_BLOCKED.with(|hook| {
                    hook.borrow_mut().take();
                });
            }
        }
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("synthetic rebuild status canary"))
            .unwrap();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let path = p.join("state/rebuild.lock");
        std::fs::write(&path, "synthetic shared probe").unwrap();
        let probe = std::fs::File::open(&path).unwrap();
        probe.lock_shared().unwrap();
        let called = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = called.clone();
        let _clear = ClearHook;
        REBUILD_STATUS_BLOCKED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                count.set(count.get() + 1);
                drop(probe);
            }));
        });
        assert!(
            rebuild(p).is_ok(),
            "rebuild did not retry after a transient shared probe"
        );
        assert_eq!(
            called.get(),
            1,
            "the actual blocked attempt did not consume the callback once"
        );
        assert!(REBUILD_STATUS_BLOCKED.with(|hook| hook.borrow().is_none()));
        assert!(p.join("knowledge.db").exists());
        std::fs::File::open(path).unwrap().try_lock().unwrap();
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

    /// An op that lands after the consumers drained and before the worker waits (here from the
    /// phase) still wakes it: the wait compares with what the pass read, not with what came after.
    #[test]
    fn an_op_that_lands_between_the_pass_and_the_wait_wakes_the_worker() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let device = {
            let mut raw = raw::open(&p).unwrap();
            raw.append(&raw::test_event("first")).unwrap();
            raw.device().to_owned()
        };
        let mut appended = false;
        let mut phase = |raw: &mut Raw, _: &Connection| -> Result<Phase> {
            if !appended {
                appended = true;
                raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({}))])?;
            }
            Ok(Phase::Idle)
        };
        let at_exit = std::cell::Cell::new(None);
        let consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen), Box::new(OpsSeen)];
        let held = lock(&p).unwrap();
        run_holding(
            &p,
            2_000,
            consumers,
            || {
                let k = knowledge::open(&p).unwrap();
                let passed = checkpoint::get_in(&k, checkpoint::OPS, "ops-seen", &device);
                at_exit.set(at_exit.get().or(Some(passed.unwrap())));
            },
            held,
            curating(&mut phase),
        )
        .unwrap();
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

    // docs/resident.md: a worker that stays (R2, R3, R10, R12).

    /// A resident worker over `Seen` in a thread, with a short idle time: what `oboete worker`
    /// runs in a home whose config.toml says `resident = true`.
    #[cfg(target_os = "linux")]
    fn resident(p: &Path, idle_ms: u64) -> std::thread::JoinHandle<Result<()>> {
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        let p = p.to_path_buf();
        std::thread::spawn(move || {
            let held = lock(&p)?;
            let phases = Phases {
                yields: true,
                resident: true,
                ..Phases::default()
            };
            run_holding(&p, idle_ms, vec![Box::new(Seen)], || {}, held, phases)
        })
    }

    /// Waits up to 10 s for `done`.
    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let t = Instant::now();
        while !done() {
            assert!(t.elapsed() < Duration::from_secs(10), "never: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A command's request to step aside, as a test makes one: the file, its folder made.
    fn yield_request(home: &Path) -> std::path::PathBuf {
        let dir = yield_requests(home);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("test")
    }

    /// A session's flags last changed longer ago than a prune keeps them.
    #[cfg(target_os = "linux")]
    fn stale_flags(p: &Path) -> std::path::PathBuf {
        let old = p.join("state").join("hooks").join("claude").join("old");
        std::fs::create_dir_all(&old).unwrap();
        let long_ago =
            std::time::SystemTime::now() - crate::hookstate::KEEP - Duration::from_secs(60);
        let dir = std::fs::File::open(&old).unwrap();
        dir.set_modified(long_ago).unwrap();
        old
    }

    /// Milestone 5 (docs/milestone-5-plan.md): raw.db put back to an old copy under a resident
    /// worker is opened again at its next check, so it reads the file hooks now write (and the
    /// forget logs are reconciled into it), not the one it held.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_opens_raw_db_again_when_another_file_takes_its_place() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("first")).unwrap();
        let device = raw.device().to_owned();
        drop(raw);
        // The copy holds the record: what the WAL kept is written into the file first.
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        let old = p.join("raw-old.db");
        std::fs::copy(p.join("raw.db"), &old).unwrap();
        let worker = resident(p, 100);
        let seen = |device: &str, n: i64| {
            knowledge::open(p)
                .ok()
                .and_then(|k| checkpoint::get(&k, "seen", device).ok())
                == Some(n)
        };
        until("the first record read", || seen(&device, 1));
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("second"))
            .unwrap();
        until("the second record read", || seen(&device, 2));
        // The old copy takes raw.db's place, as a restore by hand would put it.
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        let staged = p.join("raw.db.copy");
        std::fs::copy(&old, &staged).unwrap();
        std::fs::rename(&staged, p.join("raw.db")).unwrap();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("after the copy")).unwrap();
        raw.append(&raw::test_event("and one more")).unwrap();
        // A file put in raw.db's place is another device's (docs/cards.md S3).
        let copy = raw.device().to_owned();
        let last = raw.max_seq().unwrap();
        drop(raw);
        until("the replaced file read", || seen(&copy, last));
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        worker.join().unwrap().unwrap();
    }

    /// R3: at its idle time a resident worker backs up and prunes as a worker does before it
    /// exits, keeps the lock and waits. It does so again only after a round, and it exits at the
    /// idle time that finds `resident` turned off.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_stays_when_idle_until_its_config_says_otherwise() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("first")).unwrap();
        let old = stale_flags(p);
        let worker = resident(p, 100);
        let segments = || std::fs::read_dir(p.join("backups")).map_or(0, |d| d.count());
        // Its first idle step: the backup, then the prune.
        until("the first idle step", || !old.exists());
        let exported = segments();
        assert!(exported > 0, "no backup at the idle time");
        // Idle times pass with no round: it stays, and does that work only once.
        let old = stale_flags(p);
        std::thread::sleep(Duration::from_millis(500));
        assert!(running(p) && !worker.is_finished(), "it left when idle");
        assert!(old.exists(), "pruned again with no round since");
        // A round, then its idle step.
        raw.append(&raw::test_event("second")).unwrap();
        until("the idle step after a round", || !old.exists());
        assert!(segments() > exported);
        // Turned off: it exits at its next idle time.
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        worker.join().unwrap().unwrap();
        assert!(!running(p));
    }

    /// R2: only `oboete worker` started with no `--idle-ms` follows `[worker] resident`. With one
    /// it exits when idle whatever the file says, and the commands that borrow its loop return.
    /// R3: a config.toml that does not load leaves a resident worker as it is, and makes none.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_worker_started_with_no_idle_time_follows_the_resident_switch() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let config = |text: &str| {
            // The fixture publishes a complete snapshot. In-place truncation briefly exposes
            // an empty valid config (resident=false), rather than the intended malformed one.
            let staged = p.join("config.toml.next");
            std::fs::write(&staged, text).unwrap();
            std::fs::rename(staged, p.join("config.toml")).unwrap();
        };
        let hooks = |p: &Path| {
            let p = p.to_path_buf();
            std::thread::spawn(move || run_as(&p, 100, true))
        };
        config("[worker]\nresident = true\n");
        run(p, 50).unwrap();
        run_once(p).unwrap();
        rebuild(p).unwrap();
        let worker = hooks(p);
        until("it holds the lock", || running(p));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!worker.is_finished(), "it left a resident home when idle");
        config("[worker\n");
        std::thread::sleep(Duration::from_millis(400));
        assert!(!worker.is_finished(), "a file that does not load ended it");
        config("[worker]\nresident = false\n");
        worker.join().unwrap().unwrap();
        // Started while the file does not load: not resident.
        config("[worker\n");
        let worker = hooks(p);
        until("a worker with no readable switch exits", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
    }

    /// R3: no hook starts a resident worker again, so it is up for every wait of a phase, not
    /// only D10's short ones. The phase runs again at the time it named, not at each idle time,
    /// and the idle step is taken while it waits.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_runs_a_phase_again_at_the_time_it_waits_for() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("first")).unwrap();
        let polls = std::sync::Arc::new(AtomicUsize::new(0));
        // The first wait's end and the second poll, by the clock the worker waits by: a step of
        // the system clock moves both.
        let (due, again) = (
            std::sync::Arc::new(AtomicI64::new(0)),
            std::sync::Arc::new(AtomicI64::new(0)),
        );
        let worker = {
            let (p, polls) = (p.to_path_buf(), polls.clone());
            let (first, second) = (due.clone(), again.clone());
            std::thread::spawn(move || {
                let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
                    // First a wait of three idle times, then a long one; neither keeps a worker
                    // up that is not resident.
                    let now = crate::db::now_ms();
                    let wait = match polls.fetch_add(1, Ordering::SeqCst) {
                        0 => {
                            first.store(now + 300, Ordering::SeqCst);
                            300
                        }
                        1 => {
                            second.store(now, Ordering::SeqCst);
                            600_000
                        }
                        _ => 600_000,
                    };
                    Ok(Phase::Waiting {
                        until: now + wait,
                        up: false,
                    })
                };
                let phases = Phases {
                    curation: Some(&mut phase),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 100, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        until("the phase ran again", || polls.load(Ordering::SeqCst) > 1);
        assert!(
            again.load(Ordering::SeqCst) >= due.load(Ordering::SeqCst),
            "the phase ran again before its time"
        );
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(polls.load(Ordering::SeqCst), 2, "a round at each idle time");
        assert!(
            std::fs::read_dir(p.join("backups")).is_ok_and(|d| d.count() > 0),
            "no idle step while a phase waits"
        );
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        worker.join().unwrap().unwrap();
    }

    /// R10: a resident worker is killed at every shutdown of the PC. While it waits its outcome
    /// says all is well, so that kill is no alarm; during a round it says "stopped", as before.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_workers_outcome_is_clean_while_it_waits_and_not_during_a_round() {
        use std::sync::mpsc::channel;
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("first")).unwrap();
        let (entered, in_round) = channel();
        let (go_on, gate) = channel::<()>();
        let worker = {
            let p = p.to_path_buf();
            std::thread::spawn(move || {
                let mut rounds = 0;
                // The second round stops in its phase until the test lets it go on.
                let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
                    rounds += 1;
                    if rounds == 2 {
                        entered.send(()).ok();
                        gate.recv().ok();
                    }
                    Ok(Phase::Idle)
                };
                let phases = Phases {
                    curation: Some(&mut phase),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 100, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        let waits = || outcome(p).is_some_and(|(_, why)| why.is_empty()) && running(p);
        until("a clean outcome while it waits", waits);
        raw.append(&raw::test_event("second")).unwrap();
        in_round
            .recv_timeout(Duration::from_secs(10))
            .expect("no second round");
        assert_eq!(
            outcome(p).unwrap().1,
            STOPPED,
            "a round under a clean outcome"
        );
        go_on.send(()).unwrap();
        until("a clean outcome after the round", waits);
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        worker.join().unwrap().unwrap();
    }

    /// R3: a home removed and made again is not the home this worker locked. It stops at its next
    /// write by path and leaves the new home as it found it: no backup, no outcome, and its
    /// stores with their `-wal` and `-shm` files (SQLite removes those by name when a store's
    /// last connection closes, but not once the store's file is no longer at its path).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_worker_whose_home_was_replaced_leaves_the_new_one_alone() {
        let _contending = contending();
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        {
            let mut raw = raw::open(&p).unwrap();
            raw.append(&raw::test_event("first")).unwrap();
        }
        let worker = resident(&p, 300);
        // It waits, its idle step still before it.
        until("it waits", || {
            outcome(&p).is_some_and(|(_, why)| why.is_empty())
        });
        std::fs::remove_dir_all(&p).unwrap();
        // The new home has a lock file of its own: the same path, another file.
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::write(p.join("state").join("worker.lock"), "").unwrap();
        let mut made = vec!["config.toml".to_owned(), "state".to_owned()];
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        for store in ["raw.db", "knowledge.db"] {
            for ext in ["", "-wal", "-shm"] {
                std::fs::write(p.join(format!("{store}{ext}")), "the new home's").unwrap();
                made.push(format!("{store}{ext}"));
            }
        }
        until("the old worker stops", || worker.is_finished());
        let why = worker.join().unwrap().unwrap_err();
        assert!(why.is::<Gone>(), "{why:#}");
        let mut left: Vec<String> = std::fs::read_dir(&p)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        made.sort();
        assert_eq!(left, made, "the old worker changed the new home");
        let state = std::fs::read_dir(p.join("state")).unwrap().count();
        assert_eq!(
            state, 1,
            "the old worker wrote its outcome into the new home"
        );
    }

    /// R12: a command that needs the worker lock asks the worker that holds it to step aside.
    /// The worker backs up (a restore reads the backups), lets go and exits; the command runs.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_steps_aside_for_a_command_that_needs_the_lock() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        // An idle time it never reaches: the backup below is the one it makes as it lets go.
        let worker = resident(p, 600_000);
        until("it waits", || {
            outcome(p).is_some_and(|(_, why)| why.is_empty())
        });
        assert!(!p.join("backups").exists());
        let held = lock_asking(p).unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            std::fs::read_dir(p.join("backups")).is_ok_and(|d| d.count() > 0),
            "it let go without a backup"
        );
        assert!(!asked_aside(p));
        assert!(last_failure(p).is_none());
        drop(held);
        // `oboete rebuild` is such a command: it swaps knowledge.db as soon as it has the lock,
        // so the worker's stores are closed by then.
        // The next worker's own taking of the lock, not one a forked child still holds open.
        let last = outcome(p).unwrap().0;
        let worker = resident(p, 600_000);
        until("the next worker waits", || {
            outcome(p).is_some_and(|(taken, why)| taken > last && why.is_empty())
        });
        rebuild(p).unwrap();
        worker.join().unwrap().unwrap();
    }

    /// R12: a request left by a command that died is not obeyed for ever: after a minute it is
    /// ignored. A new one is obeyed.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_request_to_step_aside_older_than_a_minute_is_ignored() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let ask = yield_request(p);
        std::fs::write(&ask, "").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(61);
        let file = std::fs::File::options().write(true).open(&ask).unwrap();
        file.set_modified(old).unwrap();
        let worker = resident(p, 100);
        until("it holds the lock", || running(p));
        std::thread::sleep(Duration::from_millis(500));
        assert!(!worker.is_finished(), "it obeyed a request a minute old");
        std::fs::write(&ask, "").unwrap();
        until("it obeys a new request", || worker.is_finished());
        worker.join().unwrap().unwrap();
    }

    /// R3: a setting the owner changes is followed without a new record. At its idle time a
    /// resident worker that finds config.toml changed starts a round; otherwise it starts none.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_starts_a_round_when_config_toml_changed() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        let rounds = std::sync::Arc::new(AtomicUsize::new(0));
        let worker = {
            let (p, rounds) = (p.to_path_buf(), rounds.clone());
            std::thread::spawn(move || {
                let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
                    rounds.fetch_add(1, Ordering::SeqCst);
                    Ok(Phase::Idle)
                };
                let phases = Phases {
                    curation: Some(&mut phase),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 100, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        until("its first round", || rounds.load(Ordering::SeqCst) > 0);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(rounds.load(Ordering::SeqCst), 1, "a round with nothing new");
        let changed = "[worker]\nresident = true\n[summary]\ncurate = false\n";
        std::fs::write(p.join("config.toml"), changed).unwrap();
        until("a round after the change", || {
            rounds.load(Ordering::SeqCst) > 1
        });
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            rounds.load(Ordering::SeqCst),
            2,
            "a round at each idle time since"
        );
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        worker.join().unwrap().unwrap();
    }

    /// `p` removed and made again, with a lock file of its own. The old lock file lives on under
    /// another name beside it: a file made after it is freed can get its inode number back and
    /// read as the same file (docs/resident.md, Limits).
    #[cfg(target_os = "linux")]
    fn replace_home(p: &Path) {
        let _ = std::fs::hard_link(
            p.join("state").join("worker.lock"),
            p.with_extension("old-lock"),
        );
        std::fs::remove_dir_all(p).unwrap();
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::write(p.join("state").join("worker.lock"), "").unwrap();
    }

    /// R3 with a restore asked for: a home replaced while the worker waits, which asks for a
    /// restore, gets no outcome from the worker opening its stores again (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_replaced_home_that_asks_for_a_restore_gets_no_outcome() {
        let _contending = contending();
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let worker = resident(&p, 300);
        until("it waits", || {
            outcome(&p).is_some_and(|(_, why)| why.is_empty())
        });
        // As `replace_home`, the request written before the lock file.
        let _ = std::fs::hard_link(
            p.join("state").join("worker.lock"),
            p.with_extension("old-lock"),
        );
        std::fs::remove_dir_all(&p).unwrap();
        std::fs::create_dir_all(p.join("state")).unwrap();
        crate::backup::request_restore(&p);
        std::fs::write(p.join("state").join("worker.lock"), "").unwrap();
        until("the old worker stops", || worker.is_finished());
        let why = worker.join().unwrap().unwrap_err();
        assert!(why.is::<Gone>(), "{why:#}");
        let mut state: Vec<_> = std::fs::read_dir(p.join("state"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        state.sort();
        assert_eq!(
            state,
            ["restore-wanted", "worker.lock"],
            "written into the new home"
        );
    }

    /// R3 at the first taking: a lock taken in a home replaced before the run records it gets
    /// the run's outcome nowhere (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_lock_taken_before_the_home_was_replaced_writes_nothing_into_the_new_one() {
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let held = lock(&p).unwrap().unwrap();
        replace_home(&p);
        let why = run_holding(
            &p,
            0,
            vec![Box::new(Seen)],
            || {},
            Some(held),
            Phases::default(),
        )
        .unwrap_err();
        assert!(why.is::<Gone>(), "{why:#}");
        let state: Vec<_> = std::fs::read_dir(p.join("state"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(state, ["worker.lock"], "written into the new home");
    }

    /// R3 while the lock is taken: a home replaced after its lock file was opened gets no lock
    /// number, and the lock is not given (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_home_replaced_while_its_lock_is_taken_gets_no_lock_number() {
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        AFTER_OPEN.set(Some(replace_home));
        let got = lock(&p);
        AFTER_OPEN.set(None);
        assert!(got.unwrap().is_none());
        let state: Vec<_> = std::fs::read_dir(p.join("state"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(state, ["worker.lock"], "written into the new home");
    }

    /// R3 after the lock is released: a home replaced while a worker makes its last check gets
    /// no outcome and no lock number from it (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn no_outcome_goes_into_a_home_replaced_as_the_worker_exits() {
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut once = true;
        let why = run_with(&p, 0, vec![Box::new(Seen)], || {
            if std::mem::take(&mut once) {
                replace_home(&p);
            }
        })
        .unwrap_err();
        assert!(why.is::<Gone>(), "{why:#}");
        let state: Vec<_> = std::fs::read_dir(p.join("state"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(state, ["worker.lock"], "written into the new home");
    }

    /// A consumer that replaces the home in its first step and reports progress each time.
    #[cfg(target_os = "linux")]
    struct Replaces(
        std::path::PathBuf,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    );
    #[cfg(target_os = "linux")]
    impl Consumer for Replaces {
        fn name(&self) -> &'static str {
            "replaces"
        }
        fn step(&mut self, raw: &Raw, _: &Connection, _device: &str, after: i64) -> Result<i64> {
            if self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                replace_home(&self.0);
            }
            Ok((after + 1).min(raw.max_seq()?))
        }
        fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
    }

    /// R3 within a drain: the rescan opens raw.db by its path in each step, so a home replaced
    /// during a long drain is noticed between two batches, not only at the next round (Codex on
    /// #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_drain_stops_between_batches_when_its_home_was_replaced() {
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path().join("home");
        std::fs::create_dir(&p).unwrap();
        {
            let mut raw = raw::open(&p).unwrap();
            for text in ["a", "b", "c"] {
                raw.append(&raw::test_event(text)).unwrap();
            }
        }
        let steps = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Replaces(p.clone(), steps.clone()))];
        let why = run_consumers(&p, 0, consumers).unwrap_err();
        assert!(why.is::<Gone>(), "{why:#}");
        assert_eq!(steps.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// R10 and R12 with an embedding call out: the worker is not waiting. Its outcome keeps
    /// saying stopped, and it steps aside only once the call's answer is settled, which the call
    /// was paid for (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn an_embedding_call_that_is_out_is_work_in_flight() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut embed = crate::embed_phase::Phase::new(p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        let worker = {
            let p = p.to_path_buf();
            std::thread::spawn(move || {
                let phases = Phases {
                    embed: Some(&mut embed),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 600_000, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        let device = raw::open(p).unwrap().device().to_owned();
        until("the first round", || {
            knowledge::open(p)
                .and_then(|k| checkpoint::get(&k, "seen", &device))
                .is_ok_and(|at| at == 1)
        });
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(outcome(p).unwrap().1, STOPPED, "clean with a call out");
        std::fs::write(yield_request(p), "").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert!(!worker.is_finished(), "it left a call unsettled");
        release.send(()).unwrap();
        until("it steps aside once the call is settled", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
        assert!(last_failure(p).is_none());
    }

    /// R3 with an embedding call out: a worker whose config.toml stops saying `resident = true`
    /// leaves once the call's answer is settled, not at the next idle time (CodeRabbit on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_told_to_leave_settles_its_embedding_call_first() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut embed = crate::embed_phase::Phase::new(p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        let worker = {
            let p = p.to_path_buf();
            std::thread::spawn(move || {
                let phases = Phases {
                    embed: Some(&mut embed),
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 300, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        let device = raw::open(p).unwrap().device().to_owned();
        until("the first round", || {
            knowledge::open(p)
                .and_then(|k| checkpoint::get(&k, "seen", &device))
                .is_ok_and(|at| at == 1)
        });
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        // Several idle times.
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(!worker.is_finished(), "it left a call unsettled");
        release.send(()).unwrap();
        until("it leaves once the call is settled", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
        assert!(last_failure(p).is_none());
    }

    /// R12 with more to embed: a worker asked to step aside settles the call that is out and
    /// sends no other, so a backlog of batches and queries cannot keep a command waiting (Codex
    /// on #359, second round).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_worker_asked_to_step_aside_sends_no_new_embedding_call() {
        let _contending = contending();
        let mut s = crate::search::b::fixture::Store::new();
        s.decided(
            "github.com/o/r",
            1_000,
            "The parser reads one line at a time.",
            &[],
        );
        s.run();
        let stub = crate::embed::stub::Stub::start();
        crate::embed_phase::fixture::config(&s, &stub);
        let p = s.home.path().to_path_buf();
        let mut embed = crate::embed_phase::Phase::new(&p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        // A command asks before the worker's first round.
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::write(yield_request(&p), "").unwrap();
        let worker = {
            let p = p.clone();
            std::thread::spawn(move || {
                let phases = Phases {
                    embed: Some(&mut embed),
                    yields: true,
                    ..Phases::default()
                };
                run_holding(&p, 600_000, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        std::thread::sleep(Duration::from_millis(800));
        assert!(!worker.is_finished(), "it left a call unsettled");
        assert_eq!(
            stub.requests(),
            0,
            "a batch was sent for a command to wait on"
        );
        release.send(()).unwrap();
        until("it steps aside once the call is settled", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
        assert_eq!(stub.requests(), 0);
    }

    /// R12 with curation to do: a worker asked to step aside while an embedding call is out
    /// settles that call and runs no curation, which would start a call the command waits for
    /// (Codex on #359, fourth round).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_worker_asked_to_step_aside_runs_no_curation_while_it_settles_a_call() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut embed = crate::embed_phase::Phase::new(p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        // A command asks before the worker's first round.
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::write(yield_request(p), "").unwrap();
        let curations = std::sync::Arc::new(AtomicUsize::new(0));
        let worker = {
            let (p, curations) = (p.to_path_buf(), curations.clone());
            std::thread::spawn(move || {
                let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
                    curations.fetch_add(1, Ordering::SeqCst);
                    Ok(Phase::Idle)
                };
                let phases = Phases {
                    embed: Some(&mut embed),
                    curation: Some(&mut phase),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 600_000, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        std::thread::sleep(Duration::from_millis(600));
        assert!(!worker.is_finished(), "it left a call unsettled");
        release.send(()).unwrap();
        until("it steps aside once the call is settled", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
        assert_eq!(curations.load(Ordering::SeqCst), 0);
    }

    /// R3 with curation to do: a resident worker told to leave while an embedding call is out
    /// settles it and leaves, with no curation round in between (Codex on #359, fourth round).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_worker_told_to_leave_runs_no_curation_after_the_call() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut embed = crate::embed_phase::Phase::new(p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        let (released, after) = (
            std::sync::Arc::new(AtomicBool::new(false)),
            std::sync::Arc::new(AtomicUsize::new(0)),
        );
        let worker = {
            let (p, released, after) = (p.to_path_buf(), released.clone(), after.clone());
            std::thread::spawn(move || {
                let mut phase = |_: &mut Raw, _: &Connection| -> Result<Phase> {
                    if released.load(Ordering::SeqCst) {
                        after.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(Phase::Idle)
                };
                let phases = Phases {
                    embed: Some(&mut embed),
                    curation: Some(&mut phase),
                    resident: true,
                    ..Phases::default()
                };
                run_holding(&p, 300, vec![Box::new(Seen)], || {}, lock(&p)?, phases)
            })
        };
        let device = raw::open(p).unwrap().device().to_owned();
        until("the first round", || {
            knowledge::open(p)
                .and_then(|k| checkpoint::get(&k, "seen", &device))
                .is_ok_and(|at| at == 1)
        });
        std::fs::write(p.join("config.toml"), "[worker]\nresident = false\n").unwrap();
        // Several idle times: it has seen the change.
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(!worker.is_finished(), "it left a call unsettled");
        released.store(true, Ordering::SeqCst);
        release.send(()).unwrap();
        until("it leaves once the call is settled", || {
            worker.is_finished()
        });
        worker.join().unwrap().unwrap();
        assert_eq!(after.load(Ordering::SeqCst), 0);
    }

    /// R12: a command that gives up while the worker backs up for it takes its request away, and
    /// the worker goes on: a hook that appended meanwhile started none (Codex on #359, fifth
    /// round).
    #[test]
    fn a_request_withdrawn_during_the_backup_keeps_the_worker() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let raw = raw::open(p).unwrap();
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::write(yield_request(p), "").unwrap();
        AFTER_BACKUP.set(Some(|home| {
            std::fs::remove_file(yield_request(home)).unwrap();
        }));
        let left = steps_aside(p, &raw, &Holding::default());
        AFTER_BACKUP.set(None);
        assert!(!left);
    }

    /// A consumer that takes the request to step aside away in its first step, as a command that
    /// gave up does.
    #[cfg(target_os = "linux")]
    struct Withdraws(std::path::PathBuf, bool);
    #[cfg(target_os = "linux")]
    impl Consumer for Withdraws {
        fn name(&self) -> &'static str {
            "withdraws"
        }
        fn step(&mut self, raw: &Raw, _: &Connection, _device: &str, _after: i64) -> Result<i64> {
            if !std::mem::replace(&mut self.1, true) {
                let _ = std::fs::remove_file(yield_request(&self.0));
            }
            raw.max_seq()
        }
        fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
    }

    /// R12: a request that went away lets the embedding go on. A resident worker starts no round
    /// at its idle time, so it looks for that while it waits (Codex on #359, third round).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_withdrawn_request_lets_a_resident_worker_embed_again() {
        let _contending = contending();
        let mut s = crate::search::b::fixture::Store::new();
        s.decided(
            "github.com/o/r",
            1_000,
            "The parser reads one line at a time.",
            &[],
        );
        s.run();
        let stub = crate::embed::stub::Stub::start();
        crate::embed_phase::fixture::config(&s, &stub);
        let p = s.home.path().to_path_buf();
        // A record for the consumer to step on, after what `run` covered.
        s.raw.append(&raw::test_event("later")).unwrap();
        let mut embed = crate::embed_phase::Phase::new(&p);
        let unsent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("a test's call"));
        // A query that came back and is not settled yet: the worker does not step aside for the
        // request it finds, and holds the phase.
        let release = crate::embed_phase::fixture::hold_query(&mut embed, "key", "text", unsent);
        release.send(()).unwrap();
        until("the query's thread ends", || embed.done());
        std::fs::write(yield_request(&p), "").unwrap();
        let worker = {
            let p = p.clone();
            std::thread::spawn(move || {
                let phases = Phases {
                    embed: Some(&mut embed),
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                let withdraws = Box::new(Withdraws(p.clone(), false));
                run_holding(&p, 600_000, vec![withdraws], || {}, lock(&p)?, phases)
            })
        };
        until("the embedding goes on", || stub.requests() >= 1);
        assert!(!worker.is_finished());
        std::fs::write(yield_request(&p), "").unwrap();
        until("it steps aside for a new request", || worker.is_finished());
        worker.join().unwrap().unwrap();
    }

    /// R10: a resident worker that waited with a clean outcome and is asked for a restore opens
    /// the stores again as work: a kill during the restore or the rounds after it is reported
    /// (Codex on #359).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_restore_asked_for_while_it_waits_is_work_doctor_reports_a_kill_in() {
        use std::sync::{Arc, Mutex};
        struct Outcomes(std::path::PathBuf, Arc<Mutex<Vec<String>>>);
        impl Consumer for Outcomes {
            fn name(&self) -> &'static str {
                "outcomes"
            }
            fn step(&mut self, raw: &Raw, _: &Connection, _: &str, _: i64) -> Result<i64> {
                let why = outcome(&self.0).map(|(_, why)| why).unwrap_or_default();
                self.1.lock().unwrap().push(why);
                raw.max_seq()
            }
            fn rewind(&mut self, _: &Connection, _: &str, _: i64) -> Result<()> {
                Ok(())
            }
        }
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        std::fs::write(p.join("config.toml"), "[worker]\nresident = true\n").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let (p, seen) = (p.clone(), seen.clone());
            std::thread::spawn(move || {
                let held = lock(&p)?;
                let phases = Phases {
                    yields: true,
                    resident: true,
                    ..Phases::default()
                };
                let consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Outcomes(p.clone(), seen))];
                run_holding(&p, 600_000, consumers, || {}, held, phases)
            })
        };
        until("it waits", || {
            outcome(&p).is_some_and(|(_, why)| why.is_empty())
        });
        seen.lock().unwrap().clear();
        crate::backup::request_restore(&p);
        until("it reads again", || !seen.lock().unwrap().is_empty());
        assert_eq!(seen.lock().unwrap()[0], STOPPED);
        let held = lock_asking(&p).unwrap();
        worker.join().unwrap().unwrap();
        drop(held);
    }

    /// R12 with two commands: one that gives up takes only its own request away, so the worker
    /// never sees the other's go (Codex on #359). A request a command that died left a minute ago
    /// goes with the next command that asks.
    #[test]
    fn a_command_that_gives_up_leaves_the_request_of_one_still_waiting() {
        let _contending = contending();
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let _held = lock(&p).unwrap().unwrap();
        let dead = yield_request(&p);
        std::fs::write(&dead, "").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(61);
        let file = std::fs::File::options().write(true).open(&dead).unwrap();
        file.set_modified(old).unwrap();
        assert!(!asked_aside(&p));
        let wait = |p: std::path::PathBuf| {
            std::thread::spawn(move || {
                let _contending = contending();
                lock_asking(&p).map(drop)
            })
        };
        let first = wait(p.clone());
        until("the first asks", || asked_aside(&p));
        std::thread::sleep(Duration::from_secs(1));
        let second = wait(p.clone());
        let why = first.join().unwrap().unwrap_err().to_string();
        assert!(why.contains("the worker is busy"), "{why}");
        assert!(asked_aside(&p), "the second's request went with the first");
        assert!(second.join().unwrap().is_err());
        assert!(!asked_aside(&p));
        assert!(!dead.exists(), "the dead command's request stayed");
    }

    // M2 (spec 8.2): a crash at 20 points gives the rows of a run with none.

    /// A curator that is a function of its prompt: a claim quoting the end of each line.
    fn fake_curator(
        _: &str,
        prompt: &str,
        check: &crate::provider::AnswerCheck,
        _: &crate::provider::Gate,
    ) -> Result<crate::provider::ChainResult> {
        let claims: Vec<serde_json::Value> = prompt
            .lines()
            .filter_map(|l| {
                let (id, text) = l.split_once(' ')?;
                id.strip_prefix('L')?.parse::<u32>().ok()?;
                let chars: Vec<char> = text.chars().collect();
                let end: String = chars[chars.len().saturating_sub(16)..].iter().collect();
                let quote = end.trim().to_owned();
                (quote.chars().count() >= 8).then(|| (id.to_owned(), quote))
            })
            .enumerate()
            .map(|(i, (line, quote))| {
                let status = if i % 2 == 0 { "decided" } else { "proposed" };
                serde_json::json!({"id": format!("c{i}"), "kind": "decision", "status": status,
                    "speaker": "user", "scope": "repo", "body": quote, "quote": quote,
                    "line": line, "supersedes": []})
            })
            .collect();
        let output = serde_json::json!({"claims": claims, "summary": "s"});
        assert_eq!(check(&output), None, "{output}");
        Ok(crate::provider::ChainResult {
            provider: "fake".into(),
            output,
            tier: 1,
        })
    }

    /// A summarizer that answers each turn with a request and what was completed.
    fn fake_summarizer(
        _: &str,
        prompt: &str,
        check: &crate::provider::AnswerCheck,
        _: &crate::provider::Gate,
    ) -> Result<crate::provider::ChainResult> {
        let output = serde_json::json!({"skip": false, "request": format!("a turn of {}", prompt.len()),
            "investigated": "", "learned": "", "completed": "it ran", "next_steps": "",
            "notes": ""});
        assert_eq!(check(&output), None, "{output}");
        Ok(crate::provider::ChainResult {
            provider: "fake".into(),
            output,
            tier: 1,
        })
    }

    /// A worker run with the fake roles curating and summarizing, in small windows.
    fn curate_all(home: &Path) -> Result<()> {
        let rules = crate::capture::Settings::load(home)?.rules;
        let summary = crate::config::Summary {
            curate: true,
            window_tokens: 1_500,
            ..Default::default()
        };
        let db = crate::providers_db::open(home)?;
        let mut phase = |raw: &mut Raw, k: &Connection| {
            let windows =
                crate::curate::run_phase(raw, k, &db, &rules, &summary, "", &mut fake_curator)?;
            crate::turns::phase(
                raw,
                k,
                &db,
                &rules,
                &summary,
                "",
                &mut fake_summarizer,
                windows,
            )
        };
        let held = lock(home)?.expect("no other worker");
        run_holding(
            home,
            0,
            consumers(home),
            || {},
            Some(held),
            curating(&mut phase),
        )
    }

    /// Every row a run derives, each table sorted: raw.db's ops and knowledge.db's tables (an FTS
    /// table by its text, not its shadow tables), and providers.db's pending. Without the columns
    /// that hold when the run wrote them: an op's `ts` and the copies of it in knowledge.db, and
    /// `rewinds.ts`, `gaps.checked_at` and `manifests.built_at`.
    fn derived(home: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut dump = |c: &Connection, table: &str, cols: &str| {
            let mut s = c.prepare(&format!("SELECT {cols} FROM {table}")).unwrap();
            let n = s.column_count();
            let mut rows: Vec<String> = s
                .query_map([], |r| {
                    let cells: Vec<String> = (0..n)
                        .map(|i| match r.get_ref(i).unwrap() {
                            rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into(),
                            v => format!("{v:?}"),
                        })
                        .collect();
                    Ok(format!("{table}: {}", cells.join(" | ")))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.sort();
            out.extend(rows);
        };
        let raw = Connection::open(home.join("raw.db")).unwrap();
        dump(&raw, "ops", "device, op_seq, type, body, batch");
        let k = Connection::open(home.join("knowledge.db")).unwrap();
        let tables: Vec<(String, String)> = k
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let fts: Vec<&str> = tables
            .iter()
            .filter(|(_, sql)| sql.contains("USING fts5"))
            .map(|(name, _)| name.as_str())
            .collect();
        for (name, _) in &tables {
            let shadow = fts.iter().any(|f| {
                name.strip_prefix(f)
                    .is_some_and(|rest| rest.starts_with('_'))
            });
            if shadow || name.starts_with("sqlite_") {
                continue;
            }
            let cols = if fts.contains(&name.as_str()) {
                "rowid, *".to_owned()
            } else {
                k.prepare(&format!("SELECT name FROM pragma_table_info('{name}')"))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(Result::unwrap)
                    .filter(|c| !["ts", "checked_at", "built_at"].contains(&c.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            dump(&k, name, &cols);
        }
        let p = Connection::open(home.join("providers.db")).unwrap();
        dump(&p, "pending", "*");
        out
    }

    /// Copies a home, stores and all, and gives the copy of raw.db the copy's file identity, so
    /// it keeps the device id (a copied file is otherwise another device, `db::ensure_device`).
    fn copy_home(from: &Path, to: &Path) {
        copy(from, to);
        let path = to.join("raw.db");
        Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'store_file'",
                [crate::db::store_file(&path)],
            )
            .unwrap();
    }

    fn copy(from: &Path, to: &Path) {
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// M2's crash half (spec 8.2): the worker dies at 20 points of a curating run, among them
    /// between a window's ops landing in raw.db and the claims derived from them in knowledge.db,
    /// and the next worker leaves the same rows as a run that never stopped.
    #[test]
    fn a_crash_at_twenty_points_gives_the_rows_of_a_run_with_none() {
        let base = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/testdata/fixtures/overturn-cross.jsonl");
        crate::replay::run(base.path(), &fixture, None, 0, &[1], "claude", 0, 0).unwrap();
        let clean = tempfile::tempdir().unwrap();
        copy_home(base.path(), clean.path());
        crate::crash::off();
        curate_all(clean.path()).unwrap();
        let commits = crate::crash::count();
        let want = derived(clean.path());
        let count = |table: &str| {
            want.iter()
                .filter(|r| r.starts_with(&format!("{table}: ")))
                .count()
        };
        assert!(
            count("claims") > 0 && count("derivations") > 0 && count("turns") > 0,
            "{want:#?}"
        );
        assert!(commits >= 40, "{commits} commits");
        // Crashes that left ops in raw.db the claims consumer had not read yet.
        let mut underived = 0;
        for i in 0..20 {
            let at = 1 + i * commits / 20 + i % 3;
            let home = tempfile::tempdir().unwrap();
            copy_home(base.path(), home.path());
            crate::crash::at(at);
            let crashed = curate_all(home.path());
            crate::crash::off();
            assert!(crashed.is_err(), "no crash at commit {at} of {commits}");
            let ops: i64 = Connection::open(home.path().join("raw.db"))
                .unwrap()
                .query_row("SELECT COALESCE(MAX(op_seq), 0) FROM ops", [], |r| r.get(0))
                .unwrap();
            let read: i64 = Connection::open(home.path().join("knowledge.db"))
                .unwrap()
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) FROM op_checkpoints WHERE consumer = 'claims'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            underived += usize::from(ops > read);
            curate_all(home.path()).unwrap();
            let got = derived(home.path());
            let lost: Vec<&String> = want.iter().filter(|r| !got.contains(r)).collect();
            let added: Vec<&String> = got.iter().filter(|r| !want.contains(r)).collect();
            assert!(
                got == want,
                "a crash at commit {at} of {commits}: lost {lost:#?}, added {added:#?}"
            );
        }
        assert!(underived > 0, "no crash fell between an op and its claims");
    }
}
