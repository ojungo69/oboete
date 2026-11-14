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

pub(crate) struct IndexReport<'a> {
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
    pub(crate) fn drain(
        &mut self,
        home: &Path,
        raw: &Raw,
        k: &mut Connection,
        consumers: &mut [Box<dyn Consumer>],
        proof: &crate::executable::CommandHome,
    ) -> Result<()> {
        self.receipt.state = IndexState::Failed;
        loop {
            proof.check(home)?;
            if !pass_report(raw, k, consumers, &mut Some(&mut *self))? {
                break;
            }
        }
        self.receipt.state = IndexState::Complete;
        Ok(())
    }
    pub(crate) fn changed(&mut self, stage: &'static str) {
        self.receipt.stores_changed = true;
        (self.committed)(&MaintenanceCommit::Effect { stage });
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
fn check_reported_stores(home: &Path) -> Result<()> {
    for name in [
        "raw.db",
        "raw.db.restored",
        "knowledge.db",
        "raw.db-wal",
        "knowledge.db-wal",
    ] {
        let path = home.join(name);
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.is_file(),
                    "maintenance store is not a regular file"
                );
                #[cfg(test)]
                if name.ends_with("-wal")
                    && metadata.len() > 32
                    && let Some(before) = BEFORE_STORE_OPEN.take()
                {
                    before(&path);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
    /// Report effects committed outside `k` immediately, even if the step later fails.
    fn step_report(
        &mut self,
        raw: &Raw,
        k: &Connection,
        device: &str,
        after: i64,
        _committed: &mut dyn FnMut(),
    ) -> Result<i64> {
        self.step(raw, k, device, after)
    }
    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()>;
}

/// This milestone's consumers, in order.
pub fn consumers(home: &Path) -> Vec<Box<dyn Consumer>> {
    // Compression last: it waits for every consumer before it.
    vec![
        // The rescan first: the tombstones it appends are in raw before the others read a record.
        Box::new(crate::consumer::rescan::Rescan::new(home)),
        // #843: before `Fts`, so the rows past its checkpoint are those an older oboete indexed,
        // not those `Fts` indexes after it.
        Box::new(crate::consumer::fts::FtsText),
        Box::new(crate::consumer::fts::Fts),
        // Where the tool calls that may have made something are, for My Work (docs/my-work.md).
        Box::new(crate::consumer::products::Products),
        // What the records say of each Claude Code subagent, for My Work (docs/subagents.md).
        Box::new(crate::consumer::subagents::Subagents),
        // The op log's claims (milestone 3 D4), in op_seqs of every device that has ops.
        Box::new(crate::consumer::claims::Claims),
        // Claims whose quote a later tombstone masked or removed.
        Box::new(crate::consumer::claims::Anchors),
        // The op log's cards (docs/cards.md): what each curated window was.
        Box::new(crate::consumer::cards::Cards),
        // The op log's session summaries (docs/summaries.md): what each turn was.
        Box::new(crate::consumer::turns::Turns),
        // What My Work's jobs wrote (docs/my-work-model.md S4).
        Box::new(crate::consumer::my_work::MyWork),
        // Imported documents (milestone 4 D5), for search only.
        Box::new(crate::consumer::imported::Imported),
        // The files imported observations name, for the note on a file (docs/file-note.md F12).
        Box::new(crate::consumer::imported::ImportedFiles),
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
            // R14: each consumer's batch is progress, in a worker's round as in a rebuild's drain.
            crate::watchdog::progressed();
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
            let next = if let Some(report) = report.as_deref_mut() {
                c.step_report(raw, &tx, &device, at, &mut || {
                    report.changed("stores_changed")
                })?
            } else {
                c.step(raw, &tx, &device, at)?
            };
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
    static AFTER_KNOWLEDGE_OPEN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A one-shot test seam: the pathname changes between the last check and the next round.
    static BEFORE_ROUND: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// A synchronous one-shot fixture action after a rebuild meets a shared status probe.
    static REBUILD_STATUS_BLOCKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CARRY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_STORE_OPEN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// One-shot capture after planning, while preparation still owns the worker lock.
    pub(crate) static AFTER_PREPARATION_PLAN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
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
    let mut raw = crate::backup::open_raw(home)?;
    let mut k = crate::backup::open_knowledge(home)?;
    let mut consumers = consumers(home);
    checkpoint::rewind(&raw, &k, &mut consumers)?;
    // As a worker's start (milestone 5 D1 rule 7): the segments checked, then the forget request
    // logs, before any consumer runs. A raw.db put back from an older copy holds none of a forget
    // the logs hold, and the command would plan from, and send, what was forgotten.
    report_backup(home, &raw, &mut None, true);
    crate::forget::reconcile_or_say(home, &mut raw)?;
    drain(&raw, &mut k, &mut consumers)?;
    Ok(Some(held))
}

/// Owned, no-inference preparation for a caller whose plan needs the drained stores.
/// Finish the default drain before return; config and original-home proof survive for the key.
pub(crate) fn prepare_report<T>(
    home: &Path,
    caller: crate::executable::CommandCaller,
    committed: &mut impl FnMut(&MaintenanceCommit),
    inspect: impl FnOnce(&Raw, &Connection) -> Result<T>,
) -> (
    IndexReceipt,
    Result<(T, std::fs::File, crate::executable::CommandHome)>,
) {
    with_prepared_report(
        home,
        caller,
        committed,
        |_| Ok(()),
        inspect,
        |value, _, _, _, _, _| Ok(value),
    )
}

/// Validate closed-store consent, plan, then operate without releasing worker admission.
/// Every Result after worker/config admission completes the reported no-inference tail drain.
pub(crate) fn with_prepared_report<T, U>(
    home: &Path,
    caller: crate::executable::CommandCaller,
    committed: &mut impl FnMut(&MaintenanceCommit),
    validate: impl Fn(&crate::executable::CommandHome) -> Result<()>,
    inspect: impl FnOnce(&Raw, &Connection) -> Result<T>,
    operate: impl FnOnce(
        T,
        &mut Raw,
        &mut Connection,
        &mut [Box<dyn Consumer>],
        &crate::executable::CommandHome,
        &mut IndexReport<'_>,
    ) -> Result<U>,
) -> (
    IndexReceipt,
    Result<(U, std::fs::File, crate::executable::CommandHome)>,
) {
    let mut receipt = IndexReceipt::default();
    let result = (|| {
        let command = crate::executable::CommandHome::new(home, caller)?;
        // Refuse already-stale consent before asking an active worker to exit.
        validate(&command)?;
        command.check(home)?;
        let held = lock_asking_for(home, Some(&command))?;
        let proof = held
            .2
            .clone()
            .expect("reported preparation retains its home");
        proof.check(home)?;
        let config = match crate::settings::config_lock(home) {
            Ok(config) => config,
            Err(error) => {
                // A hook saw the worker hold even when config admission failed. Keep this
                // caller's original home proof for the same reported completion drain.
                proof.check(home)?;
                validate(&proof)?;
                proof.check(home)?;
                return match drain_holding_report(home, held, &mut receipt, committed) {
                    Ok(()) => Err(error.into()),
                    Err(cleanup) => Err(error)
                        .with_context(|| format!("operation cleanup also failed: {cleanup:#}")),
                };
            }
        };
        proof.check(home)?;
        // Stale consent refuses before any store work, including the completion drain.
        validate(&proof)?;
        proof.check(home)?;
        let value: Result<U> = (|| {
            receipt.state = IndexState::Failed;
            let mut report = IndexReport {
                receipt: &mut receipt,
                committed,
            };
            check_reported_stores(home)?;
            let changed = &mut report.receipt.stores_changed;
            let mut raw = crate::backup::open_raw_report(
                home,
                &mut report.receipt.raw_recovery,
                Some(&proof),
                &mut |stage| {
                    if stage == "stores_changed" {
                        *changed = true;
                    }
                    (report.committed)(&MaintenanceCommit::Effect { stage });
                },
            )?;
            proof.check(home)?;
            let mut k = crate::backup::open_knowledge_report(home, Some(&proof), &mut |stage| {
                report.changed(stage)
            })?;
            let mut consumers = consumers(home);
            checkpoint::rewind_report(
                &raw,
                &k,
                &mut consumers,
                &mut |consumer, ops, checkpoint| {
                    report.receipt.consumer_commits += 1;
                    (report.committed)(&MaintenanceCommit::Index {
                        consumer,
                        unit: if ops {
                            CheckpointUnit::Ops
                        } else {
                            CheckpointUnit::Records
                        },
                        checkpoint,
                    });
                },
            )?;
            proof.check(home)?;
            report_backup(home, &raw, &mut Some(&mut report), true);
            let reconciled =
                crate::forget::reconcile_report(home, &mut raw, &mut |stage, applied| {
                    report.receipt.forget_requests_applied += applied as u64;
                    report.changed(stage);
                })?;
            report.receipt.forget_log_warnings += reconciled.problems.len() as u64;
            report.drain(home, &raw, &mut k, &mut consumers, &proof)?;
            proof.check(home)?;
            // The planner/request builder also calls claims::schema. Keep any fresh/legacy
            // schema change in this owned transaction and report it only after commit.
            let tx = k.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changes = tx.total_changes();
            let schema: i64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
            crate::claims::schema(&tx)?;
            let value = inspect(&raw, &tx)?;
            let changed = tx.total_changes() != changes
                || tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))? != schema;
            tx.commit()?;
            if changed {
                report.changed("stores_changed");
            }
            // No planner transaction survives into provider admission or network work.
            operate(value, &mut raw, &mut k, &mut consumers, &proof, &mut report)
        })();
        #[cfg(test)]
        if let Some(after) = AFTER_PREPARATION_PLAN.take() {
            after(home);
        }
        // A hook that found this hold started no worker. The existing completion drain
        // releases admission and checks again, retaining this caller's original home proof.
        let drained = drain_holding_report(home, held, &mut receipt, committed);
        let value = match (value, drained) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(error).with_context(|| format!("operation cleanup also failed: {cleanup:#}"))
            }
        }?;
        proof.check(home)?;
        Ok((value, config, proof))
    })();
    (receipt, result)
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
        // A run that stepped aside for a command says so, not that all is well (#855).
        if holding.aside && result.is_ok() {
            note(home, holding.last, ASIDE);
        } else {
            record(home, holding.last, &result);
        }
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
    /// It stepped aside for a command (R12): its outcome says so (#855).
    aside: bool,
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
        if report.is_some() {
            check_reported_stores(home)?;
        }
        let opened = if let Some(report) = report.as_deref_mut() {
            let changed = &mut report.receipt.stores_changed;
            crate::backup::open_raw_report(
                home,
                &mut report.receipt.raw_recovery,
                holding.command.as_ref(),
                &mut |stage| {
                    if stage == "stores_changed" {
                        *changed = true;
                    }
                    (report.committed)(&MaintenanceCommit::Effect { stage });
                },
            )
        } else {
            crate::backup::open_raw(home)
        };
        if gone(home, holding) {
            return Err(Gone.into());
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
    // Or stamped by a newer oboete since (docs/store-format.md): opened again, it is refused.
    let replaced = |raw: &Raw| {
        let now = file_id(std::fs::metadata(home.join("raw.db")));
        (held.is_some() && now.is_some() && now != held) || raw.newer_format()
    };
    let opened = if let Some(report) = report.as_deref_mut() {
        crate::backup::open_knowledge_report(home, holding.command.as_ref(), &mut |stage| {
            report.changed(stage)
        })
    } else {
        crate::backup::open_knowledge(home)
    };
    #[cfg(test)]
    if let Some(after) = AFTER_KNOWLEDGE_OPEN.take() {
        after(home);
    }
    let mut k = opened?;
    if let Some(report) = report.as_deref_mut() {
        checkpoint::rewind_report(&raw, &k, consumers, &mut |consumer, ops, checkpoint| {
            report.receipt.consumer_commits += 1;
            (report.committed)(&MaintenanceCommit::Index {
                consumer,
                unit: if ops {
                    CheckpointUnit::Ops
                } else {
                    CheckpointUnit::Records
                },
                checkpoint,
            });
        })?;
    } else {
        checkpoint::rewind(&raw, &k, consumers)?;
    }
    report_backup(home, &raw, report, true);
    // Milestone 5 D1 rule 7: the forget request logs after the segments are checked, before any
    // consumer runs and before the export.
    if let Some(report) = report.as_deref_mut() {
        let reconciled = crate::forget::reconcile_report(home, &mut raw, &mut |stage, applied| {
            report.receipt.forget_requests_applied += applied as u64;
            report.changed(stage);
        })?;
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
        // At every batch, not only at the backup deadline: a newer oboete's stamp stops the
        // consumers at the next batch (docs/store-format.md; Codex on #503).
        if raw.newer_format() {
            return true;
        }
        if Instant::now() >= *next_backup {
            if replaced(raw) {
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
        // Milestone 5 D5 item 8: a uid job its purge left; never from a rebuild's or a restore's run.
        if report.is_none() {
            crate::forget::continue_due(home, raw);
        }
        false
    };
    let mut config = config_stamp(home);
    // Whether a round ran since a resident worker's last idle step (R3).
    let mut ran;
    // docs/deliverables.md: the phase's deliverables.db, opened when first needed.
    let mut deliverables_db: Option<rusqlite::Connection> = None;
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
        // R14: each round, each batch and each tick of a wait is progress.
        crate::watchdog::progressed();
        // The next phase round uses the pathname's current store. An end-of-round check
        // cannot cover a replacement that happened after it and before this first pass.
        if crate::backup::restore_requested(home) || replaced(&raw) {
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
            holding.aside = true;
            return Ok(false);
        }
        ran = true;
        if std::mem::take(&mut waited) {
            note(home, holding.last, STOPPED);
        }
        // What the pass below reads up to, new records or new ops of this device (an owner's
        // correction appends only an op): one that lands after it, even before the wait, wakes it.
        let seen = (raw.max_seq()?, raw.max_op_seq_of(raw.device())?);
        // Decision 70: a refresh request the page leaves after this round began wakes the wait.
        let began = crate::db::now_ms();
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
        // docs/deliverables.md: copies of what each turn that ended made, the worker's own (a
        // command that borrows the loop copies nothing), not while a command waits for it.
        if yields && !hold {
            match crate::deliverables::run(home, &raw, &k, &mut deliverables_db, &|| {
                asked_aside(home)
            }) {
                Ok(took) => again |= took,
                Err(e) => eprintln!("oboete worker: deliverables: {e:#}"),
            }
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
        if crate::backup::restore_requested(home) || replaced(&raw) {
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
                crate::watchdog::progressed();
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
                if crate::backup::restore_requested(home) || replaced(&raw) {
                    return Ok(true);
                }
                if yields && !calling(phases) && steps_aside(home, &raw, holding) {
                    holding.aside = true;
                    return Ok(false);
                }
                // A call that came back is written at once. And a request that went away (its
                // command gave up) starts a round, which lets the embedding go on: a resident
                // worker starts none at its idle time.
                if (raw.max_seq()?, raw.max_op_seq_of(raw.device())?) != seen
                    || crate::my_work_run::refresh_request(home, crate::db::now_ms())
                        .is_some_and(|at| at >= began)
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
            // The worker's own: a command that borrows the loop (a restore, a rebuild) ends
            // without it, and the worker's next pause builds it.
            if yields {
                maintain(home, &raw, departing, calling(phases));
                // The copies forget's denials cover, and files no row names (docs/deliverables.md).
                if let Err(e) = crate::deliverables::sweep(home, &raw, true) {
                    eprintln!("oboete worker: deliverables sweep: {e:#}");
                }
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
            // A batch a consumer put off, with nothing new to wake this wait (the rescan when the
            // dispatch hold is busy; Codex on #507), runs again here, as the exit's check below
            // starts a round for it; not once the worker is to leave (R3).
            if !departing && behind(&raw, &k, consumers)? {
                break false;
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
        if replaced(&raw) {
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
        let wanted = crate::backup::restore_requested(home) || replaced(&raw);
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
        // Decision 107: whoever held the lock between may have rewritten what a conversation
        // holds, with no tombstone (an expiry). Its key is of another open from here on.
        raw.renew();
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
    // R14 (parity row 2): a loop that makes no progress for 15 minutes leaves, so that the next
    // hook starts a worker anew; a call to a provider may keep it still for its own timeout.
    let lock_id = file_id(held.0.metadata());
    // This run's progress at once: a file a run before it wrote is not this one's (Codex on the
    // branch).
    write_progress(home, lock_id, 0);
    let (looked, left) = (home.to_path_buf(), home.to_path_buf());
    let _watch = crate::watchdog::watch(
        crate::watchdog::Watched::here(),
        crate::watchdog::EVERY,
        crate::watchdog::STILL,
        move |still| write_progress(&looked, lock_id, still),
        move |still| leave(&left, lock_id, still),
    );
    let mut curation = curation(home);
    let mut embed = crate::embed_phase::Phase::new(home);
    let mut shortlist = crate::shortlist::Builder::new(home);
    let mut viewer = crate::view::Starter::new();
    // The viewer before the stores: one that does not open still leaves the page up (Codex on
    // #378).
    if resident {
        viewer.due(home);
    }
    // X11: what a killed oboete left in the temp directory, at each start.
    crate::provider::sweep_scratch(&std::env::temp_dir(), std::time::SystemTime::now());
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

/// The curation phase's curator: the chain for one window, and the conversation each of its
/// entries keeps from one window to the next (decision 107).
struct Windows<'a> {
    providers: &'a [crate::config::Provider],
    /// What every paid entry together may spend this month.
    paid: f64,
    db: &'a Connection,
    talk: &'a mut crate::provider::Talk,
}

impl Windows<'_> {
    fn chain<'a>(
        &'a self,
        check: &'a crate::provider::AnswerCheck<'a>,
        gate: &'a crate::provider::Gate<'a>,
    ) -> crate::provider::Chain<'a> {
        crate::provider::Chain::new(self.providers, self.db)
            .paid_cap(self.paid)
            .check(check)
            .gate(gate)
    }
}

impl crate::curate::Curate for Windows<'_> {
    fn ask(
        &mut self,
        span: &str,
        prompt: &str,
        check: &crate::provider::AnswerCheck,
        gate: &crate::provider::Gate,
    ) -> Result<crate::provider::ChainResult> {
        self.chain(check, gate)
            .run("curator", span, prompt, &crate::curate::schema())
    }

    fn ask_turn(
        &mut self,
        span: &str,
        turn: &crate::provider::Turn,
        check: &crate::provider::AnswerCheck,
        gate: &crate::provider::Gate,
    ) -> Result<crate::provider::ChainResult> {
        crate::provider::Chain::new(self.providers, self.db)
            .paid_cap(self.paid)
            .check(check)
            .gate(gate)
            .talk(self.talk, turn)
            .run("curator", span, turn.prompt, &crate::curate::schema())
    }

    fn read(&self, provider: &str, span: &str) -> Vec<(i64, i64)> {
        self.talk.read(provider, span)
    }
}

/// The curation phase: it curates while `[summary] curate` asks for it, off until the cut-over
/// (spec 7.5).
fn curation(home: &Path) -> Box<CurationPhase<'static>> {
    let home = home.to_owned();
    // Opened once curation is on: a home that never asks for it gets no providers.db.
    let mut db = None;
    // Decision 107 (docs/curator-conversation.md): what each entry was sent in its conversation,
    // for as long as this phase lives: one run of the worker. A purge, an expiry and a restore
    // take the worker's lock: a store opened again within the run, and the lock let go and taken
    // again (`serve`, `Raw::renew`), give its windows another key (`curate::talk_key`), so none
    // of them runs within a conversation.
    let mut talk = crate::provider::Talk::default();
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
        let mut curator = Windows {
            providers: &cfg.providers,
            paid: cfg.paid_usd_per_month,
            db,
            talk: &mut talk,
        };
        // Who is asked and within what caps: a window held under other ones is tried again now.
        // That includes the key an entry's budget comes from (#238): another has its own limit.
        let chain = format!(
            "{:?} {} {:?}",
            cfg.providers,
            cfg.paid_usd_per_month,
            crate::provider::budget_keys(&cfg.providers)
        );
        // Owner decision 43: ops written under rules turned off owe the op text scan.
        raw.gates_with(&rules);
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
        // And as My Work's role (docs/my-work-model.md A1), each job with its own schema, while
        // `[my_work] curate` asks for it too.
        let call = |span: &str,
                    prompt: &str,
                    schema: &serde_json::Value,
                    check: &crate::provider::AnswerCheck,
                    gate: &crate::provider::Gate| {
            crate::provider::Chain::new(&cfg.providers, db)
                .paid_cap(cfg.paid_usd_per_month)
                .check(check)
                .gate(gate)
                .run(crate::my_work::ROLE, span, prompt, schema)
        };
        let mut runner = crate::my_work_run::Runner::new(
            (cfg.my_work.curate, cfg.my_work.calls_a_day),
            crate::my_work_run::jobs(),
            Box::new(call),
            (&rules, &cfg.summary, &chain),
        )
        .judge(cfg.judge.as_ref().map(|j| (j, cfg.paid_usd_per_month)))
        .refresh(&home);
        crate::turns::round(
            raw,
            k,
            db,
            &rules,
            &cfg.summary,
            &chain,
            &mut summarizer,
            &mut curator,
            &mut |raw, k, db, overdue| runner.pass(raw, k, db, overdue),
        )
    })
}

/// How long no hook has recorded before the worker builds raw's indexes (`maintain`): long enough that
/// a hook seldom comes during the build, which it waits out for at most its 2 s before it keeps
/// its events for the next hook to write back (docs/unwritten.md).
const INDEX_PAUSE_MS: i64 = 30 * 60_000;

/// What an idle worker keeps up besides the backups: raw's session index (`Raw::ensure_session_index`)
/// and the index of the events' repositories (`Raw::repo_index`, which the settings page's list of
/// repositories reads: #899 W2), each built once in a pause of the owner's work, since its first
/// build takes seconds a hook's write would wait out. Not while a command waits for the worker to
/// step aside (R12) or it is leaving, nor with a call out (R10). A build that fails is tried again
/// at a later idle time; the readers work without the indexes, only slower.
fn maintain(home: &Path, raw: &Raw, leaving: bool, calling: bool) {
    // Built (or unreadable) first: the newest hook record's time may walk a history of imports.
    let built = raw.has_session_index().unwrap_or(true) && raw.has_repo_index().unwrap_or(true);
    if leaving || calling || asked_aside(home) || built {
        return;
    }
    let paused = raw
        .last_hook_ts()
        .is_ok_and(|ts| ts.is_none_or(|ts| crate::db::now_ms() - ts >= INDEX_PAUSE_MS));
    if paused {
        let _ = raw.ensure_session_index();
        let _ = raw.repo_index();
    }
}

/// A run's outcome until it ends.
pub(crate) const STOPPED: &str = "it stopped before it finished (killed or crashed); the next worker a hook \
                       starts goes on from where it stopped";
/// R12: a run that stepped aside for a command (a restore, a rebuild, a forget's purge, a retention
/// pass, `worker stop`) ended for it, which is no failure and no kill (#855).
pub(crate) const ASIDE: &str = "it stepped aside for a command that needed the lock";

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
    note_with(home, last, why, |guard| guard.lock().is_ok());
}

/// `note`, with `take` taking its lock: R14's watchdog only tries, as the loop it ends may be stuck
/// holding it.
fn note_with(home: &Path, last: u64, why: &str, take: impl FnOnce(&std::fs::File) -> bool) {
    let state = home.join("state");
    let Ok(guard) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("worker-note.lock"))
    else {
        return;
    };
    if !take(&guard) {
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
    parse_outcome(&text).map(|(number, why)| (number, why.to_owned()))
}

pub(crate) fn parse_outcome(text: &str) -> Option<(u64, &str)> {
    let (number, why) = text.split_once('\n').unwrap_or((text, ""));
    Some((number.trim().parse().ok()?, why))
}

/// Why the last `oboete worker` stopped with an error, if it did.
pub fn last_failure(home: &Path) -> Option<String> {
    let (_, why) = outcome(home)?;
    // A run still going has not stopped.
    if why == STOPPED && running(home) {
        return None;
    }
    Some(why).filter(|why| !why.is_empty() && why != ASIDE)
}

/// Whether a process holds the worker lock now. It takes no number: this runs nothing.
pub fn running(home: &Path) -> bool {
    lock_held(&home.join("state").join("worker.lock"))
}

/// R14: the outcome of a run whose loop made no progress.
const NO_PROGRESS: &str = "its loop made no progress for 15 minutes or more, so it left; the next \
                           worker a hook starts goes on from where it stopped";

/// `state/worker-progress` (R15): this run's process id and the watchdog's looks in a row that
/// found no progress, replaced whole at each look. Nothing goes into a home that is not this
/// run's (R3).
fn write_progress(home: &Path, lock: FileId, still: u64) {
    let state = home.join("state");
    if file_id(std::fs::metadata(state.join("worker.lock"))) != lock {
        return;
    }
    let next = state.join("worker-progress.next");
    if std::fs::write(&next, format!("{}\n{still}\n", std::process::id())).is_ok() {
        let _ = std::fs::rename(&next, state.join("worker-progress"));
    }
}

/// R14: a worker whose loop is stuck notes why and when (`left`), ends its CLI runs and exits.
/// The lock goes with the process, and the next hook starts a new worker.
fn leave(home: &Path, lock: FileId, still: u64) {
    left(home, lock, still);
    eprintln!("oboete: worker: {NO_PROGRESS}");
    crate::provider::end_runs();
    std::process::exit(1);
}

/// Why a worker left, for doctor, and when, in `state/worker-left`, which the next run's outcome
/// does not replace; nothing in a home that is not the run's (R3).
fn left(home: &Path, lock: FileId, still: u64) {
    let state = home.join("state");
    if file_id(std::fs::metadata(state.join("worker.lock"))) == lock {
        if let Some((taken, _)) = outcome(home) {
            note_with(home, taken, NO_PROGRESS, |guard| guard.try_lock().is_ok());
        }
        let next = state.join("worker-left.next");
        if std::fs::write(&next, format!("{}\n{still}\n", crate::db::now_ms())).is_ok() {
            let _ = std::fs::rename(&next, state.join("worker-left"));
        }
    }
}

/// The watchdog's looks as minutes, rounded down.
fn minutes(looks: u64) -> u64 {
    looks.saturating_mul(crate::watchdog::EVERY.as_secs()) / 60
}

/// What the running worker's watchdog last wrote: its process id and the looks in a row with no
/// progress. None when the file is older than two looks: no watchdog writes it (a worker from
/// before R14, or one gone), and it must not make `status` say more than the lock does.
fn progress(home: &Path) -> Option<(u32, u64)> {
    let path = home.join("state").join("worker-progress");
    let age = std::fs::metadata(&path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    if age > crate::watchdog::EVERY * 2 + Duration::from_secs(10) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    Some((lines.next()?.parse().ok()?, lines.next()?.parse().ok()?))
}

/// The line about the last time a worker left on its own (R14), within the last week.
pub(crate) fn left_line(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join("state").join("worker-left")).ok()?;
    let mut lines = text.lines();
    let (at, still): (i64, u64) = (lines.next()?.parse().ok()?, lines.next()?.parse().ok()?);
    (crate::db::now_ms() - at < 7 * 86_400_000).then(|| {
        format!(
            "a worker last left on its own at {}, after {} min without progress",
            crate::db::utc(at),
            minutes(still)
        )
    })
}

/// `oboete worker status` (R15): whether a worker runs and its loop makes progress, the page,
/// and how runs ended. Whether one runs is the lock's answer; the progress file adds only what the
/// running worker's watchdog wrote.
pub fn status(home: &Path) -> Result<()> {
    for line in status_lines(home) {
        println!("{line}");
    }
    Ok(())
}

fn status_lines(home: &Path) -> Vec<String> {
    let running = running(home);
    let mut lines = vec![if running {
        match progress(home) {
            Some((pid, 0)) => format!("worker: running (process {pid}), making progress"),
            // How long a call out may keep it still is the watchdog's: no fixed limit is shown
            // (CodeRabbit on #613).
            Some((pid, still)) => format!(
                "worker: running (process {pid}), no progress for {} min",
                minutes(still)
            ),
            None => "worker: running".to_owned(),
        }
    } else {
        format!("worker: not running now ({})", who_starts(home))
    }];
    if running && !crate::config::autostart(home) {
        lines.push(format!("worker: {MANAGED}"));
    }
    if crate::config::worker(home).is_ok_and(|w| w.resident)
        && let Ok(view) = crate::config::view(home)
    {
        lines.push(crate::view::resident_line(home, view.port.get()));
    }
    if let Some(why) = last_failure(home) {
        lines.push(format!("the last worker run failed: {}", why.trim_end()));
    }
    lines.extend(left_line(home));
    lines
}

/// R16: said beside a running worker where no hook would start another once it stops (#678).
pub(crate) const MANAGED: &str = "when it stops, no hook starts one ([worker] autostart = false)";

/// What starts a worker that is not running (R15), or that nothing here does (R16).
pub(crate) fn who_starts(home: &Path) -> &'static str {
    if crate::config::autostart(home) {
        "the next hook starts it"
    } else {
        "no hook starts one: [worker] autostart = false"
    }
}

/// `oboete worker stop` (R15): the worker steps aside as for a command (R12), and the next hook
/// starts one again, as claude-mem's `stop` leaves it to the next hook, or what manages it does
/// (R16). The page stays (R1).
pub fn stop(home: &Path) -> Result<()> {
    let was = step_aside(home)?;
    println!(
        "{}",
        if !was {
            "worker: was not running"
        } else if crate::config::autostart(home) {
            "worker: stopped; the next hook starts it again, and the page stays up"
        } else {
            "worker: stopped; no hook starts it again ([worker] autostart = false), and the page \
             stays up"
        }
    );
    Ok(())
}

/// A running worker asked to step aside, as a command asks (R12), and its lock waited for:
/// whether one was running.
pub(crate) fn step_aside(home: &Path) -> Result<bool> {
    let was = running(home);
    let held = lock_asking(home).map_err(|e| {
        anyhow::anyhow!(
            "{e:#}; a worker whose loop makes no progress leaves by itself within {} min",
            minutes(crate::watchdog::STILL)
        )
    })?;
    drop(held);
    Ok(was)
}

/// `oboete worker restart` (R15): `stop`, then a worker started as a hook starts one (D6), none
/// where `[worker] autostart = false` leaves it to what manages it (R16).
pub fn restart(home: &Path) -> Result<()> {
    stop(home)?;
    if std::env::var_os("OBOETE_NO_SPAWN").is_some() {
        println!("worker: OBOETE_NO_SPAWN is set, so none is started");
        return Ok(());
    }
    if !crate::config::autostart(home) {
        println!(
            "worker: none started ([worker] autostart = false); what manages the worker starts it"
        );
        return Ok(());
    }
    if crate::hook::start_worker(home)?.is_none() {
        println!("worker: a hook started one already");
        return Ok(());
    }
    let until = Instant::now() + Duration::from_secs(10);
    while !running(home) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "{}",
        if running(home) {
            "worker: started"
        } else {
            "worker: started, but it does not hold the lock yet; `oboete worker status` tells"
        }
    );
    Ok(())
}

/// Read-only status readers share an existing regular lock; only the exclusive producer
/// prevents them from taking it. Links and special files are never followed or waited on.
/// Whether a rebuild is filling a new knowledge.db: `oboete rebuild`, the purge's among them, holds
/// `state/rebuild.lock` from before the old store is set aside until its last consumer has run
/// (docs/resident.md R13). Readers of knowledge.db that answer for the whole store check it after raw
/// is open: a set-aside waits for raw's lock, so a reader that sees it free reads the old store
/// (#855). A file a failed rebuild left behind is no rebuild.
pub(crate) fn rebuilding(home: &Path) -> bool {
    lock_held(&home.join("state/rebuild.lock"))
}

/// A lock file's taking time, as its modification time, set through the handle that holds it
/// (#855): when the pause it causes began, read only while it is held (`curation_pause`), so a time
/// a crash left is never shown. Its bytes are not written: Windows lets no other handle read the
/// bytes of a file locked whole, while its metadata stays readable, and a lock file that is a link
/// would carry the write to its target.
pub(crate) fn stamp(lock: &std::fs::File) {
    let _ = lock.set_modified(std::time::SystemTime::now());
}

/// Why curation is paused now, and since when (#855): `rebuild` while a rebuild fills a new
/// knowledge.db, else `rewrite` while the stored records are rewritten (a forget's purge, a
/// retention pass, masks applied), each while its lock is held. It reads lock files only, so the
/// page's stores-free route and doctor with a damaged store can ask.
pub(crate) fn curation_pause(home: &Path) -> Option<(&'static str, Option<i64>)> {
    let state = home.join("state");
    let held: Vec<(&'static str, Option<i64>)> =
        [("rebuild", "rebuild.lock"), ("rewrite", "purge.lock")]
            .into_iter()
            .filter_map(|(kind, name)| {
                let path = state.join(name);
                lock_held(&path).then(|| {
                    let since = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    let ms = since
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| i64::try_from(d.as_millis()).ok());
                    (kind, ms)
                })
            })
            .collect();
    let (kind, _) = *held.first()?;
    // A purge's rebuild holds both: the pause began when the first was taken (cubic on #868).
    Some((kind, held.iter().filter_map(|h| h.1).min()))
}

/// Doctor's line while curation is paused (#855); with `[summary] curate` off no curation waits,
/// so the line names what holds the lock alone (#871).
pub(crate) fn paused_line(home: &Path) -> Option<String> {
    let (kind, since) = curation_pause(home)?;
    let what = match kind {
        "rebuild" => "knowledge.db is being rebuilt",
        _ => "the stored records are being rewritten (a forget, the retention period or new masks)",
    };
    // A config.toml that does not load stops curation too: only one read as off says less (cubic
    // on #872).
    let line = match crate::config::load(home) {
        Ok(c) if !c.summary.curate => what.to_owned(),
        _ => format!("curation is paused while {what}"),
    };
    Some(match since {
        Some(at) => format!("{line}, since {}", crate::db::utc(at)),
        None => line,
    })
}

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
                try_lock_shared(&file),
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

pub(crate) fn try_lock_shared(f: &std::fs::File) -> Result<(), std::fs::TryLockError> {
    try_lock_with(f, std::fs::File::try_lock_shared)
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

fn carry_rebuild_vectors(
    home: &Path,
    name: &str,
    proof: &crate::executable::CommandHome,
    outcome: &mut MaintenanceOutcome,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> Result<()> {
    let kept = home.join(name);
    if kept.exists() {
        let carried = crate::knowledge::open(home).and_then(|k| {
            // Milestone 5 D5: no forgotten claim's or document's vector comes back, nor one whose
            // forget only the logs hold yet: the pass after this reconciles them.
            let (forgotten, home_id) = match crate::raw::read_only(home)? {
                Some(raw) => (
                    crate::forget::Forgotten {
                        uids: raw.forgotten_set()?,
                        records: raw.forgotten_record_keys()?,
                    },
                    raw.home_id()?,
                ),
                None => Default::default(),
            };
            let forgotten = crate::forget::with_logged(home, forgotten, home_id.as_deref())?;
            crate::embed_phase::carry(&k, &kept, &forgotten)
        });
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
                match put_back(home, name, proof) {
                    Ok(()) => {
                        outcome.effects.knowledge_put_back = true;
                        outcome.old_knowledge_kept = false;
                        crate::backup::effect("knowledge_put_back", &mut |stage| {
                            committed(&MaintenanceCommit::Effect { stage })
                        })?;
                    }
                    Err(back) => {
                        return Err(error)
                            .with_context(|| format!("rebuild put-back also failed: {back:#}"));
                    }
                }
                return Err(error).context(
                    "rebuild: the vectors of knowledge.db could not be carried; nothing was changed",
                );
            }
        }
    }
    Ok(())
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
                Ok(()) => {
                    stamp(&_rebuilding);
                    break;
                }
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
            carry_rebuild_vectors(home, &name, &proof, &mut outcome, committed)?;
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
    restore_report_to(home, expected, caller, None, committed)
}

/// docs/backup-points.md B4: the CLI's restore back to `point`, as `restore` runs one.
pub fn restore_to(home: &Path, point: &crate::backup::Point) -> Result<String> {
    restore_report_to(
        home,
        None,
        crate::executable::CommandCaller::Worker,
        Some(point),
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

/// docs/backup-points.md B3: a backup now, under the worker's lock (a running worker steps
/// aside, as for a restore). The segments written.
pub fn backup_run(home: &Path) -> Result<Vec<std::path::PathBuf>> {
    let held = lock_asking(home)?;
    let exported = crate::backup::export_timed(home);
    // A hook that appended while the worker stood aside started no other: what it wrote is
    // indexed before the lock goes, whether the export went well or not, as a restore's drain
    // does, with no model call (Codex and cubic on #880). A failed drain is said, and the
    // segments written are still reported.
    if let Err(e) = run_once_holding_report(home, held, &mut |_| {}).1 {
        eprintln!("oboete: indexing what came during the backup: {e:#}");
    }
    Ok(exported?.into_iter().map(|(path, _)| path).collect())
}

pub(crate) fn restore_report_to(
    home: &Path,
    expected: Option<&str>,
    caller: crate::executable::CommandCaller,
    point: Option<&crate::backup::Point>,
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
    let restored = crate::backup::restore_report_holding_to(
        home,
        expected,
        held.2.as_ref(),
        point,
        &mut |stage| committed(&MaintenanceCommit::Effect { stage }),
    );
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

fn run_once_holding_report(
    home: &Path,
    held: Lock,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> (IndexReceipt, Result<()>) {
    let mut receipt = IndexReceipt::default();
    let result = drain_holding_report(home, held, &mut receipt, committed);
    (receipt, result)
}

fn drain_holding_report(
    home: &Path,
    held: Lock,
    receipt: &mut IndexReceipt,
    committed: &mut impl FnMut(&MaintenanceCommit),
) -> Result<()> {
    let mut report = IndexReport { receipt, committed };
    let result = run_holding_report(
        home,
        0,
        consumers(home),
        || {},
        Some(held),
        Phases::default(),
        &mut Some(&mut report),
    );
    report.receipt.state = if result.is_ok() {
        IndexState::Complete
    } else {
        IndexState::Failed
    };
    result
}

/// One run now, for a command that has written ops and waits for the consumers to read them (a
/// claim's correction or mute, `oboete recurate` once it lets the lock go) and for tests. It waits
/// up to 2 s for the lock rather than return at once: a worker a hook started holds it only while
/// it drains, and a lock just released can still be held for a moment by a child another thread
/// forked (it keeps the open file until it execs), which made hook tests run nothing under a
/// parallel suite.
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
mod tests;
