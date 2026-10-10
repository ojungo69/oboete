//! Design B's record store (docs/spec.md sections 1.6, 2.1, 2.5; docs/milestone-2-plan.md Task 1):
//! `raw.db`, one sequence per device holding events and tombstones. (device, seq) is the only
//! ordering and partition key; session, repo and branch are labels, never an index root.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{Duration, Instant};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- Only this schema's creation of a fresh store proves its home lineage. Legacy metadata,
-- including a missing device row in an existing records table, is not that proof.
-- The id, its proof, the device and its file binding are one complete schema commit.
INSERT OR IGNORE INTO meta(key, value)
  SELECT 'home_id', '~fresh_device~'
  WHERE NOT EXISTS (SELECT 1 FROM meta WHERE key='device_id')
    AND NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='records');
INSERT OR IGNORE INTO meta(key, value)
  SELECT 'home_id_proven', '1'
  WHERE NOT EXISTS (SELECT 1 FROM meta WHERE key='device_id')
    AND NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='records');
INSERT OR IGNORE INTO meta(key, value)
  SELECT 'device_id', '~fresh_device~'
  WHERE NOT EXISTS (SELECT 1 FROM meta WHERE key='device_id')
    AND NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='records');
INSERT OR IGNORE INTO meta(key, value)
  SELECT 'store_file', '~fresh_file~'
  WHERE (SELECT value FROM meta WHERE key='device_id')='~fresh_device~'
    AND NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='records');
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
  deny_origin TEXT,            -- a forget control's stable denial, even without its target
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
-- docs/work-state.md L4: the agents' work state, read at each session start from its own ops.
CREATE INDEX IF NOT EXISTS ops_work_state ON ops(device, op_seq) WHERE type = 'work_state';
-- Milestone 5 D5: a forgotten uid, by the forget op that names it, any device's. The op is the
-- authority and this its index: a restore of the ops brings the denial back with them.
CREATE INDEX IF NOT EXISTS ops_forget_uid ON ops(json_extract(body, '$.uid'))
  WHERE type = 'forget';
-- Milestone 5 D1 (docs/milestone-5-plan.md): what forget denies, by the record's import origin,
-- the one identity (rule 5); `device` and `seq` are where it was when it was forgotten, `ts` its
-- time only when it counted toward the transcript cut (rule 10), `session` its labels' hash.
CREATE TABLE IF NOT EXISTS denied_records(
  origin TEXT PRIMARY KEY, device TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER,
  session TEXT NOT NULL, job TEXT NOT NULL
);
-- An imported record's native identity, hashed (forget::origin): no original id or text.
CREATE TABLE IF NOT EXISTS import_origins(
  device TEXT NOT NULL, seq INTEGER NOT NULL, origin TEXT NOT NULL, native_session TEXT,
  ambiguous INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY(device, seq)
);
CREATE INDEX IF NOT EXISTS import_origins_origin ON import_origins(origin);
-- Each forget request, bodyless, as its log line holds it (rules 4, 14).
CREATE TABLE IF NOT EXISTS forget_jobs(
  id TEXT PRIMARY KEY, request TEXT NOT NULL, started INTEGER NOT NULL, step INTEGER NOT NULL
);
";

/// Only a fresh schema needs an identity. The replacements are OS-random hex and the native
/// file's numeric device/inode (volume/index on Windows), never paths or user-provided SQL.
fn schema_for_file(conn: &Connection, path: &Path) -> Result<String> {
    let records: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='records')",
        [],
        |r| r.get(0),
    )?;
    let (id, file) = if records {
        // Every fresh-only INSERT is skipped on an existing records table; no entropy needed.
        (String::new(), String::new())
    } else {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes)?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let file = crate::db::store_file(path);
        anyhow::ensure!(
            !file.is_empty() && file.bytes().all(|b| b.is_ascii_digit() || b == b':'),
            "raw file identity is unavailable"
        );
        (id, file)
    };
    Ok(SCHEMA
        .replace("~fresh_device~", &id)
        .replace("~fresh_file~", &file))
}

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
    /// One write of an agent's work state (docs/work-state.md L4): `{repo, list, fields, clock}`.
    WorkState,
    /// A claim's or an imported document's uid forgotten (milestone 5 D5): `{uid, job}`, no text.
    Forget,
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
            OpKind::WorkState => "work_state",
            OpKind::Forget => "forget",
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
            Self::WorkState,
            Self::Forget,
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

impl Op {
    /// Milestone 5 D5 (2b): an op a forget rewrote to `{"forgotten": "<job>"}`, which derives
    /// nothing.
    pub fn forgotten(&self) -> bool {
        self.body.as_object().is_some_and(|o| {
            o.len() == 1 && o.get("forgotten").is_some_and(serde_json::Value::is_string)
        })
    }
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

/// Whether an import must leave an item out because the owner forgot it (spec 8.4, A104): the
/// deny-list, by the item's origin, read inside the append transaction; a database error is
/// never permission. In tests a text holding `DENIED_IN_TESTS` stands for a forgotten one too, so
/// the tests pin where it is asked.
fn denied(conn: &Connection, origin: Option<&str>, text: &str) -> Result<bool> {
    let denied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM denied_records WHERE origin = ?1)",
        [origin],
        |r| r.get(0),
    )?;
    Ok(denied || cfg!(test) && text.contains(DENIED_IN_TESTS))
}

/// Milestone 5 D5: whether a forget op names `uid`.
fn forgotten_uid(conn: &Connection, uid: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ops WHERE type = 'forget' AND json_extract(body, '$.uid') = ?1)",
        [uid],
        |r| r.get(0),
    )?)
}

/// What forget denies: records by origin, and uids by their forget ops. The derived writers'
/// fence (rule 12) and a preview's token count both, so a uid's forget also cuts a window in
/// flight again (D5: rare, and safe).
const DENIED_COUNT: &str = "SELECT (SELECT COUNT(*) FROM denied_records)
  + (SELECT COUNT(*) FROM ops WHERE type = 'forget')";

/// Opposite import sources do not share event identifiers. Keep exact-origin denials, but
/// refuse an import that could reintroduce their copy from the other source.
fn check_cross_source_forget(conn: &Connection, session: &str, source: &str) -> Result<()> {
    let unverified: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM denied_records
         WHERE session=?1 AND (?2 <> 'transcript' OR ts IS NOT NULL))",
        params![session, source],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        !unverified,
        "this import overlaps a forget without a verified cross-source event identity: this batch was not imported"
    );
    Ok(())
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

/// Import provenance before capture changes labels: only hashes, never the native session text.
#[derive(Debug, Clone)]
pub struct ImportIdentity {
    pub origin: String,
    pub session: String,
    /// A repeated event's occurrence-zero origin. Its records stay distinct, but a forget
    /// cannot choose one without a native event identifier.
    pub ambiguous: Option<String>,
    /// The parsed import namespace has no verified native session.
    pub unverified: bool,
}

pub struct Raw {
    conn: Connection,
    home: std::path::PathBuf,
    file_identity: Option<String>,
    device: String,
    /// The store's lineage, kept when a copied file gets a new device for future appends.
    home_id: String,
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

/// Finish only the complete stopped-swap file; a `.restoring` is never an authority.
/// The caller holds raw.lock. Return whether this call performed the rename.
pub(crate) fn finish_stopped_restore(home: &Path) -> Result<bool> {
    let live = home.join("raw.db");
    let restored = path(home);
    if restored == live {
        return Ok(false);
    }
    match std::fs::rename(&restored, &live) {
        Ok(()) => Ok(true),
        Err(_) if live.exists() => Ok(false),
        Err(error) => Err(error).context("finish a stopped restore"),
    }
}

/// A viewer read of an existing raw file. It never creates a store, schema, identity or lock.
/// The connection closes before the shared swap hold; a restore cannot move either store
/// while a privacy reader compares them.
pub(crate) struct ReadOnly {
    pub(crate) conn: Connection,
    pub(crate) sqlite_path: std::path::PathBuf,
    path: std::path::PathBuf,
    identity: String,
    lock_path: std::path::PathBuf,
    _swap: std::fs::File,
}

impl ReadOnly {
    /// Milestone 5 D5: whether `uid` is forgotten.
    pub(crate) fn forgotten(&self, uid: &str) -> Result<bool> {
        forgotten_uid(&self.conn, uid)
    }

    /// Windows cannot rename a file with an open SQLite handle. Close that connection, keeping
    /// the original swap hold and identity until `open` owns a hold on the same recovered file.
    pub(crate) fn into_writer(self, home: &Path) -> Result<Raw> {
        self.current()?;
        let Self {
            conn,
            mut sqlite_path,
            mut path,
            identity,
            lock_path,
            _swap: swap,
        } = self;
        conn.close().map_err(|(_, e)| e)?;
        let raw = open(home)?;
        if path.file_name() == Some(std::ffi::OsStr::new("raw.db.restored")) {
            path.set_file_name("raw.db");
            sqlite_path.set_file_name("raw.db");
        }
        current_read_file(&path, &sqlite_path, &identity, &lock_path, &swap)?;
        drop(swap);
        Ok(raw)
    }

    /// A home removed or replaced while a read was waiting must not answer from the old file.
    pub(crate) fn current(&self) -> Result<()> {
        current_read_file(
            &self.path,
            &self.sqlite_path,
            &self.identity,
            &self.lock_path,
            &self._swap,
        )
    }
}

fn current_read_file(
    path: &Path,
    sqlite_path: &Path,
    identity: &str,
    lock_path: &Path,
    swap: &std::fs::File,
) -> Result<()> {
    let regular = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file());
    anyhow::ensure!(
        !identity.is_empty()
            && regular(path)
            && regular(sqlite_path)
            && regular(lock_path)
            && crate::db::store_file(path) == identity
            && crate::db::store_file(sqlite_path) == identity
            && crate::worker::file_id(std::fs::metadata(lock_path))
                == crate::worker::file_id(swap.metadata()),
        "raw store changed during a read"
    );
    Ok(())
}

fn existing_read_path(home: &Path) -> Result<Option<std::path::PathBuf>> {
    for name in ["raw.db", "raw.db.restored"] {
        let path = home.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(metadata.is_file(), "raw store is not a regular file");
                return Ok(Some(path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("read raw store"),
        }
    }
    Ok(None)
}

pub(crate) fn read_only(home: &Path) -> Result<Option<ReadOnly>> {
    let before = existing_read_path(home)?;
    let lock_path = home.join("raw.lock");
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "raw coordination is not a regular file")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && before.is_none() => {
            return Ok(None);
        }
        Err(e) => return Err(e).context("read raw coordination"),
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NOFOLLOW | libc::O_NONBLOCK,
    );
    let file = options.open(&lock_path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "raw coordination is not a regular file"
    );
    let swap = wait_for_swap(file, false, OPEN_WAIT)?;
    anyhow::ensure!(
        crate::worker::file_id(std::fs::metadata(&lock_path))
            == crate::worker::file_id(swap.metadata()),
        "raw coordination changed during a read"
    );
    let Some(path) = existing_read_path(home)? else {
        anyhow::ensure!(before.is_none(), "raw store disappeared during a read");
        return Ok(None);
    };
    let identity = crate::db::store_file(&path);
    // SQLite NOFOLLOW rejects ancestor aliases too. Resolve only the
    // home, retaining the leaf check and the original path's identity for later validation.
    let sqlite_path = home
        .canonicalize()?
        .join(path.file_name().context("raw filename")?);
    anyhow::ensure!(
        !identity.is_empty() && crate::db::store_file(&sqlite_path) == identity,
        "raw store changed before a read"
    );
    let conn = Connection::open_with_flags(
        &sqlite_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    conn.busy_timeout(OPEN_WAIT)?;
    let read = ReadOnly {
        conn,
        sqlite_path,
        path,
        identity,
        lock_path,
        _swap: swap,
    };
    read.current()?;
    Ok(Some(read))
}

pub(crate) fn imported_counts_in(
    conn: &Connection,
    device: &str,
    ranges: &[(i64, i64)],
) -> Result<std::collections::BTreeMap<String, i64>> {
    let mut st = conn.prepare(
        "SELECT source, count(*) FROM records
             WHERE device = ?1 AND seq BETWEEN ?2 AND ?3 AND type = 'event' AND kind != 'touch'
             GROUP BY source",
    )?;
    let mut out = std::collections::BTreeMap::<String, i64>::new();
    for &(from, to) in ranges {
        for row in st.query_map(params![device, from, to], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (source, n) = row?;
            if !is_live(&source) {
                let count = out.entry(source).or_default();
                *count = count.checked_add(n).context("imported count overflow")?;
            }
        }
    }
    Ok(out)
}

pub(crate) fn ops_after_in(
    conn: &Connection,
    device: &str,
    op_seq: i64,
    limit: usize,
) -> Result<Vec<Op>> {
    op_rows_in(conn, device, op_seq, limit)?
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

fn op_rows_in(conn: &Connection, device: &str, op_seq: i64, limit: usize) -> Result<Vec<OpRow>> {
    let mut st = conn.prepare(
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

pub(crate) fn curation_checkpoint_in(
    conn: &Connection,
    device: &str,
) -> Result<(i64, Option<i64>)> {
    use rusqlite::OptionalExtension;
    let last = conn
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

pub(crate) fn max_seq_in(conn: &Connection, device: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM records WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?)
}

pub(crate) fn max_op_seq_in(conn: &Connection, device: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(op_seq), 0) FROM ops WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?)
}

pub(crate) fn integrity_check_in(conn: &Connection) -> Result<()> {
    let first: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    anyhow::ensure!(first == "ok", "raw.db integrity_check: {first}");
    Ok(())
}

pub(crate) fn migration_checkpoints_in(
    conn: &Connection,
    prefix: &str,
) -> Result<std::collections::HashMap<String, Checkpoint>> {
    let mut statement = conn.prepare(
        "SELECT op_seq, body FROM ops WHERE type = 'migration'
         AND substr(json_extract(body, '$.key'), 1, length(?1)) = ?1",
    )?;
    let rows = statement.query_map([prefix], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = std::collections::HashMap::<String, Checkpoint>::new();
    for row in rows {
        let (op_seq, body) = row?;
        let checkpoint: Checkpoint = serde_json::from_str(&body)
            .with_context(|| format!("op {op_seq}: a migration body"))?;
        if out
            .get(&checkpoint.key)
            .is_none_or(|kept| kept.through < checkpoint.through)
        {
            out.insert(checkpoint.key.clone(), checkpoint);
        }
    }
    Ok(out)
}

/// docs/work-state.md L4: `repo`'s work state writes, in the order they were written: a device's
/// in its own order, its clock never going back in it, then every device's by that clock, as
/// `exclusions_in` orders exclusions. With them, the entries imported for the claude-mem project
/// its key ends in, its worktrees' too, at their own times (docs/claude-mem-import.md I4, I5).
/// Only these ops are parsed here (Codex's security review of #408); SQLite still reads the
/// repository of every work state op, as no index may have a repository for its root (spec 1.6),
/// and a malformed op still fails the read.
pub(crate) fn work_state_in(
    conn: &Connection,
    repo: &str,
) -> Result<Vec<crate::work_state::Entry>> {
    let (scope, values) = crate::import::imported_match("json_extract(body, '$.repo')", repo);
    // The name reads imported entries only, as search's reads imported documents only: a native
    // entry is read by its own key alone, though an origin like `ssh://claude-mem:<name>/x` gives
    // that key a worktree project's shape (the security review of the N1 commit).
    let mut st = conn.prepare(&format!(
        "SELECT device, ts, body FROM main.ops
         WHERE type = 'work_state' AND {scope}
           AND (json_extract(body, '$.source') IS NOT NULL OR json_extract(body, '$.repo') = ?)
         ORDER BY device, op_seq"
    ))?;
    let mut writes = Vec::new();
    let (mut device, mut clock) = (String::new(), i64::MIN);
    let values = values
        .into_iter()
        .chain([rusqlite::types::Value::Text(repo.to_owned())]);
    let rows = st.query_map(rusqlite::params_from_iter(values), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for (i, row) in rows.enumerate() {
        let (from, ts, body) = row?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        if from != device {
            (device, clock) = (from.clone(), i64::MIN);
        }
        let own = v["clock"].as_i64().unwrap_or(ts);
        // An imported entry was written elsewhere, before or after this device's own writes: it
        // takes effect at its own time, or at its import when that is earlier (a clock ahead of
        // this one), and lifts no clock of theirs.
        let at = if v.get("source").is_some() {
            own.min(ts)
        } else {
            clock = clock.max(own);
            clock
        };
        let (Some(name), Some(fields)) = (v["list"].as_str(), v["fields"].as_object()) else {
            continue;
        };
        let entry = crate::work_state::Entry {
            list: name.to_owned(),
            fields: fields.clone(),
            ts: v["at"].as_i64().unwrap_or(ts),
        };
        writes.push((at, from, i, entry));
    }
    writes.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    Ok(writes.into_iter().map(|w| w.3).collect())
}

/// The same current list for an existing read-only connection: no indexes or schema writes.
pub(crate) fn exclusions_in(conn: &Connection) -> Result<Vec<String>> {
    // A device's ops in its own order (op_seq), its clock never going back in it, then every
    // device's by that clock: a clock set back never puts a newer op first, and an op
    // `exclude` wrote comes after every op its store held (Codex on #304).
    let mut st = conn.prepare(
        "SELECT device, ts, body FROM main.ops WHERE type = 'exclusion' ORDER BY device, op_seq",
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

/// `<home>/raw.db`: WAL, synchronous=FULL (and fullfsync on macOS), 2 s SQLite busy timeout.
/// Non-hook opens share a 10 s initialization deadline; hooks use `open_within` with 2 s.
pub fn open(home: &Path) -> Result<Raw> {
    open_within(home, crate::db::OPEN_WRITE_WAIT)
}

/// Open with one lock-wait budget for restore, WAL, schema, column and device initialization.
/// Hooks pass 2 s so a failed open reaches MUST-M16's marker before the agent kills the hook.
pub fn open_within(home: &Path, wait: std::time::Duration) -> Result<Raw> {
    open_within_report(home, wait, None, &mut |_| Ok(()))
}
pub(crate) fn open_report(
    home: &Path,
    guard: Option<&crate::executable::CommandHome>,
    committed: &mut impl FnMut(&'static str) -> Result<()>,
) -> Result<Raw> {
    open_within_report(home, crate::db::OPEN_WRITE_WAIT, guard, committed)
}
fn open_within_report(
    home: &Path,
    wait: std::time::Duration,
    guard: Option<&crate::executable::CommandHome>,
    committed: &mut impl FnMut(&'static str) -> Result<()>,
) -> Result<Raw> {
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    let deadline = std::time::Instant::now() + wait;
    let path = home.join("raw.db");
    crate::db::private(home, 0o700);
    let swap = swap_lock(
        home,
        false,
        OPEN_WAIT.min(deadline.saturating_duration_since(std::time::Instant::now())),
    )?;
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    // A restore that stopped after moving the damaged file aside and before renaming the rebuilt
    // one in: `raw.db.restored` is only ever a whole rebuild (it gets that name once its records
    // are committed), so the rename is finished here instead of creating an empty store.
    if finish_stopped_restore(home)? {
        committed("stopped_restore_finished")?;
    }
    // Give first creation the same nonempty identity witness as an existing file. This is
    // after stopped-restore recovery and under the swap hold; an interrupted empty creation is
    // the same state Connection::open already leaves before its first schema transaction.
    let mut create = std::fs::OpenOptions::new();
    create.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    match create.open(&path) {
        Ok(_) => committed("stores_changed")?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("create {}", path.display())),
    }
    let file_before = crate::db::store_file(&path);
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    // A changed path invalidates send/registration admission without introducing a new open
    // failure: the resident worker's existing before/after observation reopens the stores.
    let mut bound = !file_before.is_empty() && crate::db::store_file(&path) == file_before;
    #[cfg(test)]
    crate::crash::arm(&conn);
    crate::db::wal_until(&conn, "FULL", deadline)?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON;")?;
    // Milestone 5 D5 (2b): what a write frees is overwritten with zeros, so neither a rewritten op
    // body nor a deleted row leaves its bytes in the file.
    conn.execute_batch("PRAGMA secure_delete=ON;")?;
    if crate::db::ensure_schema_until(&conn, &schema_for_file(&conn, &path)?, deadline)
        .context("raw schema")?
    {
        committed("stores_changed")?;
    }
    // A raw.db from before the ledger named its field (milestone 2 Task 1's schema).
    if crate::db::ensure_column_until(
        &mut conn,
        "ledger",
        "field",
        "TEXT NOT NULL DEFAULT ''",
        deadline,
    )
    .context("migrate ledger")?
    {
        committed("stores_changed")?;
    }
    if crate::db::ensure_column_until(
        &mut conn,
        "import_origins",
        "native_session",
        "TEXT",
        deadline,
    )
    .context("migrate import session identity")?
    {
        committed("stores_changed")?;
    }
    if crate::db::ensure_column_until(
        &mut conn,
        "import_origins",
        "ambiguous",
        "INTEGER NOT NULL DEFAULT 1",
        deadline,
    )
    .context("migrate import identity confidence")?
    {
        committed("stores_changed")?;
    }
    if crate::db::ensure_column_until(&mut conn, "records", "deny_origin", "TEXT", deadline)
        .context("migrate forget control identity")?
    {
        committed("stores_changed")?;
    }
    for file in ["raw.db", "raw.db-wal", "raw.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    // A file copied into this home gets a new device, but keeps its proven lineage for forget
    // logs. A legacy store keeps its old device as an unverified label before it changes;
    // fresh stores already have the atomic schema id. Existing metadata needs no hook write.
    use rusqlite::OptionalExtension;
    let known_home: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='home_id'", [], |r| {
            r.get(0)
        })
        .optional()?;
    let previous_device = if known_home.is_none() {
        conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
    } else {
        None
    };
    let seed_home = |id: &str| {
        crate::db::retry_busy(&conn, deadline, || {
            let changed = conn.execute(
                "INSERT OR IGNORE INTO meta(key, value) VALUES('home_id', ?1)",
                [id],
            )?;
            Ok(changed != 0)
        })
    };
    // Seed before changing a copied store's device, so two simultaneous opens cannot choose
    // the new device as its lineage in the gap between the two writes.
    if let Some(id) = &previous_device
        && seed_home(id)?
    {
        committed("stores_changed")?;
    }
    bound &= crate::db::store_file(&path) == file_before;
    if crate::db::ensure_device_until(&conn, &path, deadline).context("device id")? {
        committed("stores_changed")?;
    }
    let file_identity: String =
        conn.query_row("SELECT value FROM meta WHERE key='store_file'", [], |r| {
            r.get(0)
        })?;
    let file_identity =
        (bound && file_identity == file_before && crate::db::store_file(&path) == file_before)
            .then_some(file_identity);
    let device = conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
        r.get::<_, String>(0)
    })?;
    let home_id = match known_home {
        Some(id) => id,
        None => {
            if previous_device.is_none() && seed_home(&device)? {
                committed("stores_changed")?;
            }
            conn.query_row("SELECT value FROM meta WHERE key='home_id'", [], |r| {
                r.get(0)
            })?
        }
    };
    Ok(Raw {
        conn,
        home: home.to_owned(),
        file_identity,
        device,
        home_id,
        _swap: swap,
    })
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

#[cfg(test)]
thread_local! {
    // One-shot for the next blocked shared or exclusive wait; callers reset an unused hook.
    pub(crate) static SWAP_BLOCKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

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
    wait_for_swap(f, exclusive, wait)
}

fn wait_for_swap(
    f: std::fs::File,
    exclusive: bool,
    wait: std::time::Duration,
) -> Result<std::fs::File> {
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
                #[cfg(test)]
                if let Some(blocked) = SWAP_BLOCKED.with(|hook| hook.borrow_mut().take()) {
                    blocked();
                }
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
    /// Order a sender's final read with forget, without holding a SQLite transaction.
    pub(crate) fn dispatch(&self) -> Result<crate::dispatch::Guard> {
        let admission = crate::dispatch::Admission::shared(&self.home)?;
        self.current()?;
        Ok(admission)
    }

    fn current(&self) -> Result<()> {
        let current = crate::db::store_file(&self.home.join("raw.db"));
        if current.is_empty() || self.file_identity.as_deref() != Some(current.as_str()) {
            return Err(anyhow::Error::new(crate::curate::ListChanged))
                .context("raw.db was replaced: reopen before dispatch or registration");
        }
        Ok(())
    }

    /// What forget would register for `target` (D1): each imported record's origin, session hash
    /// and cut time, read on this connection, so `forget_start` reads it again inside its
    /// transaction. A record with no import origin is refused: it cannot be told from a record
    /// that reuses its seq (slice 3a gives live records one).
    pub(crate) fn forget_preview(
        &self,
        target: crate::forget::Target,
    ) -> Result<crate::forget::Preview> {
        let (device, from, to) = target.bounds()?;
        anyhow::ensure!(
            device == self.device,
            "this first forget slice accepts only this device's raw records"
        );
        anyhow::ensure!(
            self.home_id_proven()?,
            "this store has no verified home identity: first-slice forget was not registered"
        );
        let (mut records, mut sources, mut sample) = (Vec::new(), Vec::<String>::new(), None);
        let mut at = from - 1;
        while at < to {
            let take = usize::try_from(to - at)
                .unwrap_or(usize::MAX)
                .min(crate::forget::MAX_RECORDS + 1);
            let batch = self.after_within(device, at, take, MAX_BATCH_BYTES)?;
            let Some(last) = batch.last().map(|r| r.seq) else {
                break;
            };
            for r in batch.into_iter().filter(|r| r.seq <= to) {
                if !matches!(r.item, Item::Event(_) | Item::Removed) {
                    continue;
                }
                let identity = self.origin_of(device, r.seq)?;
                let (origin, session, ambiguous) = identity.with_context(|| {
                    format!(
                        "record {device}:{} has no import identity: this slice forgets imported \
                         records only; nothing was registered",
                        r.seq
                    )
                })?;
                if denied(&self.conn, Some(&origin), "")? {
                    continue;
                }
                anyhow::ensure!(
                    !ambiguous,
                    "record {device}:{} has an ambiguous or unverified import identity: nothing was registered",
                    r.seq
                );
                let session = session.with_context(|| {
                    format!(
                        "record {device}:{} has no native session identity: nothing was registered",
                        r.seq
                    )
                })?;
                let (source, kind, ts) = match r.item {
                    Item::Event(e) => {
                        if sample.is_none() {
                            sample = Some(e.body.chars().take(120).collect());
                        }
                        (e.source, e.kind, e.ts)
                    }
                    Item::Removed => self
                        .original_import_metadata(device, r.seq)?
                        .with_context(|| format!("record {device}:{} has no original import metadata: nothing was registered", r.seq))?,
                    _ => continue,
                };
                anyhow::ensure!(
                    source != "oboete-v1",
                    "record {device}:{} has no verified cross-source event identity: first-slice v1 forget was not registered; nothing was registered",
                    r.seq
                );
                if !sources.contains(&source) {
                    sources.push(source.clone());
                }
                records.push(crate::forget::Record {
                    device: device.into(),
                    seq: r.seq,
                    origin,
                    session,
                    // Rule 10: only a record the transcript cut counts keeps its time.
                    ts: (source != "transcript" && kind != "touch").then_some(ts),
                });
                anyhow::ensure!(
                    records.len() <= crate::forget::MAX_RECORDS,
                    "split this selection into spans of at most {} records",
                    crate::forget::MAX_RECORDS
                );
            }
            at = last;
        }
        if !records.is_empty() {
            // Native records without provenance cannot be proved to belong to another
            // namespace. Never infer identity from a redacted live session label.
            let placeholders = vec!["?"; records.len()].join(",");
            let sql = format!(
                "SELECT EXISTS(SELECT 1 FROM records r LEFT JOIN import_origins o
                   ON o.device=r.device AND o.seq=r.seq
                 WHERE r.type IN ('event','removed')
                   AND (r.source IS NULL OR r.source <> 'transcript')
                   AND COALESCE(r.kind,'') <> 'touch'
                   AND (o.native_session IS NULL OR o.native_session IN ({placeholders})))"
            );
            let unverified: bool = self.conn.query_row(
                &sql,
                rusqlite::params_from_iter(records.iter().map(|r| &r.session)),
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                !unverified,
                "this selection may have unverified native copies: first-slice forget was not registered; nothing was registered"
            );
        }
        Ok(crate::forget::Preview {
            target,
            records,
            denied: self.denied_count()?,
            sources,
            sample,
            uid: None,
        })
    }

    /// Registers `preview` as job `job` in one raw write transaction (D1 rule 1): read again and
    /// compared under the write lock, so a record replaced since, or another forget since,
    /// makes it stale and nothing is written. The request, for the logs.
    pub(crate) fn forget_start(
        &mut self,
        preview: &crate::forget::Preview,
        job: &str,
        started: i64,
    ) -> Result<crate::forget::Request> {
        preview.validate()?;
        let _dispatch =
            crate::dispatch::exclusive(&self.home).context("forget was not registered")?;
        self.current()?;
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        let again = match &preview.target {
            // D5: its index is knowledge.db, which is not read under raw's lock (rule 2): raw's
            // part is read again, the denials.
            crate::forget::Target::Uid { uid } => {
                anyhow::ensure!(!forgotten_uid(&tx, uid)?, "{uid} is forgotten already");
                crate::forget::Preview {
                    denied: self.denied_count()?,
                    ..preview.clone()
                }
            }
            target => self.forget_preview(target.clone())?,
        };
        anyhow::ensure!(
            again.token()? == preview.token()?,
            "forget preview is stale; preview again"
        );
        let uid = matches!(preview.target, crate::forget::Target::Uid { .. });
        let request = crate::forget::Request {
            v: if uid { 2 } else { 1 },
            home: self.home_id.clone(),
            job: job.into(),
            started,
            target: preview.target.clone(),
            records: preview.records.clone(),
        };
        request.check()?;
        apply_request(&tx, &self.device, &request)?;
        tx.commit()?;
        Ok(request)
    }

    /// Applies each of `requests` that raw does not hold, by identity (rule 5), in one
    /// transaction: how many it applied.
    pub(crate) fn forget_apply(&mut self, requests: &[crate::forget::Request]) -> Result<usize> {
        if requests.is_empty() {
            return Ok(0);
        }
        let _dispatch = crate::dispatch::exclusive(&self.home)?;
        self.current()?;
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        let mut applied = 0;
        for r in requests {
            r.check()?;
            applied += usize::from(apply_request(&tx, &self.device, r)?);
        }
        tx.commit()?;
        Ok(applied)
    }

    /// Every request raw holds, oldest first (rule 4).
    pub(crate) fn forget_requests(&self) -> Result<Vec<crate::forget::Request>> {
        let mut st = self
            .conn
            .prepare("SELECT request FROM forget_jobs ORDER BY started, id")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// How many records forget denies: the derived writers' fence (rule 12).
    pub fn denied_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(DENIED_COUNT, [], |r| r.get(0))?)
    }

    /// D5: `uid`'s ops in the op log when knowledge.db has not read them yet (right after an
    /// import, or while the worker lags; Codex on #435): a claim's, its uid computed as the
    /// consumer computes it, with the corrections that name it, or a document's import ops. Its
    /// kind, how many ops and what to show first; None when the log holds none.
    pub fn uid_ops(&self, uid: &str) -> Result<Option<(&'static str, usize, String)>> {
        if uid.len() == 64 && uid.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut st = self
                .conn
                .prepare("SELECT body FROM ops WHERE type = 'claim'")?;
            let mut rows = st.query([])?;
            let (mut n, mut first) = (0, None);
            while let Some(row) = rows.next()? {
                let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(0)?)
                else {
                    continue;
                };
                if crate::claims::op_uid(&body).as_deref() == Some(uid) {
                    n += 1;
                    first.get_or_insert_with(|| body["body"].as_str().unwrap_or("").to_owned());
                }
            }
            let corrections: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM ops WHERE type = 'correction'
                 AND json_extract(body, '$.uid') = ?1",
                [uid],
                |r| r.get(0),
            )?;
            if n > 0 {
                let ops = n + usize::try_from(corrections)?;
                return Ok(Some(("claim", ops, first.unwrap_or_default())));
            }
            // No claim has it: an import op may still carry it (CodeRabbit on #435).
        }
        let (n, title): (i64, Option<String>) = self.conn.query_row(
            "SELECT COUNT(*), MIN(json_extract(body, '$.title')) FROM ops
             WHERE type = 'import' AND json_extract(body, '$.uid') = ?1",
            [uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((n > 0).then(|| {
            (
                "document",
                usize::try_from(n).unwrap_or(0),
                title.unwrap_or_default(),
            )
        }))
    }

    /// Milestone 5 D5: whether `uid`, a claim's or an imported document's, is forgotten.
    pub fn forgotten(&self, uid: &str) -> Result<bool> {
        forgotten_uid(&self.conn, uid)
    }

    /// Every forgotten uid (D5): the readers and senders pass them over.
    pub fn forgotten_set(&self) -> Result<std::collections::HashSet<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT json_extract(body, '$.uid') FROM ops
             WHERE type = 'forget' AND json_extract(body, '$.uid') IS NOT NULL",
        )?;
        let uids = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(uids)
    }

    /// Milestone 5 D5 (2b): every op of `uid` rewritten to `{"forgotten": job}`, in one write
    /// transaction taken in registration's order (dispatch, then raw): type, op_seq, ts and batch
    /// stay. How many it rewrote.
    pub(crate) fn purge_uid(&mut self, uid: &str, job: &str) -> Result<usize> {
        let _dispatch = crate::dispatch::exclusive(&self.home)
            .context("the forgotten uid's ops were not purged")?;
        self.current()?;
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        let keys = uid_op_keys(&tx, uid)?;
        let body = serde_json::json!({ "forgotten": job }).to_string();
        {
            let mut st =
                tx.prepare("UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3")?;
            for (device, op_seq) in &keys {
                st.execute(params![body, device, op_seq])?;
            }
        }
        tx.commit()?;
        Ok(keys.len())
    }

    /// Whether raw holds an op of `uid` with its body (D5: its purge's step 2 is not done).
    pub(crate) fn uid_has_ops(&self, uid: &str) -> Result<bool> {
        Ok(!uid_op_keys(&self.conn, uid)?.is_empty())
    }

    /// Milestone 5 D5 (2b): the WAL checkpointed and truncated, so no frame from before a purge
    /// stays in raw.db-wal; tried every 100 ms for `wait`. False when readers kept it from
    /// truncating.
    pub(crate) fn truncate_wal(&self, wait: Duration) -> Result<bool> {
        // Each try returns at once, a busy one too: the sleeps here pace them.
        let timeout: u32 = self
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
        self.conn.busy_timeout(Duration::ZERO)?;
        let until = Instant::now() + wait;
        let truncated = loop {
            let tried = self
                .conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                    r.get::<_, i64>(0)
                });
            let busy = match tried {
                Ok(busy) => busy != 0,
                Err(e)
                    if matches!(
                        e.sqlite_error_code(),
                        Some(
                            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                        )
                    ) =>
                {
                    true
                }
                Err(e) => break Err(e),
            };
            if !busy {
                break Ok(true);
            }
            if Instant::now() >= until {
                break Ok(false);
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.conn
            .busy_timeout(Duration::from_millis(timeout.into()))?;
        Ok(truncated?)
    }

    /// Each request raw holds with its step (D5: 1 registered, 2 its ops' bodies purged), oldest
    /// first.
    pub(crate) fn forget_jobs(&self) -> Result<Vec<(crate::forget::Request, i64)>> {
        let mut st = self
            .conn
            .prepare("SELECT request, step FROM forget_jobs ORDER BY started, id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (request, step) = row?;
            out.push((serde_json::from_str(&request)?, step));
        }
        Ok(out)
    }

    /// Records that `job` has finished `step`; a step it passed already stays.
    pub(crate) fn finish_forget_step(&self, job: &str, step: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE forget_jobs SET step = MAX(step, ?2) WHERE id = ?1",
            params![job, step],
        )?;
        Ok(())
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub(crate) fn home_id(&self) -> &str {
        &self.home_id
    }

    pub(crate) fn home_id_proven(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='home_id_proven' AND value='1')",
            [],
            |r| r.get(0),
        )?)
    }

    /// SQLite's `quick_check` on raw.db: an error names the first problem it reports.
    pub fn quick_check(&self) -> Result<()> {
        crate::db::quick_check(&self.conn, "raw.db")
    }

    /// SQLite's full `integrity_check` (every page): doctor only.
    pub fn integrity_check(&self) -> Result<()> {
        integrity_check_in(&self.conn)
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

    /// `append_imported` with each record's native identity, hashed by its importer
    /// (`forget::origin`): kept beside the record, and a record whose origin forget denies is not
    /// recorded (D1 rule 5). After any forget, an import without identities is refused: nothing
    /// could tell a forgotten record in it.
    pub fn append_imported_origins(
        &mut self,
        batch: &[crate::capture::Captured],
        origins: &[ImportIdentity],
        ruleset: &str,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Vec<i64>> {
        anyhow::ensure!(
            origins.is_empty() || origins.len() == batch.len(),
            "import identity count differs from records"
        );
        for identity in origins {
            crate::forget::check_identity(&identity.origin)?;
            crate::forget::check_identity(&identity.session)?;
            if let Some(group) = &identity.ambiguous {
                crate::forget::check_identity(group)?;
            }
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
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        if batch.iter().enumerate().any(|(index, c)| {
            c.event.kind != "touch" && origins.get(index).is_none_or(|i| i.unverified)
        }) {
            let has_denials: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM denied_records)", [], |r| {
                    r.get(0)
                })?;
            anyhow::ensure!(
                !has_denials,
                "cannot import raw without a native source identity after forget; use an importer that preserves verified provenance"
            );
        }
        let from_seq = next_seq(&tx, &self.device)?;
        let mut seqs = Vec::with_capacity(batch.len());
        for (index, c) in batch.iter().enumerate() {
            let identity = origins.get(index);
            let origin = identity.map(|i| i.origin.as_str());
            if let Some(group) = identity.and_then(|i| i.ambiguous.as_ref()) {
                anyhow::ensure!(
                    !denied(&tx, Some(group), "")?,
                    "an ambiguous transcript event overlaps a surviving forget request: this batch was not imported"
                );
                // A duplicate recognized after a checkpoint also marks its existing occurrence
                // zero: forget_start reads this again while holding the write transaction.
                tx.execute(
                    "UPDATE import_origins SET ambiguous=1 WHERE origin=?1",
                    [group],
                )?;
            }
            // A batch prepared before request replay must also prove its correspondence under
            // the write transaction. A timestamp cut cannot identify a native event's copy.
            if denied(&tx, origin, &c.event.body)? {
                continue;
            }
            if c.event.kind != "touch" {
                check_cross_source_forget(
                    &tx,
                    &identity.map_or_else(
                        || crate::forget::session(&c.event.agent, &c.event.session),
                        |i| i.session.clone(),
                    ),
                    &c.event.source,
                )?;
            }
            let seq = insert_event(&tx, &self.device, &c.event, &c.ledger, ruleset)?;
            if let Some(identity) = identity {
                tx.execute(
                    "INSERT INTO import_origins(device, seq, origin, native_session, ambiguous) VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![self.device, seq, identity.origin, identity.session, i64::from(identity.ambiguous.is_some() || identity.unverified)],
                )?;
            }
            seqs.push(seq);
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
        max_seq_in(&self.conn, device)
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

    /// The sessions, as `session_key` spells them, of `device`'s events in the inclusive seq
    /// ranges `spans` and at `seqs`, from the rows alone: what a card or a summary was made from
    /// (docs/cards.md K4, docs/summaries.md T7), which the embedding phase passes it over for
    /// (docs/tools.md V2).
    pub fn sessions_over(
        &self,
        device: &str,
        spans: &[(i64, i64)],
        seqs: &[i64],
    ) -> Result<std::collections::HashSet<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT DISTINCT COALESCE(agent, '') || char(0) || COALESCE(session, '') FROM records
             WHERE device = ?1 AND seq BETWEEN ?2 AND ?3 AND type = 'event'",
        )?;
        let mut out = std::collections::HashSet::new();
        for (from, to) in spans.iter().copied().chain(seqs.iter().map(|&s| (s, s))) {
            for key in st.query_map(params![device, from, to], |r| r.get(0))? {
                out.insert(key?);
            }
        }
        Ok(out)
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
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {RECORD} FROM records WHERE device = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
        ))?;
        let rows = st.query_map(
            params![device, seq, i64::try_from(limit).unwrap_or(i64::MAX)],
            record,
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

    /// The records of `device` at `seqs` as `after` returns them (masked, D8), in seq order; a
    /// seq with no record is left out. A reader of many records' text reads them so, a page at a
    /// time (#317: the raw index keeps no copy of it).
    pub fn at(&self, device: &str, seqs: &[i64]) -> Result<Vec<Record>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {RECORD} FROM records
             WHERE device = ?1 AND seq IN (SELECT value FROM json_each(?2)) ORDER BY seq"
        ))?;
        let mut recs = st
            .query_map(params![device, serde_json::to_string(seqs)?], record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.hide(device, &mut recs)?;
        Ok(recs)
    }
}

/// The columns `record` reads, in its order.
const RECORD: &str = "device, seq, type, ts, kind, agent, session, repo, branch, head, gitdir, \
                      cwd, source, body, original_bytes, \
                      target_device, target_seq, target_offset, target_length, enc";

/// A `records` row, its body decompressed, as `Raw::after` reads it before the masks.
fn record(r: &rusqlite::Row) -> rusqlite::Result<Record> {
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
            body: String::from_utf8(body)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
            original_bytes: r.get(14)?,
        }))
    };
    Ok(Record {
        device: r.get(0)?,
        seq: r.get(1)?,
        item,
    })
}

impl Raw {
    /// Task 8: this device's records after `seq` as backup lines, one JSON object each, as
    /// `Raw::after` returns them (masked, D8) with each event's ledger rows, until `max_bytes`
    /// of lines (always one). Returns (seq, line) pairs in seq order.
    pub fn export_lines(&self, seq: i64, max_bytes: usize) -> Result<Vec<(i64, String)>> {
        let home_id_proven = self.home_id_proven()?;
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
                let mut v: serde_json::Value = serde_json::from_str(&line(r, rows))?;
                // A rescan removes the body, not eligibility to deny its native origin. Keep
                // only the original metadata the forget preview needs for its session cutoff.
                if matches!(r.item, Item::Removed)
                    && let Some((source, kind, ts)) =
                        self.original_import_metadata(&r.device, r.seq)?
                {
                    v["source"] = serde_json::json!(source);
                    v["kind"] = serde_json::json!(kind);
                    v["ts"] = serde_json::json!(ts);
                }
                // The home lineage, unlike the appending device, survives a copied file. Its
                // record backups keep it too, so a lost raw.db still recognizes its logs.
                v["home_id"] = serde_json::json!(self.home_id);
                v["home_id_proven"] = serde_json::json!(home_id_proven);
                // An imported record keeps its origin, and a tombstone of a forgotten one the
                // deny row, so a restore from the segments alone forgets it again (D1 rule 14).
                let extra = match &r.item {
                    Item::Event(_) | Item::Removed => self.origin_of(&r.device, r.seq)?.map(|(origin, session, ambiguous)| {
                        (
                            "import_identity",
                            serde_json::json!({ "origin": origin, "session": session, "ambiguous": ambiguous }),
                        )
                    }),
                    Item::Tombstone(Target::Record { .. }) => {
                        self.deny_of(&r.device, r.seq)?.map(|d| ("deny", d))
                    }
                    _ => None,
                };
                if let Some((key, value)) = extra {
                    v[key] = value;
                }
                let line = serde_json::to_string(&v)?;
                bytes += line.len() + 1;
                out.push((r.seq, line));
                at = r.seq;
                if bytes >= max_bytes {
                    return Ok(out);
                }
            }
        }
    }

    /// An import's original metadata, without any body or labels. Older Removed stubs lack
    /// kind; their restore timestamp and source must never be guessed into a transcript cut.
    fn original_import_metadata(
        &self,
        device: &str,
        seq: i64,
    ) -> Result<Option<(String, String, i64)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT source, kind, ts FROM records WHERE device=?1 AND seq=?2
             AND type IN ('event', 'removed') AND source IN ('oboete-v1', 'transcript')
             AND kind IS NOT NULL AND kind <> ''",
                params![device, seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// An imported record's origin (D1 rule 5).
    fn origin_of(&self, device: &str, seq: i64) -> Result<Option<(String, Option<String>, bool)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT origin, native_session, ambiguous FROM import_origins WHERE device = ?1 AND seq = ?2",
                params![device, seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// A control's stable denial, including when only its device's backup was restored.
    /// Legacy unbound controls use their actual target's provenance, never the request's hints.
    fn deny_of(&self, device: &str, seq: i64) -> Result<Option<serde_json::Value>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT d.origin, d.device, d.seq, d.ts, d.session, d.job
                 FROM records t LEFT JOIN import_origins o
                   ON o.device = t.target_device AND o.seq = t.target_seq
                 JOIN denied_records d ON d.origin = COALESCE(t.deny_origin, o.origin)
                 WHERE t.device = ?1 AND t.seq = ?2 AND t.type = 'tombstone'",
                params![device, seq],
                |r| {
                    Ok(serde_json::json!({
                        "origin": r.get::<_, String>(0)?, "device": r.get::<_, String>(1)?,
                        "seq": r.get::<_, i64>(2)?, "ts": r.get::<_, Option<i64>>(3)?,
                        "session": r.get::<_, String>(4)?, "job": r.get::<_, String>(5)?,
                    }))
                },
            )
            .optional()?)
    }

    /// D1: `ops` as this device's next op seqs, in one transaction: a window op and the claims
    /// it yields commit together, and with them the curation checkpoint (D2).
    pub fn append_ops(&mut self, ops: &[(OpKind, serde_json::Value)]) -> Result<Vec<i64>> {
        let bodies = within_batch_cap(ops)?;
        let tx = begin_batch(&self.conn, Duration::ZERO)?;
        for (kind, body) in ops {
            anyhow::ensure!(
                !forgotten_op(&tx, *kind, body)?,
                "a forget request invalidated this derived batch; it was not recorded"
            );
        }
        let seqs = insert_ops(&tx, &self.device, &bodies)?;
        tx.commit()?;
        Ok(seqs)
    }

    /// `append_ops` for a writer of derived text (milestone 5 D1 rule 12): only while forget
    /// denies as many records as when its inputs were read (`denied`), compared inside the write;
    /// else nothing is written and the error is `curate::ListChanged`.
    pub fn append_ops_fenced(
        &mut self,
        ops: &[(OpKind, serde_json::Value)],
        denied: i64,
    ) -> Result<Vec<i64>> {
        let bodies = within_batch_cap(ops)?;
        let tx = begin_batch(&self.conn, Duration::ZERO)?;
        let now: i64 = tx.query_row(DENIED_COUNT, [], |r| r.get(0))?;
        if now != denied {
            return Err(crate::curate::ListChanged.into());
        }
        for (kind, body) in ops {
            anyhow::ensure!(
                !forgotten_op(&tx, *kind, body)?,
                "a forget request invalidated this derived batch; it was not recorded"
            );
        }
        let seqs = insert_ops(&tx, &self.device, &bodies)?;
        tx.commit()?;
        Ok(seqs)
    }

    /// Up to `limit` ops of `device` after `op_seq`, in op_seq order.
    pub fn ops_after(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<Op>> {
        ops_after_in(&self.conn, device, op_seq, limit)
    }

    fn op_rows(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<OpRow>> {
        op_rows_in(&self.conn, device, op_seq, limit)
    }

    /// The repositories excluded now (spec 5.5, milestone 4 D13): every device's exclusion ops in
    /// time order, an undo taking its repository back out. Read before each outbound call, with no
    /// consumer in between; with no hub, this device's list is the whole list.
    pub fn exclusions(&self) -> Result<Vec<String>> {
        exclusions_in(&self.conn)
    }

    /// Appends an exclusion op, or its undo (spec 5.5, D13), with a clock past every exclusion op
    /// this store holds, as a Lamport clock: it takes effect after each op its device had seen,
    /// whatever the clocks of the devices that wrote them. A copied store keeps its old device's
    /// ops under a new device (Codex on #304).
    pub fn exclude(&mut self, repo: &str, undo: bool) -> Result<i64> {
        let _dispatch = crate::dispatch::exclusive(&self.home)?;
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

    /// docs/work-state.md L4: one write of an agent's work state, `{repo, list, fields, clock}`,
    /// as this device's next op; `list` and `fields` have passed the gate (L5). Its clock is one
    /// past every work state clock the store holds and at least now, as an exclusion's; an
    /// imported entry's time, another clock's, lifts none (docs/claude-mem-import.md I5). No
    /// dispatch lock: a work state write orders against no provider call.
    pub fn work_state(
        &mut self,
        repo: &str,
        list: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<i64> {
        let seen: Option<i64> = self.conn.query_row(
            "SELECT MAX(COALESCE(json_extract(body, '$.clock'), ts)) FROM ops
             WHERE type = 'work_state' AND json_extract(body, '$.source') IS NULL",
            [],
            |r| r.get(0),
        )?;
        let clock = seen.map_or(i64::MIN, |c| c + 1).max(crate::db::now_ms());
        let op = serde_json::json!({"repo": repo, "list": list, "fields": fields, "clock": clock});
        Ok(self.append_ops(&[(OpKind::WorkState, op)])?[0])
    }

    /// `work_state_in` on this store.
    pub fn work_state_entries(&self, repo: &str) -> Result<Vec<crate::work_state::Entry>> {
        work_state_in(&self.conn, repo)
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
        imported_counts_in(&self.conn, device, ranges)
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
        max_op_seq_in(&self.conn, device)
    }

    /// The `source_id`s of `source`'s documents imported so far, on any device: what an import
    /// skips (D5). An op whose `source_id` is missing or not text, which this version cannot read,
    /// is passed over as the consumer passes it (OpenCodeReview on #305).
    pub fn import_keys(&self, source: &str) -> Result<std::collections::HashSet<String>> {
        self.source_ids("import", source)
    }

    /// The claude-mem work state rows already imported from `source`, by their ids
    /// (docs/claude-mem-import.md I4).
    pub fn work_state_keys(&self, source: &str) -> Result<std::collections::HashSet<String>> {
        self.source_ids("work_state", source)
    }

    /// The documents imported from `source`, each id with every repository an import op files it
    /// under, on any device: a row claude-mem moved since (a `ProjectMerge`) comes in again
    /// (docs/claude-mem-import.md I3). A set, not the newest: op seqs order one device's ops only
    /// (Codex on #431).
    pub fn import_repos(
        &self,
        source: &str,
    ) -> Result<std::collections::HashMap<String, std::collections::HashSet<String>>> {
        let mut st = self.conn.prepare(
            "SELECT json_extract(body, '$.source_id'), COALESCE(json_extract(body, '$.repo'), '')
             FROM ops WHERE type = 'import' AND json_extract(body, '$.source') = ?1
               AND typeof(json_extract(body, '$.source_id')) = 'text'",
        )?;
        let mut out = std::collections::HashMap::<String, std::collections::HashSet<String>>::new();
        for row in st.query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (id, repo) = row?;
            out.entry(id).or_default().insert(repo);
        }
        Ok(out)
    }

    fn source_ids(&self, kind: &str, source: &str) -> Result<std::collections::HashSet<String>> {
        let mut st = self.conn.prepare(
            "SELECT json_extract(body, '$.source_id') FROM ops
             WHERE type = ?1 AND json_extract(body, '$.source') = ?2
               AND typeof(json_extract(body, '$.source_id')) = 'text'",
        )?;
        let rows = st.query_map([kind, source], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The furthest checkpoint of each import whose key starts with `prefix`, on any device: where
    /// its next pass starts (D6).
    pub fn migration_checkpoints(
        &self,
        prefix: &str,
    ) -> Result<std::collections::HashMap<String, Checkpoint>> {
        migration_checkpoints_in(&self.conn, prefix)
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

    /// Ordinary capture still defines the import time cut. A surviving native forget without
    /// a cross-source event identity instead refuses the batch; its receipt time is no proof.
    pub(crate) fn transcript_cut(
        &self,
        agent: &str,
        session: &str,
        recorded: Option<i64>,
    ) -> Result<Option<i64>> {
        check_cross_source_forget(
            &self.conn,
            &crate::forget::session(agent, session),
            "transcript",
        )?;
        Ok(recorded)
    }

    /// `docs` as this device's `import` ops (D5), in appends of at most `IMPORT_BATCH` documents
    /// and `MAX_BATCH_BYTES`, each its own batch: an import stopped midway keeps what it appended.
    /// A body that would take its op over `MAX_OP_BYTES` is clipped with a marker; a document
    /// `denied` asks to leave out is not appended. The ops appended.
    pub fn append_imports(&mut self, docs: Vec<ImportDoc>) -> Result<usize> {
        self.append_imports_counted(docs, |_| {})
    }

    /// Notify only after each bounded batch commits; earlier counts survive a later error.
    pub(crate) fn append_imports_counted(
        &mut self,
        docs: Vec<ImportDoc>,
        mut committed: impl FnMut(u64),
    ) -> Result<usize> {
        let (mut batch, mut bytes, mut appended) = (Vec::new(), 0, 0);
        for doc in docs {
            let text = format!("{}\n{}", doc.title, doc.body);
            let origin = crate::forget::origin(&doc.source, &doc.source_id);
            // A forgotten document (D5) is passed over here: `append_ops` refuses a batch with it.
            if denied(&self.conn, Some(&origin), &text)? || forgotten_uid(&self.conn, &doc.uid)? {
                continue;
            }
            let body = serde_json::to_value(within_op_cap(doc)?)?;
            let size = body.to_string().len();
            if batch.len() == IMPORT_BATCH || bytes + size > MAX_BATCH_BYTES {
                let count = self.append_ops(&std::mem::take(&mut batch))?.len();
                appended += count;
                committed(count as u64);
                bytes = 0;
            }
            bytes += size;
            batch.push((OpKind::Import, body));
        }
        if !batch.is_empty() {
            let count = self.append_ops(&batch)?.len();
            appended += count;
            committed(count as u64);
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
        curation_checkpoint_in(&self.conn, device)
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
        let mut st = self.conn.prepare_cached(
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
    #[cfg(test)]
    pub fn compress_through(&self, device: &str, after: i64, through: i64) -> Result<usize> {
        self.compress_through_report(device, after, through, &mut || {})
    }

    pub(crate) fn compress_through_report(
        &self,
        device: &str,
        after: i64,
        through: i64,
        committed: &mut dyn FnMut(),
    ) -> Result<usize> {
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
            let before = rewritten;
            for (seq, z) in &smaller {
                rewritten += tx.execute(
                    "UPDATE records SET body = ?1, enc = 'zstd'
                     WHERE device = ?2 AND seq = ?3 AND enc = 'plain'",
                    params![z, device, seq],
                )?;
            }
            tx.commit()?;
            if rewritten != before {
                committed();
            }
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
    forget: Vec<crate::forget::Request>,
    home_id: Option<String>,
    home_id_proven: Option<bool>,
}

impl Rebuild {
    /// The forget requests the live raw.db and the logs hold, which `finish` applies to the
    /// rebuilt store before it is renamed in (D1 rule 14): a backup directory older than a forget
    /// cannot bring the text back.
    pub fn forget(&mut self, requests: Vec<crate::forget::Request>) {
        self.forget = requests;
    }

    pub(crate) fn home_id(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key='home_id'", [], |r| {
                r.get(0)
            })?)
    }

    pub fn new(path: &Path, device: &str) -> Result<Self> {
        anyhow::ensure!(!path.exists(), "{} exists", path.display());
        let conn = Connection::open(path)?;
        conn.execute_batch(&schema_for_file(&conn, path)?)?;
        // A staging file is fresh, but the history rebuilt into it is not proof of a fresh home.
        conn.execute("DELETE FROM meta WHERE key='home_id_proven'", [])?;
        crate::db::ensure_device(&conn, path)?;
        conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'device_id'",
            [device],
        )?;
        // Backups made before the lineage field use the partition's device as before.
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('home_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [device],
        )?;
        conn.execute_batch("BEGIN")?;
        Ok(Self {
            conn,
            forget: Vec::new(),
            home_id: None,
            home_id_proven: None,
        })
    }

    /// One backup line. Bodies are stored as zstd where that is smaller, as the compress
    /// consumer would have; a tombstone's time is 0, since `Raw::after` returns none.
    /// Its source is `forget` only for a validated carried denial; otherwise `restore`.
    pub fn add(&mut self, line: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
        let i = |k: &str| v.get(k).and_then(serde_json::Value::as_i64);
        let (device, seq) = (s("device").context("device")?, i("seq").context("seq")?);
        if let Some(value) = v.get("home_id") {
            let id = value.as_str().context("backup home identity")?;
            anyhow::ensure!(
                !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
                "invalid backup home identity"
            );
            if let Some(known) = &self.home_id {
                anyhow::ensure!(known == id, "backup home identities differ");
            } else {
                self.conn
                    .execute("UPDATE meta SET value=?1 WHERE key='home_id'", [id])?;
                self.home_id = Some(id.into());
            }
        }
        let proven = match v.get("home_id_proven") {
            Some(value) => value.as_bool().context("backup home identity proof")?,
            None => false,
        };
        anyhow::ensure!(
            !proven || s("home_id").is_some(),
            "a proven backup home needs its identity"
        );
        if let Some(known) = self.home_id_proven {
            anyhow::ensure!(known == proven, "backup home identity proofs differ");
        } else {
            if proven {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('home_id_proven', '1')",
                    [],
                )?;
            }
            self.home_id_proven = Some(proven);
        }
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
            }
            Some("removed") => {
                // Restore only a complete explicitly carried triple. A legacy or malformed
                // line remains a bodyless, ineligible stub rather than inventing its cutoff.
                let metadata = match (s("source"), s("kind"), i("ts")) {
                    (Some(source), Some(kind), Some(ts))
                        if !source.is_empty() && !kind.is_empty() =>
                    {
                        (source, Some(kind), ts)
                    }
                    _ => ("restore", None, 0),
                };
                self.conn.execute(
                    "INSERT INTO records(device, seq, type, ts, source, kind)
                     VALUES(?1, ?2, 'removed', ?3, ?4, ?5)",
                    params![device, seq, metadata.2, metadata.0, metadata.1],
                )?;
            }
            Some("tombstone") => {
                let t = &v["target"];
                let target_device = t["device"].as_str().context("target device")?;
                let target_seq = t["seq"].as_i64().context("target seq")?;
                let mut source = "restore";
                let mut deny_origin = None;
                // A forget's deny row (D1): segments alone forget it again.
                if let Some(d) = v.get("deny") {
                    let text = |k: &str| d[k].as_str().with_context(|| format!("deny {k}"));
                    let origin = text("origin")?;
                    crate::forget::check_identity(origin)?;
                    crate::forget::check_identity(text("session")?)?;
                    self.conn.execute(
                        "INSERT OR IGNORE INTO denied_records(origin, device, seq, ts, session, job)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            origin,
                            text("device")?,
                            d["seq"].as_i64().context("deny seq")?,
                            d["ts"].as_i64(),
                            text("session")?,
                            text("job")?
                        ],
                    )?;
                    // A validated carried denial makes this the durable control. Keep that
                    // marker so replaying its request does not append another after restore.
                    source = "forget";
                    deny_origin = Some(origin);
                }
                self.conn.execute(
                    "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                       target_offset, target_length, deny_origin)
                     VALUES(?1, ?2, 'tombstone', 0, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        device,
                        seq,
                        source,
                        target_device,
                        target_seq,
                        t["offset"].as_i64(),
                        t["length"].as_i64(),
                        deny_origin
                    ],
                )?;
            }
            other => anyhow::bail!("seq {seq}: unknown record type {other:?}"),
        }
        // A removed record keeps only its validated native hashes: subsequent backup
        // generations still associate the tombstone's denial without restoring any body.
        if matches!(s("type"), Some("event" | "removed"))
            && let Some(identity) = v.get("import_identity")
        {
            let origin = identity["origin"].as_str().context("import origin")?;
            crate::forget::check_identity(origin)?;
            let session = identity["session"].as_str();
            if let Some(session) = session {
                crate::forget::check_identity(session)?;
            }
            self.conn.execute(
                "INSERT INTO import_origins(device, seq, origin, native_session, ambiguous) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![device, seq, origin, session, i64::from(identity["ambiguous"].as_bool().unwrap_or(true))],
            )?;
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
    /// batch lost a record goes too, even when later segments were restored. A forget op among
    /// them is appended again (D5). Returns how many ops went, those not counted.
    // ponytail: a restore keeping a batch's records but not its migration op imports them again;
    // a check of the restored records against the source's ids would catch it.
    pub fn finish(self) -> Result<usize> {
        let past = "op_seq >= (
               SELECT MIN(w.op_seq) FROM ops w WHERE w.device = ops.device
                 AND w.type IN ('window', 'migration')
                 AND (json_extract(w.body, '$.to_seq') >
                        (SELECT COALESCE(MAX(r.seq), 0) FROM records r WHERE r.device = w.device)
                      OR w.type = 'migration' AND
                        (SELECT COUNT(*) FROM records r WHERE r.device = w.device
                           AND r.seq BETWEEN json_extract(w.body, '$.from_seq')
                                         AND json_extract(w.body, '$.to_seq'))
                        < json_extract(w.body, '$.to_seq') - json_extract(w.body, '$.from_seq') + 1))";
        // D5: a forget op among them derives nothing from the lost records. It is appended again
        // below, at a reused op seq as any new op is, so the uid stays forgotten when no request
        // log brings it back (Codex's adversarial review of slice 2a); as this device's, the only
        // one whose ops a backup holds until sync (milestone 6).
        let forgets: Vec<String> = self
            .conn
            .prepare(&format!(
                "SELECT body FROM ops WHERE type = 'forget' AND {past}"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let dropped = self
            .conn
            .execute(&format!("DELETE FROM ops WHERE {past}"), [])?;
        // After that check: a forget's tombstones take new seqs, which must not make an old window
        // look as if the records a damaged backup lost were there.
        let device: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key = 'device_id'", [], |r| {
                    r.get(0)
                })?;
        for body in &forgets {
            let uid: String = serde_json::from_str::<serde_json::Value>(body)?["uid"]
                .as_str()
                .context("a forget op without its uid")?
                .to_owned();
            if !forgotten_uid(&self.conn, &uid)? {
                insert_ops(
                    &self.conn,
                    &device,
                    &[(OpKind::Forget.name(), body.clone())],
                )?;
            }
        }
        for r in &self.forget {
            r.check()?;
            apply_request(&self.conn, &device, r)?;
        }
        self.conn.execute_batch("COMMIT")?;
        self.conn.close().map_err(|(_, e)| e)?;
        // No forget op among them is lost: each is appended again, or its uid is forgotten already.
        Ok(dropped - forgets.len())
    }
}

/// Applies one forget request by identity (D1 rule 5), inside the caller's write transaction:
/// each record's deny row, keyed by its origin, and a tombstone of every record of this store
/// whose import origin it is, at whatever seq it has now (a re-import's copy too) and that none
/// of this device's forget controls covers yet; then the job row. A rescan or another device's
/// control cannot carry this device's incremental backup denial. A reused seq is never hidden for an old
/// request. Whether the store lacked the job.
pub(crate) fn apply_request(
    conn: &Connection,
    device: &str,
    r: &crate::forget::Request,
) -> Result<bool> {
    // D5: one forget op per uid, whichever request or log line brings it first.
    if let crate::forget::Target::Uid { uid } = &r.target
        && !forgotten_uid(conn, uid)?
    {
        let body = serde_json::json!({"uid": uid, "job": r.job}).to_string();
        insert_ops(conn, device, &[(OpKind::Forget.name(), body)])?;
    }
    for rec in &r.records {
        conn.execute(
            "INSERT OR IGNORE INTO denied_records(origin, device, seq, ts, session, job)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![rec.origin, rec.device, rec.seq, rec.ts, rec.session, r.job],
        )?;
        let targets: Vec<(String, i64)> = conn
            .prepare(
                "SELECT o.device, o.seq FROM import_origins o
                 WHERE o.origin = ?1 AND NOT EXISTS (
                   SELECT 1 FROM records t WHERE t.device = ?2 AND t.type = 'tombstone'
                     AND t.source = 'forget'
                     AND (t.deny_origin IS NULL OR t.deny_origin = o.origin)
                     AND t.target_device = o.device AND t.target_seq = o.seq
                     AND t.target_offset IS NULL)
                 ORDER BY o.device, o.seq",
            )?
            .query_map(params![rec.origin, device], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (target_device, target_seq) in targets {
            let seq: i64 = conn.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE device = ?1",
                [device],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq, deny_origin)
                 VALUES(?1, ?2, 'tombstone', ?3, 'forget', ?4, ?5, ?6)",
                params![device, seq, r.started, target_device, target_seq, rec.origin],
            )?;
        }
    }
    Ok(conn.execute(
        "INSERT OR IGNORE INTO forget_jobs(id, request, started, step) VALUES(?1, ?2, ?3, 1)",
        params![r.job, serde_json::to_string(r)?, r.started],
    )? == 1)
}

/// The ops of `uid` that still hold its text (D5): a claim's Claim ops, by its uid as the consumer
/// computes it, the Correction ops that name it, and a document's Import ops; any device's.
fn uid_op_keys(conn: &Connection, uid: &str) -> Result<Vec<(String, i64)>> {
    let mut keys = Vec::new();
    if uid.len() == 64 && uid.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut st = conn.prepare("SELECT device, op_seq, body FROM ops WHERE type = 'claim'")?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(2)?)
            else {
                continue;
            };
            if crate::claims::op_uid(&body).as_deref() == Some(uid) {
                keys.push((row.get(0)?, row.get(1)?));
            }
        }
    }
    let mut st = conn.prepare(
        "SELECT device, op_seq FROM ops WHERE type IN ('correction', 'import')
           AND json_extract(body, '$.uid') = ?1",
    )?;
    for row in st.query_map([uid], |r| Ok((r.get(0)?, r.get(1)?)))? {
        keys.push(row?);
    }
    Ok(keys)
}

/// A Claim or Correction op anchored on a record forget denies, by the record's origin (D1 rule
/// 5), or a Claim, Correction or Import op of a forgotten uid (D5): never recorded. Window and
/// turn ops are fenced by `Reading` (rule 12).
fn forgotten_op(conn: &Connection, kind: OpKind, body: &serde_json::Value) -> Result<bool> {
    let uid = match kind {
        OpKind::Claim => crate::claims::op_uid(body),
        OpKind::Correction | OpKind::Import => body["uid"].as_str().map(str::to_owned),
        _ => None,
    };
    if let Some(uid) = uid
        && forgotten_uid(conn, &uid)?
    {
        return Ok(true);
    }
    let anchor = |e: &serde_json::Value| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM import_origins o JOIN denied_records d
               ON d.origin = o.origin WHERE o.device = ?1 AND o.seq = ?2)",
            params![e["device"].as_str(), e["seq"].as_i64()],
            |r| r.get(0),
        )?)
    };
    match kind {
        OpKind::Claim => {
            for e in body["evidence"].as_array().into_iter().flatten() {
                if anchor(e)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        OpKind::Correction => anchor(&body["anchor"]),
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
fn begin_batch(conn: &Connection, wait: Duration) -> rusqlite::Result<rusqlite::Transaction<'_>> {
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
fn insert_ops(tx: &Connection, device: &str, bodies: &[(&str, String)]) -> Result<Vec<i64>> {
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

    #[test]
    fn at_reads_the_records_named_in_seq_order_as_after_returns_them() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for body in ["one", "two secret", "three"] {
            raw.append(&test_event(body)).unwrap();
        }
        let device = raw.device().to_owned();
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: 2,
            offset: 4,
            length: 6,
        })
        .unwrap();
        raw.append_tombstone(Target::Record {
            device: device.clone(),
            seq: 3,
        })
        .unwrap();
        let recs = raw.at(&device, &[3, 9, 2, 1]).unwrap();
        assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(matches!(&recs[0].item, Item::Event(e) if e.body == "one"));
        assert!(matches!(&recs[1].item, Item::Event(e) if e.body == "two ******"));
        assert!(matches!(recs[2].item, Item::Removed));
        assert!(raw.at(&device, &[]).unwrap().is_empty());
    }

    #[test]
    fn dispatch_wait_failure_records_no_exclusion() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let held = raw.dispatch().unwrap();

        assert!(
            raw.exclude("x", false).is_err(),
            "a transmitting batch permitted an exclusion acknowledgement"
        );
        assert_eq!(raw.max_op_seq().unwrap(), 0);
        assert!(raw.exclusions().unwrap().is_empty());
        assert!(!home.path().join("providers.db").exists());

        held.release();
        assert_eq!(raw.exclude("x", false).unwrap(), 1);
        assert_eq!(raw.exclusions().unwrap(), vec!["x"]);
        assert_eq!(raw.exclude("x", true).unwrap(), 2);
        assert!(raw.exclusions().unwrap().is_empty());
    }

    #[test]
    fn dispatch_wait_failure_is_bounded_and_registers_no_partial_forget() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut event = test_event(r#"{"prompt":"native-timeout-canary-903"}"#);
        event.source = "transcript".into();
        let identity = ImportIdentity {
            origin: crate::forget::origin("synthetic-dispatch", "timeout-903"),
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
        let preview = crate::forget::preview(
            home.path(),
            crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            },
        )
        .unwrap();
        let held = raw.dispatch().unwrap();
        let began = Instant::now();
        let rejected = crate::forget::start(home.path(), &preview);
        let elapsed = began.elapsed();
        assert!(
            rejected.is_err(),
            "a transmitting batch permitted registration"
        );
        assert!(elapsed >= crate::db::OPEN_WRITE_WAIT && elapsed < Duration::from_secs(15));
        assert_eq!(raw.denied_count().unwrap(), 0);
        assert!(crate::forget::status(home.path()).unwrap().is_empty());
        assert!(!home.path().join("forget.log").exists());
        assert!(!home.path().join("backups/forget.log").exists());
        held.release();
        assert_eq!(
            crate::forget::start(home.path(), &preview)
                .unwrap()
                .0
                .records,
            1
        );
    }

    /// A rescan's whole tombstone hides the sample, but forget must still deny a verified
    /// transcript import in a record or span target and retain its original metadata.
    #[test]
    fn a_tombstoned_import_can_be_forgotten_without_a_sample_and_cannot_be_reimported() {
        for span in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            let mut event = test_event(r#"{"prompt":"tombstoned-native-canary-691"}"#);
            event.source = "transcript".into();
            event.ts = 1_000;
            let captured = || crate::capture::Captured {
                event: event.clone(),
                ledger: Vec::new(),
            };
            let identity = ImportIdentity {
                origin: crate::forget::origin("synthetic", "tombstoned-native"),
                session: crate::forget::session(&event.agent, &event.session),
                ambiguous: None,
                unverified: false,
            };
            let seq = raw
                .append_imported_origins(&[captured()], std::slice::from_ref(&identity), "", None)
                .unwrap()[0];
            let device = raw.device().to_owned();
            let tombstone = raw
                .append_tombstone(Target::Record {
                    device: device.clone(),
                    seq,
                })
                .unwrap();
            let target = if span {
                crate::forget::Target::Span {
                    device: device.clone(),
                    from: seq,
                    to: tombstone,
                }
            } else {
                crate::forget::Target::Record {
                    device: device.clone(),
                    seq,
                }
            };
            let preview = raw.forget_preview(target).unwrap();
            assert_eq!(preview.count(), 1, "the tombstone hid the import identity");
            assert!(preview.sample.is_none(), "a removed sample was shown");
            assert_eq!(preview.records[0].ts, None);
            assert_eq!(preview.sources, ["transcript"]);
            crate::forget::start(home.path(), &preview).unwrap();
            assert!(
                raw.append_imported_origins(
                    &[captured()],
                    std::slice::from_ref(&identity),
                    "",
                    None
                )
                .unwrap()
                .is_empty(),
                "the already tombstoned import was not denied"
            );
            assert_eq!(raw.transcript_cut("claude", "s", None).unwrap(), None);
        }
    }

    /// F2 also covers the consistent snapshot after the first schema transaction, before
    /// the opener returns: it already carries the lineage later forgets will name.
    /// Milestone 5 D5: a forgotten uid's Claim (derived again, whatever its body), Correction and
    /// Import ops are refused, an imported document of it is passed over, and a request applied
    /// again adds no second forget op.
    #[test]
    fn a_forgotten_uids_ops_are_refused_and_its_document_passed_over() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let seq = raw.append(&test_event("We use tabs.")).unwrap();
        let device = raw.device().to_owned();
        let claim = |body: &str| {
            serde_json::to_value(crate::claims::ClaimOp {
                id: "c1".into(),
                kind: "decision".into(),
                status: "decided".into(),
                speaker: "user".into(),
                scope: "repo".into(),
                body: body.into(),
                evidence: vec![crate::claims::Evidence {
                    device: device.clone(),
                    seq,
                    offset: 0,
                    length: 6,
                    sentence: 0,
                    quote: "We use".into(),
                    claim_at: None,
                }],
                supersedes: Vec::new(),
                recipe: "test".into(),
                tier: 1,
                why: String::new(),
                tainted: false,
            })
            .unwrap()
        };
        let doc = |id: &str| ImportDoc {
            uid: format!("claude-mem:test:{id}"),
            source: "claude-mem:test".into(),
            source_id: id.into(),
            kind: "decision".into(),
            repo: "claude-mem:r".into(),
            session: "s".into(),
            ts: 1,
            title: "A title".into(),
            body: "A body.".into(),
        };
        let uid = crate::claims::op_uid(&claim("Use tabs.")).unwrap();
        let forget = |raw: &mut Raw, uid: &str, job: char| {
            raw.forget_apply(&[crate::forget::Request {
                v: 2,
                home: raw.home_id().into(),
                job: job.to_string().repeat(32),
                started: 1,
                target: crate::forget::Target::Uid { uid: uid.into() },
                records: Vec::new(),
            }])
            .unwrap()
        };
        assert_eq!(forget(&mut raw, &uid, 'a'), 1);
        assert_eq!(forget(&mut raw, &doc("o1").uid, 'b'), 1);
        // Another job of a uid forgotten already writes its job row and no second op.
        assert_eq!(forget(&mut raw, &uid, 'c'), 1);
        let forgets = |raw: &Raw| {
            raw.ops_after(&device, 0, 100)
                .unwrap()
                .iter()
                .filter(|o| o.kind == OpKind::Forget)
                .count()
        };
        assert_eq!(forgets(&raw), 2);
        let correction = serde_json::json!({"uid": uid, "status": "done",
            "anchor": {"device": device, "seq": seq}});
        for (kind, body) in [
            (OpKind::Claim, claim("Indent with tabs everywhere.")),
            (OpKind::Correction, correction),
            (OpKind::Import, serde_json::to_value(doc("o1")).unwrap()),
        ] {
            assert!(raw.append_ops(&[(kind, body)]).is_err(), "{kind:?}");
        }
        assert_eq!(raw.append_imports(vec![doc("o1"), doc("o2")]).unwrap(), 1);
        assert!(raw.forgotten(&uid).unwrap() && raw.forgotten(&doc("o1").uid).unwrap());
        assert!(!raw.forgotten(&doc("o2").uid).unwrap());
        assert_eq!(raw.forget_requests().unwrap().len(), 3);
    }

    #[test]
    fn a_snapshot_of_first_schema_commit_keeps_forget_lineage() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("raw.db");
        let conn = Connection::open(&path).unwrap();
        crate::db::ensure_schema_until(
            &conn,
            &schema_for_file(&conn, &path).unwrap(),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        drop(conn);
        let old = home.path().join("first-schema.db");
        std::fs::copy(&path, &old).unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut event = test_event(r#"{"prompt":"first-schema-forget-canary-671"}"#);
        event.source = "transcript".into();
        let captured = || crate::capture::Captured {
            event: event.clone(),
            ledger: Vec::new(),
        };
        let identity = ImportIdentity {
            origin: crate::forget::origin("synthetic", "first-schema"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(&[captured()], std::slice::from_ref(&identity), "", None)
            .unwrap()[0];
        let preview = raw
            .forget_preview(crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            })
            .unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        drop(raw);
        std::fs::copy(old, &path).unwrap();
        let mut restored = open(home.path()).unwrap();
        crate::forget::reconcile(home.path(), &mut restored).unwrap();
        assert!(
            restored
                .append_imported_origins(&[captured()], &[identity], "", None)
                .unwrap()
                .is_empty(),
            "the first-schema snapshot changed lineage and reimported a forgotten event"
        );
    }

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

    /// docs/work-state.md L4: work state folds in its clock's order: a write on a copied store
    /// (another device) comes after every write it holds, even when the old device's clock ran
    /// ahead of the new one's.
    #[test]
    fn work_state_writes_fold_in_clock_order_across_devices() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let phase = |v: &str| {
            serde_json::json!({ "phase": v })
                .as_object()
                .unwrap()
                .clone()
        };
        raw.work_state("r", "release", &phase("one")).unwrap();
        raw.work_state("r", "release", &phase("two")).unwrap();
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
        other.work_state("r", "release", &phase("three")).unwrap();
        other
            .work_state("s", "release", &phase("elsewhere"))
            .unwrap();
        let phases: Vec<_> = other
            .work_state_entries("r")
            .unwrap()
            .into_iter()
            .map(|e| e.fields["phase"].clone())
            .collect();
        assert_eq!(phases, ["one", "two", "three"]);
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

    #[test]
    fn counted_documents_report_only_committed_batches_on_interruption() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let docs = (0..100)
            .map(|i| import_doc(i, "x".repeat(60_000)))
            .collect();
        crate::crash::at(2);
        let mut committed = Vec::new();
        let result = raw.append_imports_counted(docs, |n| committed.push(n));
        crate::crash::off();
        assert!(result.is_err());
        let durable = raw.ops_after(raw.device(), 0, 1_000).unwrap().len();
        assert!(durable > 0 && durable < 100);
        assert_eq!(committed, [durable as u64]);
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
