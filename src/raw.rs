//! Design B's record store (docs/spec.md sections 1.6, 2.1, 2.5; docs/milestone-2-plan.md Task 1):
//! `raw.db`, one sequence per device holding events and tombstones. (device, seq) is the only
//! ordering and partition key; session, repo and branch are labels, never an index root.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{Duration, Instant};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- One sequence per device holds events and tombstones (spec 1.6, 5.8). A rowid table: bodies up
-- to the capture cap are far above the row size WITHOUT ROWID suits (under 1/20 of a page).
CREATE TABLE IF NOT EXISTS records (
  device TEXT NOT NULL,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,          -- 'event', 'tombstone', or 'removed' (restored without its body)
  ts INTEGER NOT NULL,         -- unix ms, the event's own time (replay: the fixture's)
  kind TEXT,                   -- Event.kind; NULL for a tombstone
  agent TEXT, session TEXT,    -- labels, never keys
  repo TEXT, branch TEXT, head TEXT, gitdir TEXT, cwd TEXT,
  source TEXT NOT NULL,        -- 'hook'; later 'oboete-v1', 'transcript'
  enc TEXT NOT NULL DEFAULT 'plain',
  body BLOB,                   -- NULL for a tombstone
  original_bytes INTEGER,      -- set when head-and-tail cut the body
  target_device TEXT, target_seq INTEGER, target_offset INTEGER, target_length INTEGER,
  PRIMARY KEY (device, seq)
);
-- D8: a read finds the tombstones of the records it returns by their target.
CREATE INDEX IF NOT EXISTS tombstone_targets ON records(target_device, target_seq)
  WHERE type = 'tombstone';
-- spec 2.2: rule, where and when, never the value. `field` is where in the record: a JSON
-- pointer into the body (`/output`; `/trigger#key` for a key of that object) or a label column
-- (`cwd`). `offset` is where the mask starts in that field as stored; `length` is the secret's
-- own length; both in bytes. `ts` is when the mask was applied (a replay stamps the event with the
-- fixture's time, not this).
CREATE TABLE IF NOT EXISTS ledger (
  device TEXT NOT NULL, seq INTEGER NOT NULL, field TEXT NOT NULL, rule TEXT NOT NULL,
  offset INTEGER NOT NULL, length INTEGER NOT NULL, ts INTEGER NOT NULL, ruleset TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_seq ON ledger(device, seq);
-- Milestone 3 D1: the curation op log, this device's window, claim, correction and digest ops.
-- Kept, not derived (spec 1.7): it shares raw.db's durability, backups and restore.
CREATE TABLE IF NOT EXISTS ops (
  device TEXT NOT NULL,
  op_seq INTEGER NOT NULL,
  type TEXT NOT NULL,          -- 'window', 'claim', 'correction', 'digest', 'exclusion', ...
  ts INTEGER NOT NULL,         -- unix ms, when it was appended
  body TEXT NOT NULL,          -- JSON, at most MAX_OP_BYTES
  batch INTEGER NOT NULL,      -- the first op_seq of the append it came in: a backup keeps it whole
  PRIMARY KEY (device, op_seq)
);
-- Milestone 4 D13: the exclusion list is read before each outbound call, from its few ops alone.
CREATE INDEX IF NOT EXISTS ops_exclusions ON ops(device, op_seq) WHERE type = 'exclusion';
-- The curation checkpoint, which SessionStart reads (Task 8, MUST-M9): the last window op without
-- a scan of the ops after it (178,370 imports took 136 ms).
CREATE INDEX IF NOT EXISTS ops_windows ON ops(device, op_seq) WHERE type = 'window';
-- M5: bodyless deletion authority; privacy.db replays these after an older backup restore.
CREATE TABLE IF NOT EXISTS denied_records(
  device TEXT NOT NULL, seq INTEGER NOT NULL, fingerprint TEXT NOT NULL, origin TEXT,
  PRIMARY KEY(device,seq)
);
CREATE INDEX IF NOT EXISTS denied_fingerprint ON denied_records(fingerprint);
CREATE INDEX IF NOT EXISTS denied_origin ON denied_records(origin);
CREATE TABLE IF NOT EXISTS import_origins(
  device TEXT NOT NULL, seq INTEGER NOT NULL, origin TEXT, fingerprint TEXT NOT NULL,
  PRIMARY KEY(device,seq)
);
CREATE TABLE IF NOT EXISTS forget_jobs(
  id TEXT PRIMARY KEY, target TEXT NOT NULL, started INTEGER NOT NULL, step INTEGER NOT NULL
);
";

/// Only the viewer's prompt reads create this, never `open` or a hook.
const PROMPT_INDEX: &str = "CREATE INDEX IF NOT EXISTS records_prompts
    ON records(ts DESC, device DESC, seq DESC) WHERE type = 'event' AND kind = 'prompt'";

/// One agent event as captured, after redaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub agent: String,
    pub session: String,
    /// prompt, tool, reply, compaction, end
    pub kind: String,
    pub ts: i64,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub gitdir: Option<String>,
    pub cwd: Option<String>,
    pub source: String,
    pub body: String,
    pub original_bytes: Option<i64>,
}

/// What a tombstone points at: a whole record, or a byte range of its body.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Record {
        device: String,
        seq: i64,
    },
    Range {
        device: String,
        seq: i64,
        offset: i64,
        length: i64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Event(Box<Event>),
    /// An event a tombstone targets whole: its seq stays, its body never leaves `raw.rs`
    /// (written from milestone 2 Task 7 on).
    Removed,
    Tombstone(Target),
}

/// A record as it leaves `raw.rs`; `Raw::after` is the only way out.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub device: String,
    pub seq: i64,
    pub item: Item,
}

/// A live prompt for the feed, read through `after` (D8) and gated alone as a window reads it.
pub struct Prompt {
    pub device: String,
    pub seq: i64,
    pub ts: i64,
    pub agent: String,
    pub repo: Option<String>,
    pub text: String,
}

/// A prompt page's last kept record: its own time and stored ID, descending (page.md P3).
pub type PromptPosition = (i64, String, i64);

impl Prompt {
    pub fn position(&self) -> PromptPosition {
        (self.ts, self.device.clone(), self.seq)
    }

    /// `search::b::get`'s record key, including this device's ID too.
    pub fn key(&self) -> String {
        format!("{}:{}", self.device, self.seq)
    }
}

/// What a span of a device's events agree on (`Raw::labels_in`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpanLabels {
    /// Their agent and session, when they are of one session.
    pub session: Option<(String, String)>,
    /// Their repository, when they are of one.
    pub repo: Option<String>,
    /// The time of the last of them, unix ms; none when the span holds no event.
    pub ts: Option<i64>,
}

/// What a tombstone removes: its target's seq and, for a part of that record, the part's
/// offset and length. A window op lists these as `[seq, offset, length]` (docs/cards.md K4).
pub type Removal = (i64, Option<i64>, Option<i64>);

/// What an op records (milestone 3 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Window,
    Claim,
    Correction,
    Digest,
    /// A repository excluded, or taken back out (`undo`): `{repo, undo}` (spec 5.5, milestone 4
    /// D13). An older binary stops at an op type it does not know.
    Exclusion,
    /// A document another memory tool kept (milestone 4 D5): an `ImportDoc`.
    Import,
    /// Where an import of records stands in its source (milestone 4 D6): a `Checkpoint` and
    /// `to_seq`, the last seq when its batch was appended.
    Migration,
    /// A turn's summary (docs/summaries.md): a `turns::TurnOp`.
    Turn,
}

impl OpKind {
    fn name(self) -> &'static str {
        match self {
            OpKind::Window => "window",
            OpKind::Claim => "claim",
            OpKind::Correction => "correction",
            OpKind::Digest => "digest",
            OpKind::Exclusion => "exclusion",
            OpKind::Import => "import",
            OpKind::Migration => "migration",
            OpKind::Turn => "turn",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        [
            Self::Window,
            Self::Claim,
            Self::Correction,
            Self::Digest,
            Self::Exclusion,
            Self::Import,
            Self::Migration,
            Self::Turn,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }
}

/// One op as it leaves `raw.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub device: String,
    pub op_seq: i64,
    pub kind: OpKind,
    pub ts: i64,
    pub body: serde_json::Value,
    /// The first op_seq of the `append_ops` it came in: one window's ops share it.
    pub batch: i64,
}

/// A record hooks wrote, or `replay` wrote in their stead (dev and evaluation homes): what curation
/// reads and the manifest shows. Any other source is imported (`oboete-v1`, `transcript`,
/// milestone 4 D6).
pub fn is_live(source: &str) -> bool {
    LIVE.contains(&source)
}

/// The live sources, which `is_live` and the queries that pick live records read.
const LIVE: [&str; 2] = ["hook", "replay"];

/// A document another memory tool kept, as an `import` op's body (milestone 4 D5): a claude-mem
/// observation, session summary or prompt. `uid` is `<source>:<source_id>`, as v1's import named
/// it, so what was judged on v1's evaluation store maps to it. `ts` is unix ms.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImportDoc {
    pub uid: String,
    pub source: String,
    pub source_id: String,
    pub kind: String,
    pub repo: String,
    pub session: String,
    pub ts: i64,
    pub title: String,
    pub body: String,
}

/// Documents per `append_imports` append (D5), and the most records an `append_imported` takes.
pub const IMPORT_BATCH: usize = 500;

/// Where an import of records stands in its source (D6), written with each batch as a `migration`
/// op. `key` is `oboete-v1:<device_id>` or `transcript:<agent>:<session>`; `through` is the last
/// v1 event id or settled transcript event ordinal, excluding synthetic end-of-file events.
/// v1's `row` fingerprints its event at `through`, whose id v1 reuses when its newest events are
/// deleted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub key: String,
    pub through: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<V1Row>,
    /// SHA-256 of the settled transcript events through this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct V1Row {
    pub id: i64,
    pub ts: i64,
    /// The SHA-256 of v1's session id (`migrate::v1_row`): the op is stored text.
    pub session_id: String,
}

/// The durable deny-list, inside the append transaction. A database error is never permission.
/// The old test-only sentinel still pins importer routing; real deletion is tested separately.
fn denied(
    conn: &Connection,
    fingerprint: Option<&str>,
    origin: Option<&str>,
    text: &str,
) -> Result<bool> {
    let denied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM denied_records WHERE (?2 IS NOT NULL AND origin=?2)
          OR (?2 IS NULL AND fingerprint=?1))",
        params![fingerprint, origin],
        |r| r.get(0),
    )?;
    Ok(denied || cfg!(test) && text.contains(DENIED_IN_TESTS))
}

/// What `denied` refuses in tests.
pub const DENIED_IN_TESTS: &str = "oboete-test:forgotten";

/// A title longer than this is cut when its op is over the cap: a title as long as a body is a
/// malformed row.
const TITLE_BYTES: usize = 1 << 10;

/// `doc` with its body clipped so that its op fits `MAX_OP_BYTES`, and a title over
/// `TITLE_BYTES` too, each with a marker saying how long it was, as a clipped tool output's does.
/// An error when the other fields alone are over the cap (cubic on the Task 3 PR).
fn within_op_cap(mut doc: ImportDoc) -> Result<ImportDoc> {
    let size = |d: &ImportDoc| serde_json::to_string(d).map(|s| s.len());
    if size(&doc)? <= MAX_OP_BYTES {
        return Ok(doc);
    }
    if doc.title.len() > TITLE_BYTES {
        doc.title = clipped(&doc.title, TITLE_BYTES);
    }
    // What is over once the title is cut, which may be nothing (Codex on #305).
    let over = size(&doc)?.saturating_sub(MAX_OP_BYTES);
    if over == 0 {
        return Ok(doc);
    }
    // Each byte cut takes at least one byte of JSON with it; escapes can take more.
    let mut keep = doc.body.len().saturating_sub(over + 128);
    loop {
        let cut = ImportDoc {
            body: clipped(&doc.body, keep),
            ..doc.clone()
        };
        if size(&cut)? <= MAX_OP_BYTES {
            return Ok(cut);
        }
        anyhow::ensure!(
            keep > 0,
            "import {}: over the {MAX_OP_BYTES}-byte op cap with no body left",
            doc.uid
        );
        keep = keep * 9 / 10;
    }
}

/// The first `keep` bytes of `text` (to a character's end), and a marker saying how long it was.
fn clipped(text: &str, keep: usize) -> String {
    let marker = format!("\n…[clipped, {} chars in full]", text.chars().count());
    format!("{}{marker}", &text[..text.floor_char_boundary(keep)])
}

/// One ops row as stored: the body is JSON text.
struct OpRow {
    op_seq: i64,
    kind: String,
    ts: i64,
    body: String,
    batch: i64,
}

/// The most one op's body may take (spec 6.5, A42).
pub const MAX_OP_BYTES: usize = 64 << 10;
/// The most one append may hold, in ops and in body bytes together: an ops segment ends only
/// between appends, so a segment is at most `backup::SEGMENT_BYTES` plus one append's lines (each
/// body escaped once, and a few hundred bytes of fields per op), well within what a restore reads.
pub const MAX_BATCH_OPS: usize = 1024;
pub const MAX_BATCH_BYTES: usize = 4 << 20;

pub struct Raw {
    conn: Connection,
    device: String,
    home: std::path::PathBuf,
    /// The shared hold on `<home>/raw.lock` every open keeps (see `swap_lock`).
    _swap: std::fs::File,
}

/// The raw store file, including a stopped restore's file when raw.db has not been renamed in.
pub fn path(home: &Path) -> std::path::PathBuf {
    let path = home.join("raw.db");
    let restored = home.join("raw.db.restored");
    if !path.exists() && restored.exists() {
        restored
    } else {
        path
    }
}

/// Whether the home has a raw store: `raw.db`, or `raw.db.restored` alone (a restore stopped
/// mid-swap, which `open` finishes).
pub fn exists(home: &Path) -> bool {
    path(home).exists()
}

/// `<home>/raw.db`: WAL, synchronous=FULL (and fullfsync on macOS), 2 s SQLite busy timeout.
/// Non-hook opens share a 10 s initialization deadline; hooks use `open_within` with 2 s.
pub fn open(home: &Path) -> Result<Raw> {
    open_within(home, crate::db::OPEN_WRITE_WAIT)
}

/// Open with one lock-wait budget for restore, WAL, schema, column and device initialization.
/// Hooks pass 2 s so a failed open reaches MUST-M16's marker before the agent kills the hook.
pub fn open_within(home: &Path, wait: std::time::Duration) -> Result<Raw> {
    let deadline = std::time::Instant::now() + wait;
    let path = home.join("raw.db");
    crate::db::private(home, 0o700);
    let swap = swap_lock(
        home,
        false,
        OPEN_WAIT.min(deadline.saturating_duration_since(std::time::Instant::now())),
    )?;
    // A restore that stopped after moving the damaged file aside and before renaming the rebuilt
    // one in: `raw.db.restored` is only ever a whole rebuild (it gets that name once its records
    // are committed), so the rename is finished here instead of creating an empty store.
    let restored = self::path(home);
    if restored != path {
        // Failed, and no other open finished it: an error, never a new empty store beside it.
        if let Err(e) = std::fs::rename(&restored, &path)
            && !path.exists()
        {
            return Err(e).context("finish a stopped restore");
        }
    }
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(test)]
    crate::crash::arm(&conn);
    crate::db::wal_until(&conn, "FULL", deadline)?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON;")?;
    crate::db::ensure_schema_until(&conn, SCHEMA, deadline).context("raw schema")?;
    // A raw.db from before the ledger named its field (milestone 2 Task 1's schema).
    crate::db::ensure_column_until(
        &mut conn,
        "ledger",
        "field",
        "TEXT NOT NULL DEFAULT ''",
        deadline,
    )
    .context("migrate ledger")?;
    for file in ["raw.db", "raw.db-wal", "raw.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    crate::db::ensure_device_until(&conn, &path, deadline).context("device id")?;
    let device = conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
        r.get(0)
    })?;
    let raw = Raw {
        conn,
        device,
        home: home.to_owned(),
        _swap: swap,
    };
    raw.sync_privacy()?;
    Ok(raw)
}

/// `open`'s error when a restore still holds raw.db after `OPEN_WAIT`: a reader answers "try
/// again" (the viewer's 503, milestone 4 D11).
#[derive(Debug)]
pub struct Restoring;

impl std::fmt::Display for Restoring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("raw.db is being restored")
    }
}

impl std::error::Error for Restoring {}

/// Restore-lock waits: opens wait at most 2 s within their initialization budget (hooks 2 s,
/// other callers 10 s); a restore waits at most 10 s for open stores to close.
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const SWAP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Task 8: every open of raw.db holds `<home>/raw.lock` shared; a restore holds it exclusively
/// while it moves the damaged file aside and renames the rebuilt one in, so no writer keeps the
/// old file across the swap and loses its event there.
fn swap_lock(home: &Path, exclusive: bool, wait: std::time::Duration) -> Result<std::fs::File> {
    let path = home.join("raw.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    crate::db::private(&path, 0o600);
    let deadline = std::time::Instant::now() + wait;
    loop {
        let tried = if exclusive {
            f.try_lock()
        } else {
            f.try_lock_shared()
        };
        match tried {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) if exclusive => anyhow::bail!(
                "raw.db is open elsewhere; try again when agents and workers have stopped"
            ),
            Err(std::fs::TryLockError::WouldBlock) => return Err(Restoring.into()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// The exclusive hold a restore keeps while it swaps raw.db (released when dropped).
pub fn lock_for_swap(home: &Path) -> Result<std::fs::File> {
    swap_lock(home, true, SWAP_WAIT)
}

impl Raw {
    /// An accepted request may have outlived its raw transaction or an old backup restore.
    fn sync_privacy(&self) -> Result<()> {
        if crate::forget::needs_apply(&self.conn, &self.home)? {
            let tx = rusqlite::Transaction::new_unchecked(
                &self.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            crate::forget::apply(&tx, &self.home, &self.device)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub(crate) fn forget_preview(
        &self,
        target: crate::forget::Target,
    ) -> Result<crate::forget::Preview> {
        use rusqlite::OptionalExtension;
        self.sync_privacy()?;
        let (device, from, to) = target.bounds()?;
        anyhow::ensure!(
            device == self.device,
            "this first forget slice accepts only this device's raw records"
        );
        let version = privacy_version(&self.conn, &self.home, &self.device)?;
        let mut records = Vec::new();
        let mut sample = None;
        let mut at = from - 1;
        loop {
            let take = usize::try_from(to - at)
                .unwrap_or(usize::MAX)
                .min(crate::forget::MAX_RECORDS + 1);
            let batch = self.after_within(device, at, take, MAX_BATCH_BYTES)?;
            let Some(last) = batch.last() else { break };
            let last = last.seq;
            for r in batch.into_iter().filter(|r| r.seq <= to) {
                if let Item::Event(e) = r.item {
                    if sample.is_none() {
                        sample = Some(e.body.chars().take(120).collect());
                    }
                    let origin: Option<String> = self
                        .conn
                        .query_row(
                            "SELECT origin FROM import_origins WHERE device=?1 AND seq=?2",
                            params![device, r.seq],
                            |r| r.get(0),
                        )
                        .optional()?
                        .flatten();
                    records.push(crate::forget::Record {
                        device: device.into(),
                        seq: r.seq,
                        fingerprint: crate::forget::fingerprint(&e)?,
                        origin,
                    });
                    anyhow::ensure!(
                        records.len() <= crate::forget::MAX_RECORDS,
                        "split this selection into spans of at most {} records",
                        crate::forget::MAX_RECORDS
                    );
                }
            }
            if last >= to {
                break;
            }
            at = last;
        }
        anyhow::ensure!(
            version == privacy_version(&self.conn, &self.home, &self.device)?,
            "forget preview is stale; preview again"
        );
        Ok(crate::forget::Preview {
            target,
            version,
            records,
            sample,
        })
    }

    pub(crate) fn forget_start(
        &mut self,
        preview: &crate::forget::Preview,
    ) -> Result<crate::forget::Status> {
        preview.validate()?;
        let tx = begin_batch(&mut self.conn)?;
        anyhow::ensure!(
            preview.version == privacy_version(&tx, &self.home, &self.device)?,
            "forget preview is stale; preview again"
        );
        crate::forget::register(&self.home, preview)?;
        #[cfg(test)]
        if let Some(registered) = crate::forget::REGISTERED.get() {
            registered();
        }
        crate::forget::apply(&tx, &self.home, &self.device)?;
        let status = crate::forget::status(&self.home)?
            .pop()
            .context("registered forget request is missing")?;
        tx.commit()?;
        Ok(status)
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// SQLite's `quick_check` on raw.db: an error names the first problem it reports.
    pub fn quick_check(&self) -> Result<()> {
        crate::db::quick_check(&self.conn, "raw.db")
    }

    /// SQLite's full `integrity_check` (every page): doctor only.
    pub fn integrity_check(&self) -> Result<()> {
        let first: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        anyhow::ensure!(first == "ok", "raw.db integrity_check: {first}");
        Ok(())
    }

    /// Append one event as this device's next seq. The write lock taken by `BEGIN IMMEDIATE`
    /// makes reading the last seq and inserting the next one atomic across processes.
    pub fn append(&mut self, e: &Event) -> Result<i64> {
        self.append_with_ledger(e, &[], "")
    }

    /// `append`, with the event's redaction ledger rows in the same transaction; `ruleset` is the
    /// version of the rules that found them (`redact::Rules::version`).
    pub fn append_with_ledger(
        &mut self,
        e: &Event,
        ledger: &[(String, crate::redact::Finding)],
        ruleset: &str,
    ) -> Result<i64> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::forget::apply(&tx, &self.home, &self.device)?;
        let seq = insert_event(&tx, &self.device, e, ledger, ruleset)?;
        tx.commit()?;
        Ok(seq)
    }

    /// Imported records (`oboete-v1`, `transcript`; D6) with their ledger rows and, when given,
    /// the import's checkpoint as a `migration` op, in one transaction: a backup carries a batch
    /// with its checkpoint, and a killed import leaves neither. A record `denied` asks to leave out
    /// is not recorded; the checkpoint still moves past it. Refuses a record of a live source, and
    /// more than `IMPORT_BATCH` records or `MAX_BATCH_BYTES` of bodies. The seqs recorded.
    pub fn append_imported(
        &mut self,
        batch: &[crate::capture::Captured],
        ruleset: &str,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Vec<i64>> {
        self.append_imported_origins(batch, &[], ruleset, checkpoint)
    }

    /// The native source identity is hashed by the importer; fingerprints also protect old rows.
    pub fn append_imported_origins(
        &mut self,
        batch: &[crate::capture::Captured],
        origins: &[String],
        ruleset: &str,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Vec<i64>> {
        anyhow::ensure!(
            origins.is_empty() || origins.len() == batch.len(),
            "import identity count differs from records"
        );
        for origin in origins {
            crate::forget::check_identity(origin)?;
        }
        let bytes: usize = batch.iter().map(|c| c.event.body.len()).sum();
        anyhow::ensure!(
            batch.len() <= IMPORT_BATCH && bytes <= MAX_BATCH_BYTES,
            "an import of {} records and {bytes} bytes is over the cap of {IMPORT_BATCH} records and {MAX_BATCH_BYTES} bytes",
            batch.len()
        );
        if let Some(c) = batch.iter().find(|c| is_live(&c.event.source)) {
            anyhow::bail!("an import cannot record a {} record", c.event.source);
        }
        // No hook imports: the batch waits for another writer as a non-hook open does (#362).
        let tx = begin_batch(&mut self.conn, crate::db::OPEN_WRITE_WAIT)?;
        crate::forget::apply(&tx, &self.home, &self.device)?;
        if origins.is_empty() && batch.iter().any(|c| c.event.kind != "touch") {
            let has_denials: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM denied_records)", [], |r| {
                    r.get(0)
                })?;
            anyhow::ensure!(
                !has_denials,
                "cannot import raw without a native source identity after forget; use an importer that preserves provenance"
            );
        }
        let from_seq = next_seq(&tx, &self.device)?;
        let mut seqs = Vec::with_capacity(batch.len());
        for (index, c) in batch.iter().enumerate() {
            let fingerprint = crate::forget::fingerprint(&c.event)?;
            let origin = origins.get(index);
            if !denied(
                &tx,
                Some(&fingerprint),
                origin.map(String::as_str),
                &c.event.body,
            )? {
                let seq = insert_event(&tx, &self.device, &c.event, &c.ledger, ruleset)?;
                tx.execute(
                    "INSERT INTO import_origins(device,seq,origin,fingerprint) VALUES(?1,?2,?3,?4)",
                    params![self.device, seq, origin, fingerprint],
                )?;
                seqs.push(seq);
            }
        }
        if let Some(checkpoint) = checkpoint {
            let mut body = serde_json::to_value(checkpoint)?;
            // The batch's records, from_seq to to_seq with none between them under the write
            // lock: a restore that lost any of them drops this op (`Rebuild::finish`).
            body["from_seq"] = from_seq.into();
            body["to_seq"] = (next_seq(&tx, &self.device)? - 1).into();
            let op = within_batch_cap(&[(OpKind::Migration, body)])?;
            insert_ops(&tx, &self.device, &op)?;
        }
        tx.commit()?;
        Ok(seqs)
    }

    /// D8: a tombstone as this device's next seq. It hides its target in every later read: a
    /// whole record comes back as `Item::Removed`, a byte range of its body as `*`.
    pub fn append_tombstone(&mut self, t: Target) -> Result<i64> {
        let (device, seq, range) = match &t {
            Target::Record { device, seq } => (device, *seq, None),
            Target::Range {
                device,
                seq,
                offset,
                length,
            } => {
                anyhow::ensure!(*offset >= 0 && *length > 0, "a tombstone range is empty");
                (device, *seq, Some((*offset, *length)))
            }
        };
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let at = next_seq(&tx, &self.device)?;
        // ponytail: every tombstone is the redaction rescan's this milestone (D8); forget
        // (milestone 5) names its own source.
        tx.execute(
            "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                                 target_offset, target_length)
             VALUES(?1, ?2, 'tombstone', ?3, 'rescan', ?4, ?5, ?6, ?7)",
            params![
                self.device,
                at,
                crate::db::now_ms(),
                device,
                seq,
                range.map(|r| r.0),
                range.map(|r| r.1)
            ],
        )?;
        tx.commit()?;
        Ok(at)
    }

    /// How many tombstones raw.db holds: the viewer's `version` moves when one hides a record.
    pub fn tombstones(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM records WHERE type = 'tombstone'",
            [],
            |r| r.get(0),
        )?)
    }

    /// The targets of `device`'s tombstones after `seq`: what a reader must hide itself until
    /// its consumer has reached them.
    pub fn tombstones_after(&self, device: &str, seq: i64) -> Result<Vec<(String, i64)>> {
        self.sync_privacy()?;
        let mut st = self.conn.prepare(
            "SELECT target_device, target_seq FROM records
             WHERE device = ?1 AND seq > ?2 AND type = 'tombstone'",
        )?;
        let rows = st.query_map(params![device, seq], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// What tombstones remove from `device`'s records `from` to `to`, each once, in order. With
    /// `through`, only what `device`'s own tombstones up to that seq remove: what was gone before
    /// a read that started when its last record was `through` (docs/cards.md K4).
    pub fn removed_in(
        &self,
        device: &str,
        from: i64,
        to: i64,
        through: Option<i64>,
    ) -> Result<Vec<Removal>> {
        self.removed_of(device, None, from, to, through)
    }

    /// `removed_in` over one session's records (docs/summaries.md T7): a removal from another
    /// session's record between them is not counted; one from a record that is gone or has no
    /// labels is.
    pub fn removed_in_session(
        &self,
        device: &str,
        (agent, session): (&str, &str),
        from: i64,
        to: i64,
        through: Option<i64>,
    ) -> Result<Vec<Removal>> {
        self.removed_of(device, Some((agent, session)), from, to, through)
    }

    fn removed_of(
        &self,
        device: &str,
        session: Option<(&str, &str)>,
        from: i64,
        to: i64,
        through: Option<i64>,
    ) -> Result<Vec<Removal>> {
        // With a session: not a record of another session, nor an import of this one, which no
        // live turn rests on (Codex on #371).
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT DISTINCT t.target_seq, t.target_offset, t.target_length FROM records t
             WHERE t.type = 'tombstone' AND t.target_device = ?1
               AND t.target_seq BETWEEN ?2 AND ?3
               AND (?4 IS NULL OR t.device = ?1 AND t.seq <= ?4)
               AND (?5 IS NULL OR NOT EXISTS (
                 SELECT 1 FROM records r WHERE r.device = ?1 AND r.seq = t.target_seq
                   AND (r.agent IS NOT NULL AND r.session IS NOT NULL
                          AND (r.agent <> ?5 OR r.session <> ?6)
                        OR r.source NOT IN ('{}'))))
             ORDER BY 1, 2, 3",
            LIVE.join("', '")
        ))?;
        let (agent, session) = session.unzip();
        let rows = st.query_map(params![device, from, to, through, agent, session], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every device with records in this file: this one, and one a copied home left under its
    /// old id (`ensure_device`). A skip-scan over the primary key, one step per device.
    pub fn devices(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "WITH RECURSIVE d(device) AS (
               SELECT MIN(device) FROM records
               UNION ALL
               SELECT (SELECT MIN(device) FROM records WHERE device > d.device) FROM d
               WHERE d.device IS NOT NULL
             )
             SELECT device FROM d WHERE device IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// This device's highest seq, 0 for an empty store.
    pub fn max_seq(&self) -> Result<i64> {
        self.max_seq_of(&self.device)
    }

    /// `device`'s highest seq, 0 before its first record.
    pub fn max_seq_of(&self, device: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM records WHERE device = ?1",
            [device],
            |r| r.get(0),
        )?)
    }

    /// The first prompt this device recorded in one agent's session from a source `read` takes,
    /// with its seq, as `after` returns it: the session's goal for the curator (milestone 3
    /// Task 7), from a record its window may send (Codex on #304). A scan by label, as `turns`.
    pub fn first_prompt(
        &self,
        agent: &str,
        session: &str,
        read: impl Fn(&str) -> bool,
    ) -> Result<Option<(i64, Event)>> {
        let mut st = self.conn.prepare(
            "SELECT seq, source FROM records WHERE device = ?1 AND type = 'event' AND agent = ?2
               AND session = ?3 AND kind = 'prompt' ORDER BY seq",
        )?;
        let mut rows = st.query(rusqlite::params![self.device, agent, session])?;
        let mut seq = None;
        while let Some(r) = rows.next()? {
            if read(&r.get::<_, String>(1)?) {
                seq = Some(r.get::<_, i64>(0)?);
                break;
            }
        }
        let Some(seq) = seq else { return Ok(None) };
        Ok(self
            .after(&self.device, seq - 1, 1)?
            .into_iter()
            .find(|r| r.seq == seq)
            .and_then(|r| match r.item {
                Item::Event(e) => Some((seq, *e)),
                _ => None,
            }))
    }

    /// P5: a newly appended live prompt moves the viewer's marker before any consumer runs.
    /// The rowid tracks append order, including a replay whose record time is older. No text
    /// leaves here; body reads still go through `after`. A tombstone moves its own marker.
    pub fn prompt_version(&self) -> Result<i64> {
        self.conn.execute_batch(PROMPT_INDEX)?;
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM records
             WHERE type = 'event' AND kind = 'prompt'
               AND source IN (SELECT value FROM json_each(?1))",
            [serde_json::to_string(&LIVE)?],
            |r| r.get(0),
        )?)
    }

    /// page.md P2, P3: live-source prompts of a repository (or all), newest first after
    /// `before`. Only this viewer read creates the index, never a hook. Body reads still use
    /// `after`, including decompression and D8; hidden rows take no slot. One extra visible
    /// record may be inspected only for the exhaustion flag. The display cut is the API's.
    pub fn prompts(
        &self,
        repo: Option<&str>,
        before: Option<&PromptPosition>,
        limit: usize,
        rules: &crate::redact::Rules,
    ) -> Result<(Vec<Prompt>, bool)> {
        self.conn.execute_batch(PROMPT_INDEX)?;
        let mut st = self.conn.prepare(
            "SELECT device, seq FROM records
             WHERE type = 'event' AND kind = 'prompt'
               AND source IN (SELECT value FROM json_each(?1))
               AND (?2 IS NULL OR repo = ?2)
               AND (?3 IS NULL OR (ts, device, seq) < (?3, ?4, ?5))
             ORDER BY ts DESC, device DESC, seq DESC",
        )?;
        let mut rows = st.query(params![
            serde_json::to_string(&LIVE)?,
            repo,
            before.map(|p| p.0),
            before.map(|p| p.1.as_str()),
            before.map(|p| p.2)
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let (device, seq): (String, i64) = (r.get(0)?, r.get(1)?);
            let Some(record) = self
                .after(&device, seq - 1, 1)?
                .pop()
                .filter(|r| r.seq == seq)
            else {
                continue;
            };
            let Item::Event(e) = record.item else {
                continue;
            };
            if out.len() == limit {
                return Ok((out, true));
            }
            let gate = |s: &str| crate::redact::outbound_with(s, rules);
            out.push(Prompt {
                device,
                seq,
                ts: e.ts,
                agent: gate(&e.agent),
                repo: e.repo.as_deref().map(gate),
                text: gate(&crate::curate::long_text(&e).unwrap_or_default()),
            });
        }
        Ok((out, false))
    }

    /// The first record of the turn that `agent`'s `session` ends with its reply `reply` on this
    /// device: the session's first live event after its previous live reply, or its first live
    /// event (docs/summaries.md T1). A scan by label, as `turns`. Live alone: an import of the
    /// same session is no boundary of a live turn (Codex on #371).
    pub fn turn_start(&self, agent: &str, session: &str, reply: i64) -> Result<i64> {
        // The session's first event after its previous reply, not the record after that reply,
        // which may be another session's or a tombstone (Codex on C2). The reply at the latest.
        Ok(self.conn.query_row(
            &format!(
                "SELECT COALESCE(MIN(seq), ?4) FROM records
                 WHERE device = ?1 AND type = 'event' AND agent = ?2 AND session = ?3
                   AND seq <= ?4 AND source IN ('{live}')
                   AND seq > COALESCE(
                     (SELECT MAX(seq) FROM records WHERE device = ?1 AND type = 'event'
                        AND agent = ?2 AND session = ?3 AND kind = 'reply' AND seq < ?4
                        AND source IN ('{live}')), 0)",
                live = LIVE.join("', '")
            ),
            params![self.device, agent, session, reply],
            |r| r.get(0),
        )?)
    }

    /// Whether `device`'s events of `agent`'s `session` from `from` to `to` hold a live one: a
    /// window of imported records alone is no part of a live turn (Codex on #371).
    pub fn has_live(
        &self,
        device: &str,
        (agent, session): (&str, &str),
        from: i64,
        to: i64,
    ) -> Result<bool> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM records WHERE device = ?1 AND seq BETWEEN ?2 AND ?3
                   AND type = 'event' AND agent = ?4 AND session = ?5 AND source IN ('{}'))",
                LIVE.join("', '")
            ),
            params![device, from, to, agent, session],
            |r| r.get(0),
        )?)
    }

    /// The one repository of this device's live events of a session from `from` to `to`, none
    /// when they are of two (docs/summaries.md T5).
    pub fn session_repo(
        &self,
        (agent, session): (&str, &str),
        from: i64,
        to: i64,
    ) -> Result<Option<String>> {
        let (repos, repo): (i64, Option<String>) = self.conn.query_row(
            &format!(
                "SELECT COUNT(DISTINCT COALESCE(repo, char(0))), MIN(repo) FROM records
                 WHERE device = ?1 AND type = 'event' AND agent = ?2 AND session = ?3
                   AND seq BETWEEN ?4 AND ?5 AND source IN ('{}')",
                LIVE.join("', '")
            ),
            params![self.device, agent, session, from, to],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(repo.filter(|_| repos == 1))
    }

    /// The agent and session labels of `device`'s event `seq`, NUL between (how `curate` keys a
    /// session), from the row alone: no body is read.
    pub fn session_key(&self, device: &str, seq: i64) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let labels: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT agent, session FROM records WHERE device = ?1 AND seq = ?2
                   AND type = 'event'",
                params![device, seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(labels.map(|(a, s)| format!("{}\u{0}{}", a.unwrap_or_default(), s.unwrap_or_default())))
    }

    /// What the labels of `device`'s events `from` to `to` agree on, from the rows alone (no body
    /// is read): their agent and session when they are of one session, their repository when
    /// they are of one, and the time of the last of them (docs/cards.md K2).
    pub fn labels_in(&self, device: &str, from: i64, to: i64) -> Result<SpanLabels> {
        type Row = (
            i64,
            Option<String>,
            Option<String>,
            i64,
            Option<String>,
            Option<i64>,
        );
        let (sessions, agent, session, repos, repo, ts): Row = self.conn.query_row(
            "SELECT COUNT(DISTINCT COALESCE(agent, '') || char(0) || COALESCE(session, '')),
                    MIN(agent), MIN(session), COUNT(DISTINCT COALESCE(repo, char(0))), MIN(repo),
                    -- The last event's, by seq: times need not grow with it (a replay, a late
                    -- hook).
                    (SELECT ts FROM records WHERE device = ?1 AND seq BETWEEN ?2 AND ?3
                       AND type = 'event' ORDER BY seq DESC LIMIT 1)
             FROM records WHERE device = ?1 AND seq BETWEEN ?2 AND ?3 AND type = 'event'",
            params![device, from, to],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;
        Ok(SpanLabels {
            session: agent.zip(session).filter(|_| sessions == 1),
            repo: repo.filter(|_| repos == 1),
            ts,
        })
    }

    /// `device`'s event `seq`: its session as `session_key` spells it, and its source, from the
    /// row alone. What the embedding phase passes a record over for (milestone 4 D8, D13).
    pub fn event_labels(&self, device: &str, seq: i64) -> Result<Option<(String, String)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .prepare_cached(
                "SELECT COALESCE(agent, '') || char(0) || COALESCE(session, ''), source
                 FROM records WHERE device = ?1 AND seq = ?2 AND type = 'event'",
            )?
            .query_row(params![device, seq], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// This device's live replies (the sources `is_live` names) after `after` through `through`,
    /// the oldest first, at most `limit`, by their labels alone (no body): the turn ends a
    /// summary may be due for (docs/summaries.md T1). Along the primary key from `after`: records
    /// have no index of kind.
    pub fn replies_between(&self, after: i64, through: i64, limit: usize) -> Result<Vec<Labels>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT agent, session, repo, seq, ts, kind FROM records
             WHERE device = ?1 AND seq > ?2 AND seq <= ?3 AND type = 'event' AND kind = 'reply'
               AND source IN ('{}')
             ORDER BY seq LIMIT ?4",
            LIVE.join("', '")
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = st.query_map(params![self.device, after, through, limit], |r| {
            Ok(Labels {
                agent: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                session: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                repo: r.get(2)?,
                seq: r.get(3)?,
                ts: r.get(4)?,
                kind: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Typed prompts and replies of `agent`'s `session` on this device strictly between two seqs,
    /// of a `source` that `read` takes (a window's kind, milestone 4 D6): none when a proposal was
    /// the session's last turn before a window (milestone 3 Task 8).
    pub fn turns_between(
        &self,
        agent: &str,
        session: &str,
        after: i64,
        before: i64,
        read: impl Fn(&str) -> bool,
    ) -> Result<i64> {
        let mut st = self.conn.prepare_cached(
            "SELECT source FROM records WHERE device = ?1 AND seq > ?2 AND seq < ?3
               AND type = 'event' AND agent = ?4 AND session = ?5 AND kind IN ('prompt', 'reply')",
        )?;
        let mut n = 0;
        for source in st.query_map(params![self.device, after, before, agent, session], |r| {
            r.get::<_, String>(0)
        })? {
            n += i64::from(read(&source?));
        }
        Ok(n)
    }

    /// `agent`'s `session` events of `kind` on this device strictly between two seqs, as `after`
    /// returns them: picked by their labels, and only their bodies read.
    pub fn events_between(
        &self,
        agent: &str,
        session: &str,
        kind: &str,
        after: i64,
        before: i64,
    ) -> Result<Vec<Event>> {
        let seqs: Vec<i64> = self
            .conn
            .prepare(
                "SELECT seq FROM records WHERE device = ?1 AND seq > ?2 AND seq < ?3
                   AND type = 'event' AND agent = ?4 AND session = ?5 AND kind = ?6
                 ORDER BY seq",
            )?
            .query_map(
                params![self.device, after, before, agent, session, kind],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for seq in seqs {
            let record = self.after(&self.device, seq - 1, 1)?.into_iter().next();
            if let Some(Item::Event(e)) = record.filter(|r| r.seq == seq).map(|r| r.item) {
                out.push(*e);
            }
        }
        Ok(out)
    }

    /// The typed prompts and harness envelopes this device recorded in one agent's session (Task
    /// 11). A scan by label: sessions have no index (spec 1.6).
    pub fn turns(&self, agent: &str, session: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM records WHERE device = ?1 AND type = 'event' AND agent = ?2
               AND session = ?3 AND kind IN ('prompt', 'envelope')",
            rusqlite::params![self.device, agent, session],
            |r| r.get(0),
        )?)
    }

    /// Cursor's SessionEnd backfill counts stored prompt bodies by labels, never a session
    /// index (spec 1.6). Read through `after` so compressed and tombstoned records agree with
    /// every other reader; a copied home's earlier device counts too.
    pub fn prompt_counts(
        &self,
        agent: &str,
        session: &str,
    ) -> Result<std::collections::HashMap<String, usize>> {
        let mut st = self.conn.prepare(
            "SELECT device, seq FROM records
             WHERE type = 'event' AND kind = 'prompt' AND agent = ?1 AND session = ?2",
        )?;
        let rows = st.query_map(params![agent, session], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut counts = std::collections::HashMap::new();
        for row in rows {
            let (device, seq) = row?;
            for r in self.after(&device, seq - 1, 1)? {
                if let Item::Event(e) = r.item {
                    *counts.entry(e.body).or_insert(0) += 1;
                }
            }
        }
        Ok(counts)
    }

    /// Up to `limit` records of `device` after `seq`, in seq order.
    pub fn after(&self, device: &str, seq: i64, limit: usize) -> Result<Vec<Record>> {
        self.after_within(device, seq, limit, usize::MAX)
    }

    /// `after`, and fewer when their bodies, as read (decompressed), pass `max_bytes` together
    /// (always one): a reader in pages never holds much more than that at once (spec 3.1).
    pub fn after_within(
        &self,
        device: &str,
        seq: i64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<Record>> {
        self.sync_privacy()?;
        let mut st = self.conn.prepare(
            "SELECT device, seq, type, ts, kind, agent, session, repo, branch, head, gitdir, cwd,
                    source, body, original_bytes,
                    target_device, target_seq, target_offset, target_length, enc
             FROM records WHERE device = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = st.query_map(
            params![device, seq, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                let kind: String = r.get(2)?;
                let item = if kind == "removed" {
                    Item::Removed
                } else if kind == "tombstone" {
                    let (device, seq) = (r.get(15)?, r.get(16)?);
                    Item::Tombstone(match r.get::<_, Option<i64>>(17)? {
                        Some(offset) => Target::Range {
                            device,
                            seq,
                            offset,
                            length: r.get(18)?,
                        },
                        None => Target::Record { device, seq },
                    })
                } else {
                    let mut body: Vec<u8> = r.get(13)?;
                    // Task 10: no reader sees `enc`.
                    if r.get::<_, String>(19)? == "zstd" {
                        body = unzstd(&body).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                13,
                                rusqlite::types::Type::Blob,
                                Box::new(e),
                            )
                        })?;
                    }
                    Item::Event(Box::new(Event {
                        ts: r.get(3)?,
                        kind: r.get(4)?,
                        agent: r.get(5)?,
                        session: r.get(6)?,
                        repo: r.get(7)?,
                        branch: r.get(8)?,
                        head: r.get(9)?,
                        gitdir: r.get(10)?,
                        cwd: r.get(11)?,
                        source: r.get(12)?,
                        body: String::from_utf8_lossy(&body).into_owned(),
                        original_bytes: r.get(14)?,
                    }))
                };
                Ok(Record {
                    device: r.get(0)?,
                    seq: r.get(1)?,
                    item,
                })
            },
        )?;
        let (mut recs, mut bytes) = (Vec::new(), 0usize);
        for r in rows {
            let r = r?;
            let size = match &r.item {
                Item::Event(e) => e.body.len(),
                _ => 0,
            };
            if !recs.is_empty() && bytes.saturating_add(size) > max_bytes {
                break;
            }
            bytes = bytes.saturating_add(size);
            recs.push(r);
        }
        self.hide(device, &mut recs)?;
        Ok(recs)
    }

    /// Task 8: this device's records after `seq` as backup lines, one JSON object each, as
    /// `Raw::after` returns them (masked, D8) with each event's ledger rows, until `max_bytes`
    /// of lines (always one). Returns (seq, line) pairs in seq order.
    pub fn export_lines(&self, seq: i64, max_bytes: usize) -> Result<Vec<(i64, String)>> {
        let mut out = Vec::new();
        let (mut at, mut bytes) = (seq, 0);
        loop {
            let recs = self.after(&self.device, at, EXPORT_BATCH)?;
            let (Some(first), Some(last)) = (recs.first(), recs.last()) else {
                return Ok(out);
            };
            let mut ledger = self.ledger_between(first.seq, last.seq)?;
            // A ledger field is a JSON pointer, which names keys of the body: a record a tombstone
            // masks keeps its ledger rows without the field, so a masked key never reaches a
            // segment through them.
            let masked: std::collections::HashSet<i64> = self
                .conn
                .prepare(
                    "SELECT target_seq FROM records WHERE type = 'tombstone' AND target_device = ?1
                       AND target_seq BETWEEN ?2 AND ?3",
                )?
                .query_map(params![self.device, first.seq, last.seq], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for r in &recs {
                let mut rows = ledger.remove(&r.seq).unwrap_or_default();
                if masked.contains(&r.seq) {
                    for row in &mut rows {
                        row["field"] = serde_json::json!("~tombstoned");
                    }
                }
                let mut line = line(r, rows);
                if matches!(r.item, Item::Event(_)) {
                    use rusqlite::OptionalExtension;
                    let identity: Option<(Option<String>, String)> = self.conn.query_row(
                        "SELECT origin,fingerprint FROM import_origins WHERE device=?1 AND seq=?2",
                        params![r.device,r.seq], |r| Ok((r.get(0)?,r.get(1)?)),
                    ).optional()?;
                    if let Some((origin, fingerprint)) = identity {
                        let mut value: serde_json::Value = serde_json::from_str(&line)?;
                        value["import_identity"] =
                            serde_json::json!({"origin":origin,"fingerprint":fingerprint});
                        line = serde_json::to_string(&value)?;
                    }
                }
                bytes += line.len() + 1;
                out.push((r.seq, line));
                at = r.seq;
                if bytes >= max_bytes {
                    return Ok(out);
                }
            }
        }
    }

    /// D1: `ops` as this device's next op seqs, in one transaction: a window op and the claims
    /// it yields commit together, and with them the curation checkpoint (D2).
    pub fn append_ops(&mut self, ops: &[(OpKind, serde_json::Value)]) -> Result<Vec<i64>> {
        let bodies = within_batch_cap(ops)?;
        let tx = begin_batch(&mut self.conn, Duration::ZERO)?;
        crate::forget::apply(&tx, &self.home, &self.device)?;
        for (kind, body) in ops {
            anyhow::ensure!(
                !forgotten_op(&tx, &self.device, *kind, body)?,
                "a forget request invalidated this derived batch; it was not recorded"
            );
        }
        let seqs = insert_ops(&tx, &self.device, &bodies)?;
        tx.commit()?;
        Ok(seqs)
    }

    /// Up to `limit` ops of `device` after `op_seq`, in op_seq order.
    pub fn ops_after(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<Op>> {
        self.sync_privacy()?;
        self.op_rows(device, op_seq, limit)?
            .into_iter()
            .map(|r| {
                Ok(Op {
                    device: device.to_owned(),
                    op_seq: r.op_seq,
                    kind: OpKind::from_name(&r.kind)
                        .with_context(|| format!("op {}: unknown type {:?}", r.op_seq, r.kind))?,
                    ts: r.ts,
                    body: serde_json::from_str(&r.body)
                        .with_context(|| format!("op {}: body", r.op_seq))?,
                    batch: r.batch,
                })
            })
            .collect()
    }

    fn op_rows(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<OpRow>> {
        let mut st = self.conn.prepare(
            "SELECT op_seq, type, ts, body, batch FROM ops WHERE device = ?1 AND op_seq > ?2
             ORDER BY op_seq LIMIT ?3",
        )?;
        let rows = st.query_map(
            params![device, op_seq, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                Ok(OpRow {
                    op_seq: r.get(0)?,
                    kind: r.get(1)?,
                    ts: r.get(2)?,
                    body: r.get(3)?,
                    batch: r.get(4)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The repositories excluded now (spec 5.5, milestone 4 D13): every device's exclusion ops in
    /// time order, an undo taking its repository back out. Read before each outbound call, with no
    /// consumer in between; with no hub, this device's list is the whole list.
    pub fn exclusions(&self) -> Result<Vec<String>> {
        // A device's ops in its own order (op_seq), its clock never going back in it, then every
        // device's by that clock: a clock set back never puts a newer op first, and an op
        // `exclude` wrote comes after every op its store held (Codex on #304).
        let mut st = self.conn.prepare(
            "SELECT device, ts, body FROM ops WHERE type = 'exclusion' ORDER BY device, op_seq",
        )?;
        let mut ops: Vec<(i64, String, usize, serde_json::Value)> = Vec::new();
        let (mut device, mut clock) = (String::new(), i64::MIN);
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for (i, row) in rows.enumerate() {
            let (from, ts, body): (String, i64, String) = row?;
            let v: serde_json::Value = serde_json::from_str(&body)?;
            if from != device {
                (device, clock) = (from.clone(), i64::MIN);
            }
            clock = clock.max(v["clock"].as_i64().unwrap_or(ts));
            ops.push((clock, from, i, v));
        }
        ops.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
        let mut out = std::collections::BTreeSet::new();
        for (_, _, _, v) in ops {
            let Some(repo) = v["repo"].as_str() else {
                continue;
            };
            if v["undo"] == true {
                out.remove(repo);
            } else {
                out.insert(repo.to_owned());
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Appends an exclusion op, or its undo (spec 5.5, D13), with a clock past every exclusion op
    /// this store holds, as a Lamport clock: it takes effect after each op its device had seen,
    /// whatever the clocks of the devices that wrote them. A copied store keeps its old device's
    /// ops under a new device (Codex on #304).
    pub fn exclude(&mut self, repo: &str, undo: bool) -> Result<i64> {
        let seen: Option<i64> = self.conn.query_row(
            "SELECT MAX(COALESCE(json_extract(body, '$.clock'), ts)) FROM ops
             WHERE type = 'exclusion'",
            [],
            |r| r.get(0),
        )?;
        let clock = seen.map_or(i64::MIN, |c| c + 1).max(crate::db::now_ms());
        let op = serde_json::json!({ "repo": repo, "undo": undo, "clock": clock });
        Ok(self.append_ops(&[(OpKind::Exclusion, op)])?[0])
    }

    /// The sessions, as `agent` NUL `session`, with a record in one of `repos`: what an excluded
    /// repository's session touched is sent nowhere (spec 5.5), whatever else it touched.
    /// Read before each outbound call, so through an index of the events' repositories, built the
    /// first time any exclusion is read: in the worker or the CLI, never in a hook, which then only
    /// adds to it (CodeRabbit on #304).
    pub fn sessions_in(&self, repos: &[String]) -> Result<std::collections::HashSet<String>> {
        if repos.is_empty() {
            return Ok(Default::default());
        }
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS records_repo ON records(repo) WHERE type = 'event'",
        )?;
        let mut st = self.conn.prepare(
            "SELECT DISTINCT COALESCE(agent, '') || char(0) || COALESCE(session, '') FROM records
             WHERE type = 'event' AND repo IN (SELECT value FROM json_each(?1))",
        )?;
        let rows = st.query_map([serde_json::to_string(repos)?], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Per imported source (not `is_live`), `device`'s events in the seq ranges `ranges`, each
    /// inclusive.
    pub fn imported_counts(
        &self,
        device: &str,
        ranges: &[(i64, i64)],
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let mut st = self.conn.prepare(
            "SELECT source, count(*) FROM records
             WHERE device = ?1 AND seq BETWEEN ?2 AND ?3 AND type = 'event' AND kind != 'touch'
             GROUP BY source",
        )?;
        let mut out = std::collections::BTreeMap::new();
        for &(from, to) in ranges {
            for row in st.query_map(params![device, from, to], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (source, n) = row?;
                if !is_live(&source) {
                    *out.entry(source).or_default() += n;
                }
            }
        }
        Ok(out)
    }

    /// The bodies of `device`'s ops of `kind` after `op_seq`, in op_seq order: what a reader checks
    /// that the worker has not applied yet (`claims::Pending`).
    pub fn ops_of(
        &self,
        kind: OpKind,
        device: &str,
        op_seq: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let mut st = self.conn.prepare(
            "SELECT op_seq, body FROM ops WHERE device = ?1 AND op_seq > ?2 AND type = ?3
             ORDER BY op_seq",
        )?;
        let rows = st.query_map(params![device, op_seq, kind.name()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (op_seq, body) = r?;
            serde_json::from_str(&body).with_context(|| format!("op {op_seq}: body"))
        })
        .collect()
    }

    /// This device's highest op seq, 0 before its first op.
    pub fn max_op_seq(&self) -> Result<i64> {
        self.max_op_seq_of(&self.device)
    }

    /// `device`'s highest op seq, 0 before its first op.
    pub fn max_op_seq_of(&self, device: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(op_seq), 0) FROM ops WHERE device = ?1",
            [device],
            |r| r.get(0),
        )?)
    }

    /// The `source_id`s of `source`'s documents imported so far, on any device: what an import
    /// skips (D5). An op whose `source_id` is missing or not text, which this version cannot read,
    /// is passed over as the consumer passes it (OpenCodeReview on #305).
    pub fn import_keys(&self, source: &str) -> Result<std::collections::HashSet<String>> {
        let mut st = self.conn.prepare(
            "SELECT json_extract(body, '$.source_id') FROM ops
             WHERE type = 'import' AND json_extract(body, '$.source') = ?1
               AND typeof(json_extract(body, '$.source_id')) = 'text'",
        )?;
        let rows = st.query_map([source], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The furthest checkpoint of each import whose key starts with `prefix`, on any device: where
    /// its next pass starts (D6).
    pub fn migration_checkpoints(
        &self,
        prefix: &str,
    ) -> Result<std::collections::HashMap<String, Checkpoint>> {
        let mut st = self.conn.prepare(
            "SELECT op_seq, body FROM ops WHERE type = 'migration'
               AND substr(json_extract(body, '$.key'), 1, length(?1)) = ?1",
        )?;
        let rows = st.query_map([prefix], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = std::collections::HashMap::<String, Checkpoint>::new();
        for row in rows {
            let (op_seq, body) = row?;
            let c: Checkpoint = serde_json::from_str(&body)
                .with_context(|| format!("op {op_seq}: a migration body"))?;
            if out.get(&c.key).is_none_or(|kept| kept.through < c.through) {
                out.insert(c.key.clone(), c);
            }
        }
        Ok(out)
    }

    /// The sessions of `source`'s records, its `touch` records left out (D6: the sessions a v1
    /// pass checks oboete.db still holds).
    pub fn sessions_of(&self, source: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT DISTINCT session FROM records WHERE type = 'event' AND source = ?1
               AND kind != 'touch' AND session IS NOT NULL ORDER BY session",
        )?;
        let rows = st.query_map([source], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The (session, repo) labels of `source`'s `touch` records (D6): the `session_repos` rows a
    /// v1 pass has recorded.
    pub fn touches(
        &self,
        source: &str,
    ) -> Result<std::collections::HashSet<(String, Option<String>)>> {
        let mut st = self.conn.prepare(
            "SELECT session, repo FROM records WHERE type = 'event' AND source = ?1
               AND kind = 'touch'",
        )?;
        let rows = st.query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The time of each (agent, session)'s earliest record that neither the transcript import
    /// wrote nor labels a v1 repository (`touch`): where that session's transcript import stops
    /// (D6).
    pub fn earliest_by_session(&self) -> Result<std::collections::HashMap<(String, String), i64>> {
        let mut st = self.conn.prepare(
            "SELECT agent, session, MIN(ts) FROM records
             WHERE type = 'event' AND source != 'transcript' AND kind != 'touch'
               AND agent IS NOT NULL AND session IS NOT NULL
             GROUP BY agent, session",
        )?;
        let rows = st.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `docs` as this device's `import` ops (D5), in appends of at most `IMPORT_BATCH` documents
    /// and `MAX_BATCH_BYTES`, each its own batch: an import stopped midway keeps what it appended.
    /// A body that would take its op over `MAX_OP_BYTES` is clipped with a marker; a document
    /// `denied` asks to leave out is not appended. The ops appended.
    pub fn append_imports(&mut self, docs: Vec<ImportDoc>) -> Result<usize> {
        self.sync_privacy()?;
        let (mut batch, mut bytes, mut appended) = (Vec::new(), 0, 0);
        for doc in docs {
            let text = format!("{}\n{}", doc.title, doc.body);
            if denied(
                &self.conn,
                None,
                Some(&crate::forget::origin(&doc.source, &doc.source_id)),
                &text,
            )? {
                continue;
            }
            let body = serde_json::to_value(within_op_cap(doc)?)?;
            let size = body.to_string().len();
            if batch.len() == IMPORT_BATCH || bytes + size > MAX_BATCH_BYTES {
                appended += self.append_ops(&std::mem::take(&mut batch))?.len();
                bytes = 0;
            }
            bytes += size;
            batch.push((OpKind::Import, body));
        }
        if !batch.is_empty() {
            appended += self.append_ops(&batch)?.len();
        }
        Ok(appended)
    }

    /// The devices that have ops (this one's, and from milestone 6 other devices' through sync),
    /// read each pass: a skip-scan over the primary key, one step per device, as `devices`.
    pub fn op_devices(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "WITH RECURSIVE d(device) AS (
               SELECT MIN(device) FROM ops
               UNION ALL
               SELECT (SELECT MIN(device) FROM ops WHERE device > d.device) FROM d
               WHERE d.device IS NOT NULL
             )
             SELECT device FROM d WHERE device IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// D9: the time of this device's newest record a hook wrote, walking down from the top past
    /// tombstones, imports and replays, which never say the owner is at work.
    pub fn last_hook_ts(&self) -> Result<Option<i64>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT ts FROM records WHERE device = ?1 AND type = 'event' AND source = 'hook'
                 ORDER BY seq DESC LIMIT 1",
                [&self.device],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// D2: where `device`'s curation has reached, as (seq, offset): the end of its last window
    /// op that is not a recuration. An offset is where the next window starts inside an event a
    /// window split; none means after the whole event. (0, None) before the first window.
    pub fn curation_checkpoint(&self, device: &str) -> Result<(i64, Option<i64>)> {
        use rusqlite::OptionalExtension;
        let last = self
            .conn
            .query_row(
                "SELECT op_seq, json_extract(body, '$.to_seq'), json_extract(body, '$.to_offset')
                 FROM ops WHERE device = ?1 AND type = 'window'
                   AND COALESCE(json_extract(body, '$.recurate'), 0) = 0
                 ORDER BY op_seq DESC LIMIT 1",
                [device],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?;
        match last {
            None => Ok((0, None)),
            Some((_, Some(seq), offset)) => Ok((seq, offset)),
            Some((op_seq, None, _)) => anyhow::bail!("window op {op_seq} has no to_seq"),
        }
    }

    /// The (from_seq, to_seq) of the window op appended together with `device`'s op `op_seq`:
    /// the window a claim op's quotes were found in.
    pub fn window_of(&self, device: &str, op_seq: i64) -> Result<Option<(i64, i64)>> {
        use rusqlite::OptionalExtension;
        let span: Option<(Option<i64>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT json_extract(w.body, '$.from_seq'), json_extract(w.body, '$.to_seq')
                 FROM ops o JOIN ops w
                   ON w.device = o.device AND w.batch = o.batch AND w.type = 'window'
                 WHERE o.device = ?1 AND o.op_seq = ?2 LIMIT 1",
                params![device, op_seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match span {
            Some((Some(from), Some(to))) => Some((from, to)),
            _ => None,
        })
    }

    /// The claim ops of the session's previous window, whose proposals its next window carries
    /// (milestone 3 Task 7, D12): the last window the worker cut that covered this device's latest
    /// event of `agent`'s `session` before the window starting at `from` (seq and offset) that has
    /// text (a resumed session's `start` is covered on its own, by a window that holds none of its
    /// lines), up to `from` when a recuration cuts the records another way. Its claims and those
    /// of every later window op quoted inside it (#240): the newest window first, each window's
    /// claims in the order it wrote them. The window starting at `from`, curated before and now
    /// again, is not a previous one. Per session, since sessions interleave: another session's
    /// window may come between.
    pub fn previous_window_ops(
        &self,
        agent: &str,
        session: &str,
        from: (i64, Option<i64>),
        read: impl Fn(&str) -> bool,
    ) -> Result<Vec<Op>> {
        // A window that starts inside an event: its first part was in the previous window.
        let before = from.0 + i64::from(from.1.is_some());
        let start = (from.0, from.1.unwrap_or(0));
        // Down the primary key from `before`: the session's latest event of a kind `read` takes
        // (a window's, milestone 4 D6) is usually close.
        let mut latest = self.conn.prepare_cached(
            "SELECT seq, source FROM records WHERE device = ?1 AND seq < ?2 AND type = 'event'
               AND agent = ?3 AND session = ?4 AND kind NOT IN ('start', 'end')
             ORDER BY seq DESC",
        )?;
        let mut rows = latest.query(params![self.device, before, agent, session])?;
        let mut seq = None;
        while let Some(r) = rows.next()? {
            if read(&r.get::<_, String>(1)?) {
                seq = Some(r.get::<_, i64>(0)?);
                break;
            }
        }
        drop(rows);
        let Some(seq) = seq else {
            return Ok(Vec::new());
        };
        // Down from the newest window op that covered the event to the first the worker cut: its
        // recurations come after it, and the windows before it (another part of a split event)
        // end where it starts. Ranges as positions (seq, offset): the first byte, and past the
        // last.
        let mut stmt = self.conn.prepare(
            "SELECT op_seq, COALESCE(json_extract(body, '$.recurate'), 0),
                    json_extract(body, '$.from_seq'), COALESCE(json_extract(body, '$.from_offset'), 0),
                    json_extract(body, '$.to_seq'), COALESCE(json_extract(body, '$.to_offset'), ?3)
             FROM ops WHERE device = ?1 AND type = 'window'
               AND json_extract(body, '$.from_seq') <= ?2
               AND json_extract(body, '$.to_seq') >= ?2
             ORDER BY op_seq DESC",
        )?;
        let mut rows = stmt.query(params![self.device, seq, i64::MAX])?;
        let mut cut = None;
        while let Some(r) = rows.next()? {
            let (first, end): ((i64, i64), (i64, i64)) =
                ((r.get(2)?, r.get(3)?), (r.get(4)?, r.get(5)?));
            // The window starting at `from` is this one, curated before; one that runs past it
            // (a recuration cut the records another way) is the previous window up to it.
            if first >= start {
                continue;
            }
            if !r.get::<_, bool>(1)? {
                cut = Some((r.get::<_, i64>(0)?, (first, end.min(start))));
                break;
            }
        }
        let Some((since, (cut_from, cut_to))) = cut else {
            return Ok(Vec::new());
        };
        // Its claims, and those every later window op quoted inside it: a recuration of a part
        // of it holding the event or not, or one that cut the records another way. By the first
        // quote, as the claim's uid is, whole, as `curate::anchored_in` reads a window's.
        let batches: Vec<i64> = self
            .conn
            .prepare(
                "SELECT batch FROM ops WHERE device = ?1 AND type = 'window' AND op_seq >= ?2
                   AND json_extract(body, '$.from_seq') <= ?3
                   AND json_extract(body, '$.to_seq') >= ?4
                 ORDER BY op_seq DESC",
            )?
            .query_map(params![self.device, since, cut_to.0, cut_from.0], |r| {
                r.get(0)
            })?
            .collect::<rusqlite::Result<_>>()?;
        let inside = |o: &Op| {
            let e = &o.body["evidence"][0];
            e["device"].as_str() == Some(self.device.as_str())
                && matches!((e["seq"].as_i64(), e["offset"].as_i64(), e["length"].as_i64()),
                    (Some(s), Some(f), Some(n)) if cut_from <= (s, f) && (s, f + n) <= cut_to)
        };
        let mut ops = Vec::new();
        for batch in batches {
            ops.extend(
                self.ops_after(&self.device, batch - 1, MAX_BATCH_OPS)?
                    .into_iter()
                    .take_while(|o| o.batch == batch)
                    .filter(|o| o.kind == OpKind::Claim && inside(o)),
            );
        }
        Ok(ops)
    }

    /// D1: this device's ops after `op_seq` as backup lines, from `max_bytes` of lines on only
    /// up to the end of an append: a segment never holds a window op without the claims that
    /// committed with it. The body stays the stored JSON text, so a restore gives back the same
    /// bytes.
    pub fn export_op_lines(&self, op_seq: i64, max_bytes: usize) -> Result<Vec<(i64, String)>> {
        let (mut out, mut at, mut bytes) = (Vec::new(), op_seq, 0);
        let mut last_batch = None;
        loop {
            let rows = self.op_rows(&self.device, at, EXPORT_BATCH)?;
            if rows.is_empty() {
                return Ok(out);
            }
            for r in rows {
                if bytes >= max_bytes && last_batch != Some(r.batch) {
                    return Ok(out);
                }
                let line = serde_json::json!({"device": self.device, "op_seq": r.op_seq,
                    "type": r.kind, "ts": r.ts, "body": r.body, "batch": r.batch})
                .to_string();
                bytes += line.len() + 1;
                out.push((r.op_seq, line));
                (at, last_batch) = (r.op_seq, Some(r.batch));
            }
        }
    }

    /// Task 8: the sha256 of each of this device's records as its backup line, so a restore can
    /// be compared with what was backed up (compression does not change it).
    #[cfg(test)]
    pub fn hashes(&self) -> Result<std::collections::BTreeMap<(String, i64), String>> {
        use sha2::{Digest, Sha256};
        Ok(self
            .export_lines(0, usize::MAX)?
            .into_iter()
            .map(|(seq, l)| {
                let h = Sha256::digest(l.as_bytes());
                let hex = h.iter().map(|b| format!("{b:02x}")).collect();
                ((self.device.clone(), seq), hex)
            })
            .collect())
    }

    /// This device's ledger rows for seqs from `first` through `last`, by seq.
    fn ledger_between(
        &self,
        first: i64,
        last: i64,
    ) -> Result<std::collections::HashMap<i64, Vec<serde_json::Value>>> {
        let mut st = self.conn.prepare(
            "SELECT seq, field, rule, offset, length, ts, ruleset FROM ledger
             WHERE device = ?1 AND seq BETWEEN ?2 AND ?3 ORDER BY seq, rowid",
        )?;
        let mut out: std::collections::HashMap<i64, Vec<serde_json::Value>> = Default::default();
        let rows = st.query_map(params![self.device, first, last], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                serde_json::json!({
                    "field": r.get::<_, String>(1)?, "rule": r.get::<_, String>(2)?,
                    "offset": r.get::<_, i64>(3)?, "length": r.get::<_, i64>(4)?,
                    "ts": r.get::<_, i64>(5)?, "ruleset": r.get::<_, String>(6)?,
                }),
            ))
        })?;
        for row in rows {
            let (seq, v) = row?;
            out.entry(seq).or_default().push(v);
        }
        Ok(out)
    }

    /// D8, in the one place records leave: each event a tombstone targets whole becomes
    /// `Item::Removed`; each range it targets becomes as many `*` as the bytes of every character
    /// it touches, so offsets stay valid and hiding twice changes nothing. The tombstones are
    /// loaded once per read, by the seqs the read returns.
    fn hide(&self, device: &str, recs: &mut [Record]) -> Result<()> {
        let (Some(first), Some(last)) = (recs.first(), recs.last()) else {
            return Ok(());
        };
        let mut st = self.conn.prepare(
            "SELECT target_seq, target_offset, target_length FROM records
             WHERE type = 'tombstone' AND target_device = ?1 AND target_seq BETWEEN ?2 AND ?3",
        )?;
        let targets: Vec<(i64, Option<i64>, Option<i64>)> = st
            .query_map(params![device, first.seq, last.seq], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (seq, offset, length) in targets {
            let Some(r) = recs.iter_mut().find(|r| r.seq == seq) else {
                continue;
            };
            let Item::Event(e) = &mut r.item else {
                continue; // a tombstone of a tombstone hides nothing
            };
            match (offset, length) {
                (Some(o), Some(l)) => e.body = masked(&e.body, o, l),
                _ => r.item = Item::Removed,
            }
        }
        Ok(())
    }

    /// Task 10 (spec 2.4): the plain event bodies of `device` after `after` through `through`,
    /// rewritten as zstd (level 3) where that is smaller; (device, seq) and every label stay.
    /// Bodies are read and compressed outside the write lock, then written in one short
    /// transaction per batch, so a hook waits on it no longer than on another hook. Returns how
    /// many were rewritten.
    pub fn compress_through(&self, device: &str, after: i64, through: i64) -> Result<usize> {
        let mut from = after;
        let mut rewritten = 0;
        loop {
            // Sizes first: a batch loads at most `COMPRESS_BATCH` bodies and `BATCH_BYTES` of them
            // (always one), and a body above `MAX_BODY_BYTES` is never loaded: it stays plain, as
            // capture caps each string, not a whole body, and a compressed body must read back.
            let sizes: Vec<(i64, i64)> = self
                .conn
                .prepare(
                    "SELECT seq, length(body) FROM records WHERE device = ?1 AND seq > ?2
                       AND seq <= ?3 AND type = 'event' AND enc = 'plain' ORDER BY seq LIMIT ?4",
                )?
                .query_map(params![device, from, through, COMPRESS_BATCH as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if sizes.is_empty() {
                return Ok(rewritten);
            }
            let (mut upto, mut bytes, mut chosen) = (from, 0u64, Vec::new());
            for (seq, len) in sizes {
                let len = len as u64;
                if len <= MAX_BODY_BYTES {
                    if !chosen.is_empty() && bytes + len > BATCH_BYTES {
                        break;
                    }
                    bytes += len;
                    chosen.push(seq);
                }
                upto = seq;
            }
            let mut smaller = Vec::new();
            for seq in chosen {
                let body: Vec<u8> = self.conn.query_row(
                    "SELECT body FROM records WHERE device = ?1 AND seq = ?2",
                    params![device, seq],
                    |r| r.get(0),
                )?;
                let z = zstd::bulk::compress(&body, 3)?;
                if z.len() < body.len() {
                    smaller.push((seq, z));
                }
            }
            let tx = rusqlite::Transaction::new_unchecked(
                &self.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            for (seq, z) in &smaller {
                rewritten += tx.execute(
                    "UPDATE records SET body = ?1, enc = 'zstd'
                     WHERE device = ?2 AND seq = ?3 AND enc = 'plain'",
                    params![z, device, seq],
                )?;
            }
            tx.commit()?;
            from = upto;
        }
    }
}

/// Records `export_lines` reads at once.
const EXPORT_BATCH: usize = 500;

/// One record as a backup line. The key order is fixed (serde_json keeps insertion order), so
/// the same record always gives the same bytes.
fn line(r: &Record, ledger: Vec<serde_json::Value>) -> String {
    let v = match &r.item {
        Item::Event(e) => serde_json::json!({
            "device": r.device, "seq": r.seq, "type": "event", "ts": e.ts, "kind": e.kind,
            "agent": e.agent, "session": e.session, "repo": e.repo, "branch": e.branch,
            "head": e.head, "gitdir": e.gitdir, "cwd": e.cwd, "source": e.source,
            "body": e.body, "original_bytes": e.original_bytes, "ledger": ledger,
        }),
        Item::Removed => serde_json::json!({"device": r.device, "seq": r.seq, "type": "removed"}),
        Item::Tombstone(t) => {
            let (device, seq, offset, length) = match t {
                Target::Record { device, seq } => (device, seq, None, None),
                Target::Range {
                    device,
                    seq,
                    offset,
                    length,
                } => (device, seq, Some(offset), Some(length)),
            };
            serde_json::json!({"device": r.device, "seq": r.seq, "type": "tombstone",
                "target": {"device": device, "seq": seq, "offset": offset, "length": length}})
        }
    };
    v.to_string()
}

/// Task 8: a new raw.db at `path` (which must not exist) with `device`'s id, filled from backup
/// lines. The file keeps the id after it is renamed into place: its identity (`meta.store_file`)
/// is the file's own, which a rename keeps.
pub struct Rebuild {
    conn: Connection,
    privacy: Option<(std::path::PathBuf, String)>,
}

impl Rebuild {
    /// Restore applies bodyless controls before making its rebuilt file visible.
    pub fn apply_privacy(&mut self, home: &Path, device: &str) {
        self.privacy = Some((home.to_owned(), device.to_owned()));
    }

    pub fn new(path: &Path, device: &str) -> Result<Self> {
        anyhow::ensure!(!path.exists(), "{} exists", path.display());
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        crate::db::ensure_device(&conn, path)?;
        conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'device_id'",
            [device],
        )?;
        conn.execute_batch("BEGIN")?;
        Ok(Self {
            conn,
            privacy: None,
        })
    }

    /// One backup line. Bodies are stored as zstd where that is smaller, as the compress
    /// consumer would have; a tombstone gets no time or source back (`Raw::after` never
    /// returns them): ts 0, source `restore`.
    pub fn add(&mut self, line: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
        let i = |k: &str| v.get(k).and_then(serde_json::Value::as_i64);
        let (device, seq) = (s("device").context("device")?, i("seq").context("seq")?);
        match s("type") {
            Some("event") => {
                let body = s("body").unwrap_or("").as_bytes();
                let z = zstd::bulk::compress(body, 3)?;
                let (enc, stored) = if z.len() < body.len() {
                    ("zstd", z.as_slice())
                } else {
                    ("plain", body)
                };
                self.conn.execute(
                    "INSERT INTO records(device, seq, type, ts, kind, agent, session, repo, branch,
                       head, gitdir, cwd, source, enc, body, original_bytes)
                     VALUES(?1, ?2, 'event', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        device,
                        seq,
                        i("ts").unwrap_or(0),
                        s("kind").unwrap_or(""),
                        s("agent").unwrap_or(""),
                        s("session").unwrap_or(""),
                        s("repo"),
                        s("branch"),
                        s("head"),
                        s("gitdir"),
                        s("cwd"),
                        s("source").unwrap_or(""),
                        enc,
                        stored,
                        i("original_bytes")
                    ],
                )?;
                for l in v["ledger"].as_array().into_iter().flatten() {
                    let ls = |k: &str| l.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
                    let li = |k: &str| l.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
                    self.conn.execute(
                        "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            device,
                            seq,
                            ls("field"),
                            ls("rule"),
                            li("offset"),
                            li("length"),
                            li("ts"),
                            ls("ruleset")
                        ],
                    )?;
                }
                if let Some(identity) = v.get("import_identity") {
                    let fingerprint = identity["fingerprint"]
                        .as_str()
                        .context("import identity fingerprint")?;
                    let origin = identity["origin"].as_str();
                    crate::forget::check_identity(fingerprint)?;
                    if let Some(origin) = origin {
                        crate::forget::check_identity(origin)?;
                    }
                    self.conn.execute("INSERT INTO import_origins(device,seq,origin,fingerprint) VALUES(?1,?2,?3,?4)", params![device,seq,origin,fingerprint])?;
                }
            }
            Some("removed") => {
                self.conn.execute(
                    "INSERT INTO records(device, seq, type, ts, source) VALUES(?1, ?2, 'removed', 0, 'restore')",
                    params![device, seq],
                )?;
            }
            Some("tombstone") => {
                let t = &v["target"];
                self.conn.execute(
                    "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                       target_offset, target_length)
                     VALUES(?1, ?2, 'tombstone', 0, 'restore', ?3, ?4, ?5, ?6)",
                    params![
                        device,
                        seq,
                        t["device"].as_str().context("target device")?,
                        t["seq"].as_i64().context("target seq")?,
                        t["offset"].as_i64(),
                        t["length"].as_i64()
                    ],
                )?;
            }
            other => anyhow::bail!("seq {seq}: unknown record type {other:?}"),
        }
        Ok(())
    }

    /// One op backup line (D1).
    pub fn add_op(&mut self, line: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let op_seq = v["op_seq"].as_i64().context("op_seq")?;
        let kind = v["type"]
            .as_str()
            .and_then(OpKind::from_name)
            .with_context(|| format!("op {op_seq}: unknown type"))?;
        self.conn.execute(
            "INSERT INTO ops(device, op_seq, type, ts, body, batch)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                v["device"].as_str().context("device")?,
                op_seq,
                kind.name(),
                v["ts"].as_i64().unwrap_or(0),
                v["body"].as_str().context("body")?,
                v["batch"].as_i64().context("batch")?
            ],
        )?;
        Ok(())
    }

    /// Commit and close, so the file is whole on disk before it is renamed into place. A window
    /// or migration op past the restored records (their segment was damaged and skipped) goes,
    /// with every op after it: the curation checkpoint must never pass a seq the store does not
    /// hold, or the records that reuse those seqs would never be curated, and an import's must
    /// not pass records it lost, or its next pass would skip them (D6); so a migration op whose
    /// batch lost a record goes too, even when later segments were restored. Returns how many ops
    /// went.
    // ponytail: a restore keeping a batch's records but not its migration op imports them again;
    // a check of the restored records against the source's ids would catch it.
    pub fn finish(self) -> Result<usize> {
        let dropped = self.conn.execute(
            "DELETE FROM ops WHERE op_seq >= (
               SELECT MIN(w.op_seq) FROM ops w WHERE w.device = ops.device
                 AND w.type IN ('window', 'migration')
                 AND (json_extract(w.body, '$.to_seq') >
                        (SELECT COALESCE(MAX(r.seq), 0) FROM records r WHERE r.device = w.device)
                      OR w.type = 'migration' AND
                        (SELECT COUNT(*) FROM records r WHERE r.device = w.device
                           AND r.seq BETWEEN json_extract(w.body, '$.from_seq')
                                         AND json_extract(w.body, '$.to_seq'))
                        < json_extract(w.body, '$.to_seq') - json_extract(w.body, '$.from_seq') + 1))",
            [],
        )?;
        // Check missing records before adding the controls' reserved seqs: a tombstone must not
        // make an old window appear to have records a damaged backup actually lost.
        if let Some((home, device)) = self.privacy {
            crate::forget::apply(&self.conn, &home, &device)?;
        }
        self.conn.execute_batch("COMMIT")?;
        self.conn.close().map_err(|(_, e)| e)?;
        Ok(dropped)
    }
}

fn privacy_version(conn: &Connection, home: &Path, device: &str) -> Result<crate::forget::Version> {
    Ok(crate::forget::Version {
        device: device.into(),
        seq: conn.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM records WHERE device=?1",
            [device],
            |r| r.get(0),
        )?,
        op_seq: conn.query_row(
            "SELECT COALESCE(MAX(op_seq),0) FROM ops WHERE device=?1",
            [device],
            |r| r.get(0),
        )?,
        control: crate::forget::version(home)?,
    })
}

/// Final raw-write fence for an answer composed before registration. Recuration of a window
/// spanning a denied record waits for the physical-purge slice to rebuild its remaining span.
fn forgotten_op(
    conn: &Connection,
    device: &str,
    kind: OpKind,
    body: &serde_json::Value,
) -> Result<bool> {
    let anchor = |e: &serde_json::Value| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM denied_records WHERE device=?1 AND seq=?2)",
            params![e["device"].as_str(), e["seq"].as_i64()],
            |r| r.get(0),
        )?)
    };
    match kind {
        OpKind::Window => Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM denied_records WHERE device=?1 AND seq BETWEEN ?2 AND ?3)", params![device,body["from_seq"].as_i64(),body["to_seq"].as_i64()], |r| r.get(0))?),
        OpKind::Claim => {
            for e in body["evidence"].as_array().into_iter().flatten() {
                if anchor(e)? { return Ok(true); }
            }
            Ok(false)
        }
        OpKind::Correction => anchor(&body["anchor"]),
        OpKind::Digest => Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM denied_records d JOIN records r ON r.device=d.device AND r.seq=d.seq
             WHERE r.device=?1 AND r.agent=?2 AND r.session=?3 AND r.seq<=?4)",
            params![body["through"]["device"].as_str(),body["agent"].as_str(),body["session"].as_str(),body["through"]["seq"].as_i64()], |r| r.get(0))?),
        _ => Ok(false),
    }
}

/// `body` with the bytes from `offset` for `length` replaced by `*`, widened to whole characters
/// and cut at the body's end.
fn masked(body: &str, offset: i64, length: i64) -> String {
    let clamp = |n: i64| usize::try_from(n).unwrap_or(0).min(body.len());
    let (mut start, mut end) = (clamp(offset), clamp(offset.saturating_add(length)));
    while !body.is_char_boundary(start) {
        start -= 1;
    }
    while !body.is_char_boundary(end) {
        end += 1;
    }
    format!(
        "{}{}{}",
        &body[..start],
        "*".repeat(end - start),
        &body[end..]
    )
}

/// SQLite's increasing busy sleeps miss the short gaps between hooks' writes. Poll only the
/// batch's BEGIN at 1 ms, within the connection's existing timeout or `wait` when that is longer,
/// then restore that timeout before its statements and commit. Nothing in an acquired
/// transaction is retried.
fn begin_batch(
    conn: &mut Connection,
    wait: Duration,
) -> rusqlite::Result<rusqlite::Transaction<'_>> {
    let timeout: u32 = conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
    let timeout = Duration::from_millis(timeout.into());
    let deadline = Instant::now() + timeout.max(wait);
    conn.busy_timeout(Duration::ZERO)?;
    let result = loop {
        match rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate) {
            Err(e)
                if matches!(
                    e.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => break result,
        }
    };
    conn.busy_timeout(timeout)?;
    result
}

/// The next seq of `device`, inside a write transaction.
fn next_seq(tx: &rusqlite::Transaction, device: &str) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?)
}

/// `e` and its ledger rows as `device`'s next seq, inside a write transaction.
fn insert_event(
    tx: &rusqlite::Transaction,
    device: &str,
    e: &Event,
    ledger: &[(String, crate::redact::Finding)],
    ruleset: &str,
) -> Result<i64> {
    let seq = next_seq(tx, device)?;
    tx.execute(
        "INSERT INTO records(device, seq, type, ts, kind, agent, session, repo, branch, head,
                             gitdir, cwd, source, body, original_bytes)
         VALUES(?1, ?2, 'event', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            device,
            seq,
            e.ts,
            e.kind,
            e.agent,
            e.session,
            e.repo,
            e.branch,
            e.head,
            e.gitdir,
            e.cwd,
            e.source,
            e.body.as_bytes(),
            e.original_bytes
        ],
    )?;
    let now = crate::db::now_ms();
    for (field, f) in ledger {
        tx.execute(
            "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                device,
                seq,
                field,
                f.rule,
                f.offset as i64,
                f.length as i64,
                now,
                ruleset
            ],
        )?;
    }
    Ok(seq)
}

/// `ops` serialized, each within `MAX_OP_BYTES` and all within one append's caps.
fn within_batch_cap(ops: &[(OpKind, serde_json::Value)]) -> Result<Vec<(&'static str, String)>> {
    let bodies = ops
        .iter()
        .map(|(kind, body)| {
            let text = body.to_string();
            anyhow::ensure!(
                text.len() <= MAX_OP_BYTES,
                "a {} op of {} bytes is over the {MAX_OP_BYTES}-byte cap",
                kind.name(),
                text.len()
            );
            Ok((kind.name(), text))
        })
        .collect::<Result<Vec<_>>>()?;
    let total: usize = bodies.iter().map(|(_, b)| b.len()).sum();
    anyhow::ensure!(
        bodies.len() <= MAX_BATCH_OPS && total <= MAX_BATCH_BYTES,
        "an append of {} ops and {total} bytes is over the cap of {MAX_BATCH_OPS} ops and {MAX_BATCH_BYTES} bytes",
        bodies.len()
    );
    Ok(bodies)
}

/// `bodies` as `device`'s next ops, one batch, inside a write transaction.
fn insert_ops(
    tx: &rusqlite::Transaction,
    device: &str,
    bodies: &[(&str, String)],
) -> Result<Vec<i64>> {
    let mut op_seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(op_seq), 0) FROM ops WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?;
    let (ts, batch) = (crate::db::now_ms(), op_seq + 1);
    let mut seqs = Vec::with_capacity(bodies.len());
    for (kind, body) in bodies {
        op_seq += 1;
        tx.execute(
            "INSERT INTO ops(device, op_seq, type, ts, body, batch)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![device, op_seq, kind, ts, body, batch],
        )?;
        seqs.push(op_seq);
    }
    Ok(seqs)
}

/// Records `compress_through` reads per batch, and the bytes of their bodies it loads at once.
const COMPRESS_BATCH: usize = 200;
const BATCH_BYTES: u64 = 16 << 20;

/// The most a stored body may decompress to, so a crafted frame, as a restored or synced record
/// could carry, cannot expand without bound. `compress_through` leaves larger bodies plain.
const MAX_BODY_BYTES: u64 = 64 << 20;

fn unzstd(z: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    zstd::Decoder::new(z)?
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut out)?;
    if out.len() as u64 > MAX_BODY_BYTES {
        return Err(std::io::Error::other(
            "a stored body decompresses past 64 MiB",
        ));
    }
    Ok(out)
}

/// A prompt event with this body, for tests of every later consumer.
/// An event record's labels (`Raw::newest_labels`).
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    pub agent: String,
    pub session: String,
    pub repo: Option<String>,
    pub seq: i64,
    pub ts: i64,
    pub kind: String,
}

#[cfg(test)]
pub fn test_event(body: &str) -> Event {
    Event {
        agent: "claude".into(),
        session: "s".into(),
        kind: "prompt".into(),
        ts: 0,
        repo: None,
        branch: None,
        head: None,
        gitdir: None,
        cwd: None,
        source: "hook".into(),
        body: body.into(),
        original_bytes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/cards.md K4: what tombstones remove from a span, each once; with `through`, only
    /// what this device's own tombstones up to it remove, as another device's seqs say nothing of
    /// when.
    #[test]
    fn removals_of_a_span_are_listed_once_and_through_a_seq() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let device = raw.device().to_owned();
        for _ in 0..3 {
            raw.append(&test_event("{}")).unwrap();
        }
        let part = |seq: i64, offset: i64| Target::Range {
            device: device.clone(),
            seq,
            offset,
            length: 1,
        };
        let whole = Target::Record {
            device: device.clone(),
            seq: 1,
        };
        raw.append_tombstone(part(2, 0)).unwrap(); // 4
        raw.append_tombstone(part(2, 0)).unwrap(); // 5: the same part again
        raw.append_tombstone(whole).unwrap(); // 6
        raw.append_tombstone(part(3, 0)).unwrap(); // 7: outside the span asked for
        raw.conn
            .execute(
                "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                   target_offset, target_length)
                 VALUES('other', 1, 'tombstone', 0, 'rescan', ?1, 2, 1, 1)",
                [&device],
            )
            .unwrap();
        let first = (2, Some(0), Some(1));
        assert_eq!(
            raw.removed_in(&device, 1, 2, None).unwrap(),
            [(1, None, None), first, (2, Some(1), Some(1))]
        );
        assert_eq!(raw.removed_in(&device, 1, 2, Some(5)).unwrap(), [first]);
        assert_eq!(
            raw.removed_in(&device, 1, 2, Some(6)).unwrap(),
            [(1, None, None), first]
        );
    }

    #[test]
    fn a_range_is_masked_by_whole_characters_and_masking_again_changes_nothing() {
        assert_eq!(masked("abcdef", 1, 2), "a**def");
        // A range that starts or ends inside a character covers all of its bytes.
        assert_eq!(masked("aé日b", 2, 2), "a*****b");
        assert_eq!(masked("abc", 2, 100), "ab*"); // cut at the end
        assert_eq!(masked("abc", 7, 2), "abc");
        let once = masked("aé日b", 2, 2);
        assert_eq!(masked(&once, 2, 2), once);
        assert_eq!(once.len(), "aé日b".len());
    }

    /// Whether `e` is SQLite's "database is locked".
    fn busy(e: &anyhow::Error) -> bool {
        e.downcast_ref::<rusqlite::Error>()
            .is_some_and(|e| e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy))
    }

    #[test]
    fn two_writers_get_consecutive_seqs_and_both_land() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        open(p).unwrap(); // create the file first
        let seqs: Vec<i64> = std::thread::scope(|s| {
            let h: Vec<_> = (0..2)
                .map(|i| {
                    s.spawn(move || {
                        let mut r = open(p).unwrap();
                        // A writer in a tight loop can keep the other out past the busy timeout
                        // on a slow disk (#362): this is about the seqs, so it asks again.
                        (0..50)
                            .map(|n| {
                                loop {
                                    match r.append(&test_event(&format!("{i}-{n}"))) {
                                        Ok(seq) => break seq,
                                        Err(e) if busy(&e) => continue,
                                        Err(e) => panic!("{e:#}"),
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            h.into_iter().flat_map(|t| t.join().unwrap()).collect()
        });
        let mut sorted = seqs.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=100).collect::<Vec<_>>());
        let r = open(p).unwrap();
        assert_eq!(r.max_seq().unwrap(), 100);
        let bodies: std::collections::HashSet<String> = r
            .after(r.device(), 0, 1000)
            .unwrap()
            .into_iter()
            .map(|rec| match rec.item {
                Item::Event(e) => e.body,
                Item::Removed | Item::Tombstone(_) => unreachable!(),
            })
            .collect();
        assert_eq!(bodies.len(), 100);
    }

    #[test]
    fn an_initialized_store_opens_without_waiting_for_a_writer() {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| sent.send(open(home.path())).unwrap());
            let opened = received.recv_timeout(std::time::Duration::from_secs(2));
            writer.conn.execute_batch("ROLLBACK").unwrap();
            let opened = opened.expect("opening an initialized store must not need a write lock");
            assert_eq!(opened.unwrap().device(), writer.device());
        });
    }

    #[test]
    fn missing_schema_objects_share_one_write_transaction_and_roll_back_together() {
        let home = tempfile::tempdir().unwrap();
        let store = open(home.path()).unwrap();
        let drop_tables = "DROP TABLE ledger; DROP TABLE ops;";
        store.conn.execute_batch(drop_tables).unwrap();
        crate::crash::off();
        let reopened = open(home.path()).unwrap();
        // One commit means no writer can interleave between the schema's CREATE statements.
        assert_eq!(crate::crash::count(), 1);
        let tables = || {
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                     AND name IN ('ledger', 'ops')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(tables(), 2);
        reopened.conn.execute_batch(drop_tables).unwrap();
        crate::crash::at(1);
        let failed = open(home.path());
        crate::crash::off();
        assert!(failed.is_err());
        assert_eq!(tables(), 0);
    }

    #[test]
    fn missing_open_state_waits_past_the_busy_timeout_for_a_writer() {
        // Each case needs a different open-time write. Observe each opener's first actual
        // lock retry before holding the locks a little longer, rather than timing thread start.
        let changes = [
            "DROP INDEX ops_exclusions",
            "DROP TABLE ops",
            "ALTER TABLE ledger DROP COLUMN field",
            "DELETE FROM meta",
            "PRAGMA journal_mode=DELETE",
        ];
        let homes: Vec<_> = changes
            .iter()
            .map(|_| tempfile::tempdir().unwrap())
            .collect();
        let writers: Vec<_> = homes
            .iter()
            .zip(changes)
            .map(|(home, sql)| {
                let writer = open(home.path()).unwrap();
                writer.conn.execute_batch(sql).unwrap();
                writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                writer
            })
            .collect();
        std::thread::scope(|scope| {
            let (retried, received) = std::sync::mpsc::channel();
            let threads: Vec<_> = homes
                .iter()
                .map(|home| {
                    let retried = retried.clone();
                    scope.spawn(move || {
                        crate::db::BUSY_RETRY_NOTICE.with(|notice| notice.set(Some(retried)));
                        let opened = open(home.path());
                        assert!(
                            crate::db::BUSY_RETRY_NOTICE.with(|notice| notice.take().is_none()),
                            "opening a contended store must retry"
                        );
                        opened
                    })
                })
                .collect();
            drop(retried);
            let observed: Result<Vec<_>, _> = threads
                .iter()
                .map(|_| received.recv_timeout(std::time::Duration::from_secs(5)))
                .collect();
            if observed.is_ok() {
                // The ordinary write attempts have already exhausted SQLite's 2 s timeout.
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            for writer in &writers {
                writer.conn.execute_batch("ROLLBACK").unwrap();
            }
            observed.expect("every opener must retry before the write locks are released");
            for (index, thread) in threads.into_iter().enumerate() {
                let mut opened = thread.join().unwrap().unwrap();
                if index != 3 {
                    assert_eq!(opened.device(), writers[index].device());
                }
                assert_eq!(opened.append(&test_event("after the lock")).unwrap(), 1);
                let timeout: i64 = opened
                    .conn
                    .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(timeout, 2_000);
                let synchronous: i64 = opened
                    .conn
                    .query_row("PRAGMA synchronous", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(synchronous, 2);
            }
        });
    }

    #[test]
    fn raw_db_is_full_and_has_no_label_index() {
        let home = tempfile::tempdir().unwrap();
        let r = open(home.path()).unwrap();
        let sync: i64 = r
            .conn
            .query_row("PRAGMA synchronous", [], |x| x.get(0))
            .unwrap();
        assert_eq!(sync, 2); // FULL
        // spec 1.6: session, repo and branch are labels, never an index root.
        let idx: i64 = r
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND
                 (sql LIKE '%session%' OR sql LIKE '%repo%' OR sql LIKE '%branch%')",
                [],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn after_returns_what_was_appended_in_seq_order() {
        let home = tempfile::tempdir().unwrap();
        let mut r = open(home.path()).unwrap();
        let mut e = test_event("ロボットの記録");
        e.repo = Some("github.com/o/r".into());
        e.original_bytes = Some(300_000);
        for _ in 0..3 {
            r.append(&test_event("x")).unwrap();
        }
        assert_eq!(r.append(&e).unwrap(), 4);
        let recs = r.after(r.device(), 2, 10).unwrap();
        assert_eq!(recs.iter().map(|x| x.seq).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(recs[1].item, Item::Event(Box::new(e)));
        assert_eq!(recs[1].device, r.device());
        assert!(r.after("other-device", 0, 10).unwrap().is_empty());
    }

    #[test]
    fn a_ledger_from_task_1_gains_its_field_column() {
        let home = tempfile::tempdir().unwrap();
        let c = Connection::open(home.path().join("raw.db")).unwrap();
        c.execute_batch(
            "CREATE TABLE ledger (device TEXT NOT NULL, seq INTEGER NOT NULL, rule TEXT NOT NULL,
             offset INTEGER NOT NULL, length INTEGER NOT NULL, ts INTEGER NOT NULL, ruleset TEXT NOT NULL);",
        )
        .unwrap();
        drop(c);
        let mut r = open(home.path()).unwrap();
        let f = crate::redact::Finding {
            rule: "r".into(),
            offset: 0,
            length: 1,
        };
        r.append_with_ledger(&test_event("x"), &[("/prompt".into(), f)], "test")
            .unwrap();
        let field: String = r
            .conn
            .query_row("SELECT field FROM ledger", [], |x| x.get(0))
            .unwrap();
        assert_eq!(field, "/prompt");
    }

    #[test]
    fn bodies_larger_than_a_batch_together_are_compressed_over_several() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let big = "c".repeat(BATCH_BYTES as usize / 2 + 1);
        for _ in 0..3 {
            raw.append(&test_event(&big)).unwrap();
        }
        let device = raw.device().to_owned();
        assert_eq!(raw.compress_through(&device, 0, 3).unwrap(), 3);
        let recs = raw.after(&device, 0, 3).unwrap();
        assert!(
            recs.iter()
                .all(|r| matches!(&r.item, Item::Event(e) if e.body == big))
        );
    }

    #[test]
    fn a_body_above_the_decode_limit_stays_plain_and_reads_back() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let big = "a".repeat(MAX_BODY_BYTES as usize + 1);
        raw.append(&test_event(&big)).unwrap();
        raw.append(&test_event(&"b".repeat(1000))).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(raw.compress_through(&device, 0, 2).unwrap(), 1); // only the second
        let recs = raw.after(&device, 0, 2).unwrap();
        assert!(matches!(&recs[0].item, Item::Event(e) if e.body.len() == big.len()));
    }

    #[test]
    fn a_body_that_decompresses_past_the_limit_is_an_error() {
        let bomb = zstd::bulk::compress(&vec![0u8; (MAX_BODY_BYTES + 1) as usize], 3).unwrap();
        assert!(bomb.len() < 64 * 1024, "{}", bomb.len());
        assert!(unzstd(&bomb).is_err());
        let ok = zstd::bulk::compress(b"fine", 3).unwrap();
        assert_eq!(unzstd(&ok).unwrap(), b"fine");
    }

    /// Codex on #364: a span's time is its last event's by seq, not the largest: records are not
    /// always appended in time order (a replay, a hook that writes late).
    #[test]
    fn a_spans_time_is_its_last_events() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for ts in [5_000, 9_000, 7_000] {
            raw.append(&Event {
                ts,
                ..test_event("{}")
            })
            .unwrap();
        }
        let dev = raw.device().to_owned();
        assert_eq!(raw.labels_in(&dev, 1, 3).unwrap().ts, Some(7_000));
        assert_eq!(raw.labels_in(&dev, 1, 2).unwrap().ts, Some(9_000));
    }

    #[test]
    fn device_id_stays_with_the_file_and_changes_on_a_copy() {
        let home = tempfile::tempdir().unwrap();
        let first = open(home.path()).unwrap().device().to_owned();
        assert!(
            first.len() == 8 && first.bytes().all(|b| b.is_ascii_hexdigit()),
            "{first}"
        );
        assert_eq!(open(home.path()).unwrap().device(), first);
        // The same store copied elsewhere (another machine's home) is another device.
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let other = open(copy.path()).unwrap();
        assert_ne!(other.device(), first);
        assert_eq!(open(copy.path()).unwrap().device(), other.device());
    }

    #[test]
    fn a_page_is_bounded_in_bytes_and_always_holds_one_record() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        for _ in 0..3 {
            raw.append(&test_event(&"x".repeat(1000))).unwrap();
        }
        assert_eq!(raw.after_within(&dev, 0, 10, 1).unwrap().len(), 1);
        assert_eq!(raw.after_within(&dev, 0, 10, 1 << 20).unwrap().len(), 3);
        assert_eq!(raw.after_within(&dev, 0, 2, 1 << 20).unwrap().len(), 2);
        // Bodies that compress well are counted as read, not as stored.
        raw.compress_through(&dev, 0, 3).unwrap();
        let stored: i64 = raw
            .conn
            .query_row("SELECT SUM(length(body)) FROM records", [], |r| r.get(0))
            .unwrap();
        assert!(
            stored < 1000,
            "the bodies were not compressed: {stored} bytes"
        );
        assert_eq!(raw.after_within(&dev, 0, 10, 1500).unwrap().len(), 1);
    }

    /// Codex on #304: the list folds a device's ops in their own order, so a clock set back
    /// between two of them never puts the newer one first.
    #[test]
    fn a_clock_set_back_never_puts_a_newer_exclusion_first() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let op = |undo: bool| {
            let body = serde_json::json!({"repo": "x", "undo": undo});
            (OpKind::Exclusion, body)
        };
        raw.append_ops(&[op(false)]).unwrap();
        raw.append_ops(&[op(true)]).unwrap();
        let last = raw.append_ops(&[op(false)]).unwrap()[0];
        raw.conn
            .execute("UPDATE ops SET ts = 0 WHERE op_seq = ?1", [last])
            .unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["x"]);
    }

    /// Codex on #304: a copied store keeps the old device's ops under a new device, and an
    /// exclusion appended there comes after every op it holds, even when the old device's clock
    /// ran ahead of the new one's.
    #[test]
    fn an_exclusion_comes_after_every_op_a_copied_store_holds() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude("x", false).unwrap();
        raw.exclude("x", true).unwrap();
        raw.conn
            .execute(
                "UPDATE ops SET ts = ts + 3600000,
                   body = json_set(body, '$.clock', json_extract(body, '$.clock') + 3600000)",
                [],
            )
            .unwrap();
        drop(raw);
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let mut other = open(copy.path()).unwrap();
        other.exclude("x", false).unwrap();
        assert_eq!(other.exclusions().unwrap(), ["x"]);
    }

    fn import_doc(id: usize, body: String) -> ImportDoc {
        ImportDoc {
            uid: format!("claude-mem:abc:o{id}"),
            source: "claude-mem:abc".into(),
            source_id: format!("o{id}"),
            kind: "decision".into(),
            repo: "claude-mem:r".into(),
            session: "s".into(),
            ts: 1,
            title: "t".into(),
            body,
        }
    }

    /// D5: a body that would take its op over the cap is cut to fit, the marker saying how long
    /// it was, JSON escapes counted.
    #[test]
    fn an_op_over_the_cap_is_clipped_with_a_marker() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let body = "a \"quoted\" line\n".repeat(8_000);
        let chars = body.chars().count();
        assert_eq!(raw.append_imports(vec![import_doc(1, body)]).unwrap(), 1);
        let op = raw.ops_after(raw.device(), 0, 1).unwrap().remove(0);
        assert!(op.body.to_string().len() <= MAX_OP_BYTES);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        let marker = format!("\n…[clipped, {chars} chars in full]");
        assert!(
            kept.body.ends_with(&marker),
            "{}",
            &kept.body[kept.body.len() - 80..]
        );
        assert!(kept.body.starts_with("a \"quoted\" line\n"));
    }

    /// cubic on the Task 3 PR: a title as long as a body is cut too, and a document whose other
    /// fields alone are over the cap is an error, never an endless loop.
    #[test]
    fn a_title_over_the_cap_is_cut_and_other_fields_over_it_are_an_error() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let doc = ImportDoc {
            title: "t".repeat(100_000),
            ..import_doc(1, "b".repeat(100_000))
        };
        raw.append_imports(vec![doc]).unwrap();
        let op = raw.ops_after(raw.device(), 0, 1).unwrap().remove(0);
        assert!(op.body.to_string().len() <= MAX_OP_BYTES);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        assert!(kept.title.ends_with("…[clipped, 100000 chars in full]"));
        // A title that alone took the op over the cap leaves its body whole (Codex on #305).
        let doc = ImportDoc {
            title: "t".repeat(100_000),
            ..import_doc(3, "b".repeat(1_000))
        };
        raw.append_imports(vec![doc]).unwrap();
        let op = raw.ops_after(raw.device(), 1, 1).unwrap().remove(0);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        assert_eq!(kept.body, "b".repeat(1_000));
        let doc = ImportDoc {
            session: "s".repeat(100_000),
            ..import_doc(2, "b".into())
        };
        assert!(raw.append_imports(vec![doc]).is_err());
    }

    /// D5: large documents go in more than one append, each within the byte cap, and none lost.
    #[test]
    fn a_batch_of_large_documents_splits_under_the_byte_cap() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let docs: Vec<ImportDoc> = (0..100)
            .map(|i| import_doc(i, "x".repeat(60_000)))
            .collect();
        assert_eq!(raw.append_imports(docs).unwrap(), 100);
        let ops = raw.ops_after(raw.device(), 0, 1_000).unwrap();
        let mut batches: std::collections::BTreeMap<i64, usize> = Default::default();
        for op in &ops {
            *batches.entry(op.batch).or_default() += op.body.to_string().len();
        }
        assert_eq!(ops.len(), 100);
        assert!(batches.len() > 1);
        assert!(batches.values().all(|&bytes| bytes <= MAX_BATCH_BYTES));
    }

    /// OpenCodeReview on #305: an import op without a source id is passed over, not an error.
    #[test]
    fn import_keys_pass_over_an_op_without_a_text_source_id() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let ops = [
            serde_json::json!({"source": "claude-mem"}),
            serde_json::json!({"source": "claude-mem", "source_id": 7}),
            serde_json::json!({"source": "claude-mem", "source_id": "o1"}),
        ];
        raw.append_ops(&ops.map(|op| (OpKind::Import, op))).unwrap();
        let keys = raw.import_keys("claude-mem").unwrap();
        assert_eq!(keys, ["o1".to_owned()].into_iter().collect());
    }

    fn imported(body: &str) -> crate::capture::Captured {
        crate::capture::Captured {
            event: Event {
                source: "oboete-v1".into(),
                ..test_event(body)
            },
            ledger: Vec::new(),
        }
    }

    fn v1_checkpoint(through: i64) -> Checkpoint {
        Checkpoint {
            key: "oboete-v1:d1".into(),
            through,
            row: Some(V1Row {
                id: through,
                ts: 7,
                session_id: "s1".into(),
            }),
            prefix: None,
        }
    }

    #[test]
    fn batch_lock_failures_restore_the_timeout_without_committing() {
        for records in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            let append = |raw: &mut Raw| {
                if records {
                    raw.append_imported(&[imported("kept")], "v", Some(&v1_checkpoint(1)))
                } else {
                    raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "kept"}))])
                }
            };
            let timeout = |raw: &Raw| {
                raw.conn
                    .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0))
                    .unwrap()
            };
            assert_eq!(timeout(&raw), 2_000);
            // An op's batch gives up at the connection's timeout; an import waits longer
            // (`an_imported_batch_waits_past_the_busy_timeout_for_a_writer`).
            if !records {
                raw.conn.busy_timeout(Duration::from_millis(100)).unwrap();
                let writer = Connection::open(home.path().join("raw.db")).unwrap();
                writer.execute_batch("BEGIN IMMEDIATE").unwrap();
                let started = Instant::now();
                let error = append(&mut raw).unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<rusqlite::Error>()
                        .unwrap()
                        .sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                assert!(started.elapsed() >= Duration::from_millis(100));
                assert!(started.elapsed() < Duration::from_secs(1));
                assert_eq!(timeout(&raw), 100);
                assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
                writer.execute_batch("ROLLBACK").unwrap();
            }

            // A non-lock error returns promptly and restores the original timeout too.
            raw.conn.busy_timeout(Duration::from_secs(2)).unwrap();
            raw.conn.execute_batch("PRAGMA query_only=ON").unwrap();
            let started = Instant::now();
            let error = append(&mut raw).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<rusqlite::Error>()
                    .unwrap()
                    .sqlite_error_code(),
                Some(rusqlite::ErrorCode::ReadOnly)
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(timeout(&raw), 2_000);
            raw.conn.execute_batch("PRAGMA query_only=OFF").unwrap();

            // A failed commit is attempted once; neither the rows nor their checkpoint lands.
            crate::crash::at(1);
            let failed = append(&mut raw);
            let commits = crate::crash::count();
            crate::crash::off();
            assert!(failed.is_err());
            assert_eq!(commits, 1);
            assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
            assert!(raw.migration_checkpoints("oboete-v1:").unwrap().is_empty());
            assert_eq!(append(&mut raw).unwrap(), [1]);
            assert_eq!(timeout(&raw), 2_000);
            assert_eq!(raw.max_op_seq().unwrap(), 1);
            if records {
                assert_eq!(raw.max_seq().unwrap(), 1);
                assert_eq!(
                    raw.migration_checkpoints("oboete-v1:").unwrap()["oboete-v1:d1"],
                    v1_checkpoint(1)
                );
            }
        }
    }

    /// D6: a batch of imported records, their ledger rows and the checkpoint op commit together;
    /// the op says through which seq the batch went, and each key's furthest checkpoint reads
    /// back by its prefix.
    /// #362: an import is no hook, so its batch waits for another writer as a non-hook open does
    /// (`db::OPEN_WRITE_WAIT`), not for the connection's short busy timeout: a pass beside a busy
    /// agent stopped with "database is locked".
    #[test]
    fn an_imported_batch_waits_past_the_busy_timeout_for_a_writer() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.conn.busy_timeout(Duration::from_millis(50)).unwrap();
        let writer = open(home.path()).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let seqs = std::thread::scope(|s| {
            let import = s.spawn(|| raw.append_imported(&[imported("a")], "v9", None));
            std::thread::sleep(Duration::from_millis(400));
            assert!(!import.is_finished(), "it gave up at the busy timeout");
            writer.conn.execute_batch("ROLLBACK").unwrap();
            import.join().unwrap().unwrap()
        });
        assert_eq!(seqs, [1]);
        // Its own timeout is back for what follows.
        let timeout: u32 = raw
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 50);
    }

    #[test]
    fn an_imported_batch_lands_with_its_ledger_and_checkpoint() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.append(&test_event("live")).unwrap();
        let mut masked = imported("b");
        let finding = crate::redact::Finding {
            rule: "r".into(),
            offset: 1,
            length: 2,
        };
        masked.ledger.push(("/prompt".into(), finding));
        let seqs = raw
            .append_imported(&[imported("a"), masked], "v9", Some(&v1_checkpoint(42)))
            .unwrap();
        assert_eq!(seqs, [2, 3]);
        let sources: Vec<String> = raw
            .after(raw.device(), 1, 10)
            .unwrap()
            .into_iter()
            .map(|r| match r.item {
                Item::Event(e) => e.source,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(sources, ["oboete-v1", "oboete-v1"]);
        let ledger: (i64, String) = raw
            .conn
            .query_row("SELECT seq, ruleset FROM ledger", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(ledger, (3, "v9".to_owned()));
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Migration);
        assert_eq!(
            ops[0].body,
            serde_json::json!({"key": "oboete-v1:d1", "through": 42, "from_seq": 2, "to_seq": 3,
                "row": {"id": 42, "ts": 7, "session_id": "s1"}})
        );
        // A transcript's checkpoint has no row; one with nothing to record holds the last seq.
        let t = Checkpoint {
            key: "transcript:claude:s1".into(),
            through: 9,
            row: None,
            prefix: None,
        };
        assert!(raw.append_imported(&[], "v9", Some(&t)).unwrap().is_empty());
        raw.append_imported(&[imported("c")], "v9", Some(&v1_checkpoint(50)))
            .unwrap();
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(
            ops[1].body,
            // No record: an empty range, which no restore can lose.
            serde_json::json!({"key": "transcript:claude:s1", "through": 9, "from_seq": 4,
                "to_seq": 3})
        );
        assert_eq!(
            raw.migration_checkpoints("oboete-v1:").unwrap(),
            [("oboete-v1:d1".to_owned(), v1_checkpoint(50))]
                .into_iter()
                .collect()
        );
        assert_eq!(
            raw.migration_checkpoints("transcript:").unwrap(),
            [(t.key.clone(), t)].into_iter().collect()
        );
    }

    /// D6: an append of imported records is one transaction, bounded as an op batch is, and it
    /// never takes a record of a live source, which curation and the manifest read.
    #[test]
    fn an_imported_batch_over_a_cap_or_with_a_live_record_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let many: Vec<_> = (0..=IMPORT_BATCH).map(|_| imported("x")).collect();
        assert!(raw.append_imported(&many, "v", None).is_err());
        let big: Vec<_> = (0..5).map(|_| imported(&"x".repeat(1 << 20))).collect();
        assert!(raw.append_imported(&big, "v", None).is_err());
        for source in LIVE {
            let mut live = imported("x");
            live.event.source = source.into();
            let batch = [imported("y"), live];
            assert!(
                raw.append_imported(&batch, "v", Some(&v1_checkpoint(1)))
                    .is_err()
            );
        }
        assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
        // At the caps it lands.
        assert_eq!(raw.append_imported(&big[1..], "v", None).unwrap().len(), 4);
        let seqs = raw.append_imported(&many[1..], "v", None).unwrap();
        assert_eq!(seqs.len(), IMPORT_BATCH);
    }

    /// spec 8.4, A104: both import paths ask `denied` of each item and leave a denied one out. A
    /// batch left with nothing still moves its checkpoint, so the next pass does not read it again.
    #[test]
    fn every_append_path_calls_denied() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let forgotten = format!("x {DENIED_IN_TESTS} y");
        let batch = [imported(&forgotten), imported("kept")];
        let seqs = raw
            .append_imported(&batch, "v", Some(&v1_checkpoint(2)))
            .unwrap();
        assert_eq!(seqs, [1]);
        let batch = [imported(&forgotten)];
        let seqs = raw
            .append_imported(&batch, "v", Some(&v1_checkpoint(3)))
            .unwrap();
        assert!(seqs.is_empty());
        let kept = raw.after(raw.device(), 0, 10).unwrap();
        assert!(
            matches!(&kept[..], [r] if r.item == Item::Event(Box::new(imported("kept").event)))
        );
        let checkpoints = raw.migration_checkpoints("oboete-v1:").unwrap();
        assert_eq!(checkpoints["oboete-v1:d1"].through, 3);
        let docs = vec![
            import_doc(1, forgotten.clone()),
            ImportDoc {
                title: forgotten,
                ..import_doc(2, "b".into())
            },
            import_doc(3, "kept".into()),
        ];
        assert_eq!(raw.append_imports(docs).unwrap(), 1);
        let keys = raw.import_keys("claude-mem:abc").unwrap();
        assert_eq!(keys, ["o3".to_owned()].into_iter().collect());
    }

    /// D6, the transcript cut: each session's earliest record that is neither the transcript's
    /// own nor a v1 repository label.
    #[test]
    fn earliest_by_session_leaves_out_transcript_and_touch_records() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let at = |agent: &str, session: &str, ts: i64, source: &str, kind: &str| Event {
            agent: agent.into(),
            session: session.into(),
            ts,
            source: source.into(),
            kind: kind.into(),
            ..test_event("x")
        };
        for e in [
            at("claude", "s1", 50, "hook", "prompt"),
            at("claude", "s1", 30, "oboete-v1", "prompt"),
            at("claude", "s1", 10, "transcript", "prompt"),
            at("claude", "s1", 5, "oboete-v1", "touch"),
            at("codex", "s1", 70, "replay", "tool"),
            at("codex", "s2", 3, "transcript", "prompt"),
        ] {
            raw.append(&e).unwrap();
        }
        let key = |a: &str, s: &str| (a.to_owned(), s.to_owned());
        assert_eq!(
            raw.earliest_by_session().unwrap(),
            [(key("claude", "s1"), 30), (key("codex", "s1"), 70)]
                .into_iter()
                .collect()
        );
    }

    /// D6: v1's repository labels are records, never counted as imported events.
    #[test]
    fn imported_counts_leave_touch_records_out() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut touch = imported("{}");
        touch.event.kind = "touch".into();
        raw.append_imported(&[imported("a"), touch], "v", None)
            .unwrap();
        let dev = raw.device().to_owned();
        assert_eq!(
            raw.imported_counts(&dev, &[(1, 10)]).unwrap(),
            [("oboete-v1".to_owned(), 1)].into_iter().collect()
        );
    }

    /// OpenCodeReview on #304: an event's agent and session labels may be NULL; `sessions_in`
    /// keys such an event as `session_key` does, never an error that stops the curation phase.
    #[test]
    fn sessions_in_keys_an_event_without_labels_as_session_key_does() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let e = Event {
            repo: Some("x".into()),
            ..test_event("hi")
        };
        let seq = raw.append(&e).unwrap();
        raw.conn
            .execute(
                "UPDATE records SET agent = NULL, session = NULL WHERE seq = ?1",
                [seq],
            )
            .unwrap();
        let device = raw.device().to_owned();
        let key = raw.session_key(&device, seq).unwrap().unwrap();
        let keys = raw.sessions_in(&["x".to_owned()]).unwrap();
        assert_eq!(keys, [key].into_iter().collect());
    }

    #[test]
    fn ops_commit_together_and_read_back_in_order() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let window = serde_json::json!({"from_seq": 1, "to_seq": 4, "outcome": "curated"});
        let claim = serde_json::json!({"text": "all timestamps on disk stay UTC"});
        let seqs = raw
            .append_ops(&[
                (OpKind::Window, window.clone()),
                (OpKind::Claim, claim.clone()),
            ])
            .unwrap();
        assert_eq!(seqs, [1, 2]);
        let ops = raw.ops_after(&dev, 0, 10).unwrap();
        assert_eq!(
            ops.iter()
                .map(|o| (o.op_seq, o.kind, o.body.clone()))
                .collect::<Vec<_>>(),
            [
                (1, OpKind::Window, window),
                (2, OpKind::Claim, claim.clone())
            ]
        );
        // An op over the cap fails the whole append: the op before it is not written either.
        let huge = serde_json::json!({"text": "x".repeat(MAX_OP_BYTES)});
        assert!(
            raw.append_ops(&[(OpKind::Claim, claim), (OpKind::Claim, huge)])
                .is_err()
        );
        // So does an append whose ops fit one by one but not together.
        let big = serde_json::json!({"text": "x".repeat(MAX_OP_BYTES - 100)});
        let many = vec![(OpKind::Claim, big); MAX_BATCH_BYTES / MAX_OP_BYTES + 1];
        assert!(raw.append_ops(&many).is_err());
        // And one of many small ops, whose lines' own fields would add up.
        let many = vec![(OpKind::Claim, serde_json::json!(0)); MAX_BATCH_OPS + 1];
        assert!(raw.append_ops(&many).is_err());
        assert_eq!(raw.max_op_seq().unwrap(), 2);
        assert_eq!(raw.ops_after(&dev, 1, 10).unwrap().len(), 1);
    }

    #[test]
    fn an_ops_segment_ends_only_between_appends() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let claim = |i: i32| {
            (
                OpKind::Claim,
                serde_json::json!({"text": format!("claim {i}")}),
            )
        };
        let window = (OpKind::Window, serde_json::json!({"to_seq": 1}));
        raw.append_ops(&[window, claim(1), claim(2)]).unwrap();
        raw.append_ops(&[claim(3)]).unwrap();
        // The cap is reached after the first line; the window's claims still come with it.
        let seqs = |lines: Vec<(i64, String)>| lines.into_iter().map(|l| l.0).collect::<Vec<_>>();
        assert_eq!(seqs(raw.export_op_lines(0, 1).unwrap()), [1, 2, 3]);
        assert_eq!(seqs(raw.export_op_lines(3, 1).unwrap()), [4]);
    }

    #[test]
    fn the_curation_checkpoint_is_the_last_window_that_is_not_a_recuration() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        assert_eq!(raw.curation_checkpoint(&dev).unwrap(), (0, None));
        let mut window = |body: serde_json::Value| {
            raw.append_ops(&[(OpKind::Window, body)]).unwrap();
            raw.curation_checkpoint(&dev).unwrap()
        };
        assert_eq!(
            window(serde_json::json!({"to_seq": 5, "to_offset": null})),
            (5, None)
        );
        // A window that ends inside an event: the next one starts at that offset.
        assert_eq!(
            window(serde_json::json!({"to_seq": 7, "to_offset": 120})),
            (7, Some(120))
        );
        // `oboete recurate` of an older span moves it neither back nor forward.
        let back = serde_json::json!({"to_seq": 2, "to_offset": null, "recurate": true});
        assert_eq!(window(back), (7, Some(120)));
        raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "c"}))])
            .unwrap();
        assert_eq!(raw.curation_checkpoint(&dev).unwrap(), (7, Some(120)));
    }

    /// A session's previous window (#240) is the one the worker cut before the window at `from`,
    /// up to `from` when it runs past it: not a recuration of another part of a split event, nor
    /// the window at `from` curated before. Its claims come with those of every later window op
    /// quoted inside it, the newest window first.
    #[test]
    fn the_previous_window_is_the_workers_with_the_recurations_of_its_parts() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for i in 1..=4 {
            raw.append(&test_event(&format!("line {i}"))).unwrap();
        }
        // A window op and its claims, each quoted at (seq, offset) for 5 bytes.
        let window = |raw: &mut Raw,
                      span: [Option<i64>; 4],
                      recurate: bool,
                      claims: &[(&str, i64, i64)]| {
            let op = serde_json::json!({"from_seq": span[0], "from_offset": span[1],
                "to_seq": span[2], "to_offset": span[3], "recurate": recurate});
            let mut ops = vec![(OpKind::Window, op)];
            for (text, seq, offset) in claims {
                let claim = serde_json::json!({"text": text, "evidence": [{"device": raw.device(),
                    "seq": seq, "offset": offset, "length": 5}]});
                ops.push((OpKind::Claim, claim));
            }
            raw.append_ops(&ops).unwrap();
        };
        let previous = |raw: &Raw, from: (i64, Option<i64>)| -> Vec<String> {
            let ops = raw
                .previous_window_ops("claude", "s", from, |_| true)
                .unwrap();
            ops.into_iter()
                .map(|o| o.body["text"].as_str().unwrap().to_owned())
                .collect()
        };
        // Event 2 is split at offset 50; its first part is curated again after the worker went on.
        window(
            &mut raw,
            [Some(1), None, Some(2), Some(50)],
            false,
            &[("w1", 1, 0)],
        );
        window(
            &mut raw,
            [Some(2), Some(50), Some(2), None],
            false,
            &[("w2", 2, 60)],
        );
        window(
            &mut raw,
            [Some(3), None, Some(4), None],
            false,
            &[("w3", 3, 0)],
        );
        // A quote a split cuts in two is in neither part, as `curate::anchored_in` reads it.
        window(
            &mut raw,
            [Some(1), None, Some(2), Some(50)],
            true,
            &[("r1", 1, 0), ("r1 cut", 2, 48)],
        );
        assert_eq!(previous(&raw, (3, None)), ["w2"]);
        // The window at event 2's offset 50 curated again: its previous window is the first part.
        assert_eq!(previous(&raw, (2, Some(50))), ["r1", "w1"]);
        // A part of the worker's window curated again: the rest of the window is still its own.
        window(
            &mut raw,
            [Some(4), None, Some(4), None],
            true,
            &[("r4", 4, 0)],
        );
        assert_eq!(previous(&raw, (5, None)), ["r4", "w3"]);
        // Event 3 curated again on its own; then event 4 alone, whose previous window is the
        // worker's 3-4 up to event 4.
        window(
            &mut raw,
            [Some(3), None, Some(3), None],
            true,
            &[("r3", 3, 0)],
        );
        assert_eq!(previous(&raw, (4, None)), ["r3", "w3"]);
        // A recuration of a part that does not hold the session's latest event is the window's too.
        assert_eq!(previous(&raw, (5, None)), ["r3", "r4", "w3"]);
        // Recurations cut another way, over the window's start or into the window at 4: only
        // what they quoted inside the previous window.
        let across = [("r23 before", 2, 60), ("r23", 3, 5), ("r23 after", 4, 0)];
        window(&mut raw, [Some(2), Some(50), Some(4), None], true, &across);
        assert_eq!(previous(&raw, (4, None)), ["r23", "r3", "w3"]);
        assert_eq!(
            previous(&raw, (5, None)),
            ["r23", "r23 after", "r3", "r4", "w3"]
        );
        // A quote of another device's record is in no window of this device, whatever its seq.
        let op = serde_json::json!({"from_seq": 3, "from_offset": null, "to_seq": 4,
            "to_offset": null, "recurate": true});
        let quote = |device: &str, text: &str| {
            let e = serde_json::json!({"device": device, "seq": 3, "offset": 10, "length": 5});
            (
                OpKind::Claim,
                serde_json::json!({"text": text, "evidence": [e]}),
            )
        };
        let (here, other) = (quote(raw.device(), "r34"), quote("other", "elsewhere"));
        raw.append_ops(&[(OpKind::Window, op), other, here])
            .unwrap();
        assert_eq!(
            previous(&raw, (5, None)),
            ["r34", "r23", "r23 after", "r3", "r4", "w3"]
        );
    }
}
