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
-- G22: the window ops a skipped span is made of (skipped, refused, a recuration), which a session's
-- start reads: through this index, those alone and not every window's body. `instr`, never a JSON
-- function: a malformed body cannot fail the open that builds it. serde_json writes these keys
-- without spaces. `SKIP_WINDOWS` repeats the condition, for the planner to take the index.
CREATE INDEX IF NOT EXISTS ops_window_skips ON ops(device, op_seq)
  WHERE type = 'window' AND (instr(body, '\"outcome\":\"skipped\"') > 0
    OR instr(body, '\"refused\":true') > 0 OR instr(body, '\"recurate\":true') > 0);
-- docs/work-state.md L4: the agents' work state, read at each session start from its own ops.
CREATE INDEX IF NOT EXISTS ops_work_state ON ops(device, op_seq) WHERE type = 'work_state';
-- Milestone 5 slice 3d: the claim, correction and turn ops a purge reads for what was made from a
-- forgotten record, at each look for its jobs, without every import's body.
CREATE INDEX IF NOT EXISTS ops_claims ON ops(device, op_seq) WHERE type = 'claim';
CREATE INDEX IF NOT EXISTS ops_corrections ON ops(device, op_seq) WHERE type = 'correction';
CREATE INDEX IF NOT EXISTS ops_turns ON ops(device, op_seq) WHERE type = 'turn';
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
-- A record's identity, hashed: an import's native one as its importer kept it (forget::origin), and
-- a forgotten live record's from its device, seq and time (forget::live_origin, M5 slice 3a). No
-- original id or text.
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
-- docs/unwritten.md U3 (#454): each kept file written back, by its content's hash, in the
-- transaction that wrote its events: a file a call left behind after that commit is removed, never
-- written again. Not a name: they start again from 0 once the directory is empty.
CREATE TABLE IF NOT EXISTS unwritten_written(hash TEXT PRIMARY KEY, at INTEGER NOT NULL);
-- Milestone 5 slice 3b (D3): each page of a scope's forget, by its kind (`session`, `repo`,
-- `range`), its key (a session's or a repository's hash), the device a repository's or a time
-- range's forget is bound to, a time range's times, and when it was registered: a record of that
-- scope from before then, arriving later, is not recorded. A row a page, so read whole: no index
-- on the key (spec 1.6, a label is no index root).
CREATE TABLE IF NOT EXISTS denied_scopes(
  job TEXT PRIMARY KEY, kind TEXT NOT NULL, key TEXT NOT NULL, before INTEGER NOT NULL,
  device TEXT, since INTEGER, until INTEGER
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub(crate) fn from_name(name: &str) -> Option<Self> {
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
        forgotten_body(&self.body)
    }
}

/// Whether `body` is the one a forget writes, `{"forgotten": "<job>"}` and nothing else (D5).
pub(crate) fn forgotten_body(body: &serde_json::Value) -> bool {
    body.as_object().is_some_and(|o| {
        o.len() == 1 && o.get("forgotten").is_some_and(serde_json::Value::is_string)
    })
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

/// Every forgotten uid (D5): the readers and senders pass them over.
fn forgotten_set(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut st = conn.prepare_cached(
        "SELECT json_extract(body, '$.uid') FROM ops
         WHERE type = 'forget' AND json_extract(body, '$.uid') IS NOT NULL",
    )?;
    let uids = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(uids)
}

/// Every uid a record job's forget op tombstones, with that job (`Raw::record_derived_uids`).
fn record_derived_uids(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    Ok(conn
        .prepare(
            "SELECT json_extract(body, '$.uid'), json_extract(body, '$.job') FROM ops
             WHERE type = 'forget'
               AND json_extract(body, '$.job') IN (SELECT job FROM denied_records)",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Each forgotten record's vector key (`<device>:<seq>`, kind 'r'): a carry takes no vector of
/// one (slice 3c), nor of a record a rescan masked, whose vector may be of its text before the mask
/// (slice 3e-1). No forgotten record in a store a writer of forget never opened.
fn forgotten_record_keys(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let tables: bool = conn.query_row(
        "SELECT COUNT(*) = 2 FROM sqlite_master
         WHERE type = 'table' AND name IN ('denied_records', 'import_origins')",
        [],
        |r| r.get(0),
    )?;
    let masked = "SELECT target_device || ':' || target_seq FROM records
                  WHERE type = 'tombstone' AND target_offset IS NOT NULL";
    let sql = if tables {
        format!(
            "SELECT o.device || ':' || o.seq FROM denied_records d
             JOIN import_origins o ON o.origin = d.origin UNION {masked}"
        )
    } else {
        masked.to_owned()
    };
    Ok(conn
        .prepare(&sql)?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Milestone 5 D5: whether a forget op names `uid`.
fn forgotten_uid(conn: &Connection, uid: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ops WHERE type = 'forget' AND json_extract(body, '$.uid') = ?1)",
        [uid],
        |r| r.get(0),
    )?)
}

/// What forget denies: records by origin, and uids by their forget ops. A preview's token count,
/// and with the masks the derived writers' fence (`FENCE_COUNT`).
const DENIED_COUNT: &str = "SELECT (SELECT COUNT(*) FROM denied_records)
  + (SELECT COUNT(*) FROM ops WHERE type = 'forget')";

/// The derived writers' fence (rule 12): what forget denies, so a uid's forget also cuts a window in
/// flight again (D5: rare, and safe), and the ranges a rescan masked (slice 3e-1): what a curator
/// or a turn's call read before one is cut again, never written.
const FENCE_COUNT: &str = "SELECT (SELECT COUNT(*) FROM denied_records)
  + (SELECT COUNT(*) FROM ops WHERE type = 'forget')
  + (SELECT COUNT(*) FROM records WHERE type = 'tombstone' AND target_offset IS NOT NULL)";

/// Slice 3e-1: how many ranges a rescan masked, and how many of them the last purge that did every
/// step paid (`Raw::masks_owed`).
const MASKS: &str = "SELECT
    (SELECT COUNT(*) FROM records WHERE type = 'tombstone' AND target_offset IS NOT NULL),
    (SELECT value FROM meta WHERE key = 'rescan_masks_paid')";

/// Opposite import sources do not share event identifiers. Keep exact-origin denials, but
/// refuse an import that could reintroduce their copy from the other source.
fn check_cross_source_forget(conn: &Connection, session: &str, source: &str) -> Result<()> {
    // Slice 3b: a denial a forget of the whole session covers (its own page's, or one from before
    // its registration) refuses no batch: the session's tombstone refuses each record from before
    // the registration and takes those from after it (CodeRabbit and Greptile on #465).
    let unverified: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM denied_records d
         WHERE d.session=?1 AND (?2 <> 'transcript' OR d.ts IS NOT NULL)
           AND NOT EXISTS(SELECT 1 FROM denied_scopes s
             WHERE s.kind = 'session' AND s.key = d.session
               AND (s.job = d.job OR d.ts <= s.before)))",
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

/// Which source wrote a record, for the copies a forget takes with it (slice 3a): no record-level
/// identity is shared between them, so a record's copies may be in its session's records of another.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Family {
    Live,
    Transcript,
    V1,
}

impl Family {
    fn of(source: &str) -> Option<Self> {
        match source {
            "transcript" => Some(Self::Transcript),
            "oboete-v1" => Some(Self::V1),
            s if is_live(s) => Some(Self::Live),
            _ => None,
        }
    }
}

/// Slice 3b: a scope's records as forget names them, each with its time and its kind.
type Found = Vec<(Named, i64, String)>;

/// A record as forget names it (D1 rule 5, slice 3a): its identity, its session's hash when its
/// labels prove it, its source, whether an import's identity is ambiguous, its time when the
/// transcript cut counts it (rule 10), and its first characters for the preview's sample.
struct Named {
    device: String,
    seq: i64,
    origin: String,
    session: Option<String>,
    family: Family,
    agent: Option<String>,
    ambiguous: bool,
    ts: Option<i64>,
    source: String,
    sample: Option<String>,
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
    /// Owner decision 43: the text of the ops this handle appends was gated with the redaction
    /// rules turned off (`gates_with`), so each append owes the op text scan in its transaction.
    gated_off: bool,
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

    /// Every forgotten uid (D5).
    pub(crate) fn forgotten_set(&self) -> Result<std::collections::HashSet<String>> {
        forgotten_set(&self.conn)
    }

    /// Each forgotten record's vector key (slice 3c).
    pub(crate) fn forgotten_record_keys(&self) -> Result<std::collections::HashSet<String>> {
        forgotten_record_keys(&self.conn)
    }

    /// The store's lineage, which each forget request names; none before a writer seeds it.
    pub(crate) fn home_id(&self) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key='home_id'", [], |r| {
                r.get(0)
            })
            .optional()?)
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
    format_of(&conn)?;
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

/// The window ops a skipped span is made of, through `ops_window_skips` (its condition, word for
/// word), then each body read as JSON.
const SKIP_WINDOWS: &str = "SELECT op_seq, ts, body, batch FROM ops
     WHERE device = ?1 AND op_seq > ?2
       AND type = 'window' AND (instr(body, '\"outcome\":\"skipped\"') > 0
         OR instr(body, '\"refused\":true') > 0 OR instr(body, '\"recurate\":true') > 0)
       AND (json_extract(body, '$.outcome') = 'skipped' OR json_extract(body, '$.refused') = 1
            OR json_extract(body, '$.recurate') = 1)
     ORDER BY op_seq LIMIT ?3";

/// `device`'s window ops after `op_seq` that a skipped span is made of (`curate::spans_skipped`):
/// one every provider skipped or refused, and a recuration. A session's start reads them (G22): on
/// a copy of a store with 15,492 windows (66 of them these), reading every window's body made a
/// session's start take 70 ms against 33 ms before G22 (debug build); through the index, 37 ms.
pub(crate) fn skip_window_ops_after_in(
    conn: &Connection,
    device: &str,
    op_seq: i64,
    limit: usize,
) -> Result<Vec<Op>> {
    let mut st = conn.prepare_cached(SKIP_WINDOWS)?;
    let rows = st.query_map(
        params![device, op_seq, i64::try_from(limit).unwrap_or(i64::MAX)],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get(1)?,
                r.get::<_, String>(2)?,
                r.get(3)?,
            ))
        },
    )?;
    rows.map(|row| {
        let (op_seq, ts, body, batch) = row?;
        Ok(Op {
            device: device.to_owned(),
            op_seq,
            kind: OpKind::Window,
            ts,
            body: serde_json::from_str(&body).with_context(|| format!("op {op_seq}: body"))?,
            batch,
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

/// The same current list for an existing read-only connection: no indexes or schema writes. What
/// is never recorded is never sent either (slice 4a, spec 6.1).
pub(crate) fn exclusions_in(conn: &Connection) -> Result<Vec<String>> {
    let (mut sent, never) = exclusion_sets(conn)?;
    sent.extend(never.repos);
    Ok(sent.into_iter().collect())
}

/// Slice 4a (spec 6.1): what is never recorded now, on an existing connection.
pub(crate) fn capture_exclusions_in(conn: &Connection) -> Result<CaptureExclusions> {
    Ok(exclusion_sets(conn)?.1)
}

/// Slice 4a (spec 6.1): what is never recorded, as every device's exclusion ops of level `capture`
/// leave it: repositories by their label (`repo`), this device's folders by their `cwd` label.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CaptureExclusions {
    pub repos: std::collections::BTreeSet<String>,
    pub folders: std::collections::BTreeSet<String>,
}

impl CaptureExclusions {
    /// Whether an event labelled `repo` and `cwd` is never recorded: its repository listed, or its
    /// working directory in a listed folder, component by component.
    pub fn covers(&self, repo: Option<&str>, cwd: Option<&str>) -> bool {
        repo.is_some_and(|r| self.repos.contains(r))
            || cwd.is_some_and(|c| self.folder_around(c).is_some())
    }

    /// The listed folder that `cwd` lies in, component by component, `cwd` itself included.
    pub fn folder_around(&self, cwd: &str) -> Option<&String> {
        let c = folded(cwd);
        self.folders
            .iter()
            .find(|f| std::path::Path::new(&*c).starts_with(&*folded(f)))
    }

    /// Slice 4a (owner, 2026-10-13): whether an event is never recorded, as every writer asks it:
    /// its repository and folder as the writer read them before the gate (`read`, where it holds
    /// them) against the list as given, and its labels against the list as given and as the rules
    /// now read it (`now`, `capture::never_recorded`).
    pub fn refuses(
        &self,
        now: Option<&CaptureExclusions>,
        labels: (Option<&str>, Option<&str>),
        read: Option<(&str, Option<&str>)>,
    ) -> bool {
        read.is_some_and(|(repo, cwd)| self.covers(Some(repo), cwd))
            || self.covers(labels.0, labels.1)
            || now.is_some_and(|now| now.covers(labels.0, labels.1))
    }

    /// Whether a work state op of `repo` is never recorded: its repository listed, or, for a list
    /// imported from claude-mem (`imported`), the repository of its project's name (milestone 4
    /// D13, docs/claude-mem-import.md I6). A list written here is matched by its own key alone, as
    /// `work_state_in` reads it, whatever shape that key has (Codex on #489).
    pub fn covers_work_state(&self, repo: &str, imported: bool) -> bool {
        if !imported {
            return self.repos.contains(repo);
        }
        let repos: Vec<String> = self.repos.iter().cloned().collect();
        crate::embed_phase::import_excluded(repo, &repos)
    }
}

/// Slice 4a: whether `label`, which a mask hides part of, may be one of `names`: each mask read as
/// any text. A label masked under a rule removed since cannot be read again, so a repository never
/// recorded holds back from sending every label it may be (Codex's security review of #489).
fn may_name<'a>(label: &str, mut names: impl Iterator<Item = &'a String>) -> bool {
    if !label.contains(crate::redact::MASK) {
        return false;
    }
    let parts: Vec<String> = label
        .split(crate::redact::MASK)
        .map(regex::escape)
        .collect();
    let Ok(masked) = regex::Regex::new(&format!("^{}$", parts.join(".+"))) else {
        return true;
    };
    names.any(|n| masked.is_match(n))
}

/// Slice 4a: a folder as the platform's file systems name one: Windows' and macOS's ignore letter
/// case unless a volume was made otherwise, so either spelling of an excluded folder is covered
/// (Codex on #489).
pub(crate) fn folded(path: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(any(windows, target_os = "macos")) {
        path.to_lowercase().into()
    } else {
        path.into()
    }
}

/// Slice 4a: a work state write refused inside its transaction: its repository, or the folder it
/// was written from, excluded from capture by then (Codex on #489).
#[derive(Debug)]
pub struct NeverRecorded;

impl std::fmt::Display for NeverRecorded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "nothing is recorded in this repository or folder now: the work state was not kept",
        )
    }
}

impl std::error::Error for NeverRecorded {}

/// Every device's exclusion ops in time order, each entry set by the last op that names it: the
/// repositories `oboete exclude` keeps from being sent, and what is never recorded (level
/// `capture`, slice 4a). An undo takes its entry out of its own set only.
fn exclusion_sets(
    conn: &Connection,
) -> Result<(std::collections::BTreeSet<String>, CaptureExclusions)> {
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
    let (mut sent, mut never) = (
        std::collections::BTreeSet::new(),
        CaptureExclusions::default(),
    );
    for (_, _, _, v) in ops {
        let (set, name) = match (
            v["level"].as_str(),
            v["repo"].as_str(),
            v["folder"].as_str(),
        ) {
            (None, Some(repo), _) => (&mut sent, repo),
            (Some("capture"), Some(repo), _) => (&mut never.repos, repo),
            (Some("capture"), None, Some(folder)) => (&mut never.folders, folder),
            _ => continue,
        };
        if v["undo"] == true {
            set.remove(name);
        } else {
            set.insert(name.to_owned());
        }
    }
    Ok((sent, never))
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
    // docs/store-format.md: a raw.db a newer oboete wrote is refused before anything writes the
    // file, its journal mode included (CodeRabbit, Codex and Cursor on #503).
    let format = crate::db::retry_busy(&conn, deadline, || format_of(&conn))?;
    crate::db::wal_until(&conn, "FULL", deadline)?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON;")?;
    // Milestone 5 D5 (2b): what a write frees is overwritten with zeros, so neither a rewritten op
    // body nor a deleted row leaves its bytes in the file.
    conn.execute_batch("PRAGMA secure_delete=ON;")?;
    // Stamped before the schema and the migrations, read again under the write lock: of two
    // binaries opening together, the newer one's stamp stays, and the older one refuses it before
    // it migrates anything.
    if format < FORMAT
        && crate::db::retry_busy(&conn, deadline, || {
            let tx = rusqlite::Transaction::new_unchecked(
                &conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let raised = format_of(&tx)? < FORMAT;
            if raised {
                tx.execute_batch(&format!("PRAGMA user_version = {FORMAT}"))?;
            }
            tx.commit()?;
            Ok(raised)
        })?
    {
        committed("stores_changed")?;
    }
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
    // Slice 3b-2: a table slice 3b-1 made has no repository's or time range's columns.
    for (column, decl) in [
        ("device", "TEXT"),
        ("since", "INTEGER"),
        ("until", "INTEGER"),
    ] {
        if crate::db::ensure_column_until(&mut conn, "denied_scopes", column, decl, deadline)
            .context("migrate forget scopes")?
        {
            committed("stores_changed")?;
        }
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
    #[cfg(test)]
    if let Some(meanwhile) = BEFORE_OPEN_RETURNS.take() {
        meanwhile(&path);
    }
    // And once more before the handle is returned: a newer oboete may have stamped the file while
    // this one migrated it (CodeRabbit on #503).
    format_of(&conn)?;
    Ok(Raw {
        conn,
        home: home.to_owned(),
        file_identity,
        device,
        home_id,
        _swap: swap,
        gated_off: false,
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

/// The home's format (docs/store-format.md): raw.db's `user_version`, raised by a release that
/// writes anything an older binary would mishandle, in any store or a backup segment.
pub(crate) const FORMAT: i64 = 1;

/// `open`'s and `read_only`'s error for a raw.db a newer oboete wrote (`user_version` above
/// `FORMAT`): nothing reads or writes it with this binary's meaning of it.
#[derive(Debug)]
pub struct NewerFormat {
    pub found: i64,
}

impl std::fmt::Display for NewerFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "raw.db was written by a newer oboete (format {}; this one reads up to {FORMAT}): \
             update oboete",
            self.found
        )
    }
}

impl std::error::Error for NewerFormat {}

/// The format `conn`'s raw.db was last written with, refused when it is newer than this binary's.
fn format_of(conn: &Connection) -> Result<i64> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    anyhow::ensure!(found <= FORMAT, NewerFormat { found });
    Ok(found)
}

/// Restore-lock waits: opens wait at most 2 s within their initialization budget (hooks 2 s,
/// other callers 10 s); a restore waits at most 10 s for open stores to close.
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const SWAP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    /// What happens to raw.db after an open's migrations, before it checks the format once more.
    static BEFORE_OPEN_RETURNS: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
}

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

    /// What forget would register for `target` (D1): each record's identity (an import's origin,
    /// or a live record's computed from where and when it was recorded, slice 3a), session hash and
    /// cut time, and the records of the same sessions from another source, which may hold copies of
    /// them (no record-level identity is shared between sources). Read on this connection, so
    /// `forget_start` reads it again inside its transaction.
    pub(crate) fn forget_preview(
        &self,
        target: crate::forget::Target,
    ) -> Result<crate::forget::Preview> {
        match &target {
            crate::forget::Target::Session { agent, session } => {
                return self.session_preview(agent, session);
            }
            crate::forget::Target::Repo { device, .. }
            | crate::forget::Target::Range { device, .. } => {
                // The device the target was read with is this store's: a raw.db restored or put
                // back since would bind its tombstone to another device (Codex on #468).
                anyhow::ensure!(
                    *device == self.device,
                    "raw.db changed since this forget's target was read: run the forget again"
                );
                return self.scope_preview(target);
            }
            _ => {}
        }
        let (device, from, to) = target.bounds()?;
        let device = device.to_owned();
        let device = device.as_str();
        anyhow::ensure!(
            device == self.device,
            "this forget slice accepts only this device's raw records"
        );
        anyhow::ensure!(
            self.home_id_proven()?,
            "this store has no verified home identity: forget was not registered"
        );
        let (mut picked, mut passed) = (Vec::<Named>::new(), 0);
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
                match self.named(device, &r)? {
                    Some(n) if !denied(&self.conn, Some(&n.origin), "")? => picked.push(n),
                    Some(_) => {}
                    None => passed += 1,
                }
                anyhow::ensure!(
                    picked.len() <= crate::forget::MAX_RECORDS,
                    "split this selection into spans of at most {} records",
                    crate::forget::MAX_RECORDS
                );
            }
            at = last;
        }
        let copies = self.copies(&picked)?;
        // The cap `Preview::validate` holds a request to, said before the selection is shown
        // (Codex on #460).
        anyhow::ensure!(
            picked.len() + copies.len() <= crate::forget::MAX_RECORDS,
            "split this selection into spans of at most {} records, their copies counted",
            crate::forget::MAX_RECORDS
        );
        self.check_named(device, &picked, &copies)?;
        let mut sources = Vec::<String>::new();
        for n in picked.iter().chain(&copies) {
            if !sources.contains(&n.source) {
                sources.push(n.source.clone());
            }
        }
        let record = |n: &Named| crate::forget::Record {
            device: n.device.clone(),
            seq: n.seq,
            origin: n.origin.clone(),
            session: n.session.clone().unwrap_or_default(),
            ts: n.ts,
        };
        Ok(crate::forget::Preview {
            target,
            sample: picked.iter().find_map(|n| n.sample.clone()),
            records: picked.iter().map(record).collect(),
            copies: copies.iter().map(record).collect(),
            passed,
            denied: self.denied_count()?,
            sources,
            uid: None,
            scope: None,
            claims: 0,
            cards: 0,
            summaries: 0,
            written: 0,
        })
    }

    /// Slice 3b: what forgetting the session `session` of `agent` (its hash of the session's id)
    /// would register: every record of it with text or a v1 touch, whichever source's and device's
    /// in this store
    /// (no copies to add: every source is picked), an import's by the session its importer hashed,
    /// a live one's by its label when that is the agent's own id; a forgotten one left out. Its
    /// ambiguous transcript records are taken: the session's denial refuses its records from before
    /// it that arrive later (D3), so a re-import cannot bring one back under another identity.
    /// The records alone, as `forget_start` reads them again: `session_summary` adds the rest.
    fn session_preview(&self, agent: &str, session: &str) -> Result<crate::forget::Preview> {
        anyhow::ensure!(
            self.home_id_proven()?,
            "this store has no verified home identity: forget was not registered"
        );
        let found = self.session_records(agent, session)?;
        let mut sources = Vec::<String>::new();
        for (n, _, _) in &found {
            if !sources.contains(&n.source) {
                sources.push(n.source.clone());
            }
        }
        Ok(crate::forget::Preview {
            target: crate::forget::Target::Session {
                agent: agent.into(),
                session: session.into(),
            },
            records: found
                .iter()
                .map(|(n, _, _)| crate::forget::Record {
                    device: n.device.clone(),
                    seq: n.seq,
                    origin: n.origin.clone(),
                    session: session.into(),
                    ts: n.ts,
                })
                .collect(),
            copies: Vec::new(),
            passed: 0,
            denied: self.denied_count()?,
            sources,
            sample: None,
            uid: None,
            scope: None,
            claims: 0,
            cards: 0,
            summaries: 0,
            written: 0,
        })
    }

    /// The session's records `session_preview` takes, each with its time and kind, in (device,
    /// seq) order.
    fn session_records(&self, agent: &str, session: &str) -> Result<Found> {
        let mut found: Vec<(Named, i64, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut st = self.conn.prepare(
            "SELECT o.device, o.seq, o.origin, o.ambiguous, r.source, r.kind, r.ts
             FROM import_origins o JOIN records r ON r.device = o.device AND r.seq = o.seq
             WHERE o.native_session = ?1 AND r.type = 'event'",
        )?;
        let rows = st
            .query_map([session], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (device, seq, origin, ambiguous, source, kind, ts) in rows {
            let Some(family) = Family::of(&source) else {
                continue;
            };
            if denied(&self.conn, Some(&origin), "")? || !seen.insert((device.clone(), seq)) {
                continue;
            }
            found.push((
                Named {
                    device,
                    seq,
                    // Rule 10: only a record the transcript cut counts keeps its time.
                    ts: (family != Family::Transcript && kind != "touch").then_some(ts),
                    origin,
                    session: Some(session.into()),
                    family,
                    agent: Some(agent.into()),
                    ambiguous,
                    source,
                    sample: None,
                },
                ts,
                kind,
            ));
        }
        let live = LIVE.map(|s| format!("'{s}'")).join(",");
        let labels: Vec<String> = self
            .conn
            .prepare(&format!(
                "SELECT DISTINCT session FROM records WHERE type = 'event' AND agent = ?1
                   AND source IN ({live}) AND session IS NOT NULL"
            ))?
            .query_map([agent], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        // A label the ledger masks is not the agent's id, whatever it reads as (`verified_session`;
        // Codex on #465).
        let mut st = self.conn.prepare(&format!(
            "SELECT device, seq, ts, source, kind FROM records WHERE type = 'event' AND agent = ?1
               AND session = ?2 AND source IN ({live})
               AND NOT EXISTS (SELECT 1 FROM ledger l WHERE l.device = records.device
                 AND l.seq = records.seq AND l.field = 'session')"
        ))?;
        for label in labels.iter().filter(|l| {
            *l != "unknown"
                && !l.starts_with("unknown-")
                && crate::forget::session(agent, l) == session
        }) {
            let rows = st
                .query_map(params![agent, label], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // A masked label hashes to another session, and live capture records no touch.
            for (device, seq, ts, source, kind) in rows {
                let origin = crate::forget::live_origin(&device, seq, ts);
                if denied(&self.conn, Some(&origin), "")? || !seen.insert((device.clone(), seq)) {
                    continue;
                }
                found.push((
                    Named {
                        device,
                        seq,
                        ts: Some(ts),
                        origin,
                        session: Some(session.into()),
                        family: Family::Live,
                        agent: Some(agent.into()),
                        ambiguous: false,
                        source,
                        sample: None,
                    },
                    ts,
                    kind,
                ));
            }
        }
        found.sort_by(|a, b| (&a.0.device, a.0.seq).cmp(&(&b.0.device, b.0.seq)));
        Ok(found)
    }

    /// Slice 3b-2: what forgetting a repository or a time range would register: its records with
    /// text (a repository's on every device, a time range's on its device, with those a restore
    /// kept no body of), each named as a record's is, a forgotten one left out, and their sessions'
    /// records from another source as copies (slice 3a). A record that names no session is taken,
    /// with no copies to find (the preview counts it), and so is an ambiguous transcript identity:
    /// the scope's denial refuses its records from before it that arrive later (D3), so a re-import
    /// cannot bring one back under another identity.
    fn scope_preview(&self, target: crate::forget::Target) -> Result<crate::forget::Preview> {
        anyhow::ensure!(
            self.home_id_proven()?,
            "this store has no verified home identity: forget was not registered"
        );
        let (found, passed) = self.scope_records(&target)?;
        let picked: Vec<Named> = found.into_iter().map(|(n, _, _)| n).collect();
        let copies = self.copies(&picked)?;
        let mut sources = Vec::<String>::new();
        for n in picked.iter().chain(&copies) {
            if !sources.contains(&n.source) {
                sources.push(n.source.clone());
            }
        }
        let record = |n: &Named| crate::forget::Record {
            device: n.device.clone(),
            seq: n.seq,
            origin: n.origin.clone(),
            session: n.session.clone().unwrap_or_default(),
            ts: n.ts,
        };
        Ok(crate::forget::Preview {
            target,
            sample: None,
            records: picked.iter().map(record).collect(),
            copies: copies.iter().map(record).collect(),
            passed,
            denied: self.denied_count()?,
            sources,
            uid: None,
            scope: None,
            claims: 0,
            cards: 0,
            summaries: 0,
            written: 0,
        })
    }

    /// Slice 3b-2: a repository's or a time range's records with text, each with its time and
    /// kind, in (device, seq) order, named in one read (a repository may hold most of a store) as
    /// `named` names a record, a forgotten one left out; a time range's records a restore kept no
    /// body of are named one by one, and those with no identity left are passed over (how many).
    fn scope_records(&self, target: &crate::forget::Target) -> Result<(Found, usize)> {
        use rusqlite::types::Value;
        let (clause, args): (&str, Vec<Value>) = match target {
            // A label the ledger masks is not the repository's, whatever it reads as: the preview
            // counts such records, not taken (Greptile, Codex on #468).
            crate::forget::Target::Repo { repo, .. } => (
                "r.repo IN (SELECT value FROM json_each(?1)) AND NOT EXISTS (SELECT 1 FROM ledger l
                   WHERE l.device = r.device AND l.seq = r.seq AND l.field = 'repo')",
                vec![serde_json::to_string(&self.repo_labels(repo)?)?.into()],
            ),
            crate::forget::Target::Range {
                device,
                since,
                until,
            } => (
                "r.device = ?1 AND r.ts >= ?2 AND r.ts < ?3",
                vec![device.clone().into(), (*since).into(), (*until).into()],
            ),
            _ => anyhow::bail!("not a repository or a time range"),
        };
        type Row = (
            String,
            i64,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<bool>,
            bool,
        );
        let rows: Vec<Row> = self
            .conn
            .prepare(&format!(
                "SELECT r.device, r.seq, r.ts, r.source, r.kind, r.agent, r.session, o.origin,
                        o.native_session, o.ambiguous,
                        EXISTS(SELECT 1 FROM ledger l WHERE l.device = r.device AND l.seq = r.seq
                          AND l.field = 'session')
                 FROM records r LEFT JOIN import_origins o ON o.device = r.device AND o.seq = r.seq
                 WHERE r.type = 'event' AND {clause}"
            ))?
            .query_map(rusqlite::params_from_iter(args), |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut found = Vec::with_capacity(rows.len());
        for (device, seq, ts, source, kind, agent, label, origin, native, ambiguous, masked) in rows
        {
            let family = Family::of(&source).with_context(|| {
                format!("record {device}:{seq} has an unknown source: nothing was registered")
            })?;
            let (origin, session, ambiguous) = if family == Family::Live {
                // Its session is its label when that is its agent's own id (slice 3a): unmasked,
                // and not the `unknown` a hook writes for an event that sent none.
                let session = agent.as_deref().zip(label.as_deref()).and_then(|(a, l)| {
                    (!masked && l != "unknown" && !l.starts_with("unknown-"))
                        .then(|| crate::forget::session(a, l))
                });
                (crate::forget::live_origin(&device, seq, ts), session, false)
            } else {
                let origin = origin.with_context(|| {
                    format!("record {device}:{seq} has no import identity: nothing was registered")
                })?;
                let session = native.with_context(|| {
                    format!(
                        "record {device}:{seq} has no native session identity: nothing was registered"
                    )
                })?;
                (origin, Some(session), ambiguous.unwrap_or(true))
            };
            if denied(&self.conn, Some(&origin), "")? {
                continue;
            }
            found.push((
                Named {
                    // Rule 10: only a record the transcript cut counts keeps its time.
                    ts: (family != Family::Transcript && kind != "touch").then_some(ts),
                    device,
                    seq,
                    origin,
                    session,
                    family,
                    agent,
                    ambiguous,
                    source,
                    sample: None,
                },
                ts,
                kind,
            ));
        }
        let mut passed = 0;
        if let crate::forget::Target::Range {
            device,
            since,
            until,
        } = target
        {
            let removed: Vec<(i64, i64, Option<String>)> = self
                .conn
                .prepare(
                    "SELECT seq, ts, kind FROM records WHERE type = 'removed' AND device = ?1
                       AND ts >= ?2 AND ts < ?3",
                )?
                .query_map(params![device, since, until], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (seq, ts, kind) in removed {
                let Some(r) = self
                    .after(device, seq - 1, 1)?
                    .into_iter()
                    .find(|r| r.seq == seq)
                else {
                    continue;
                };
                match self.named(device, &r)? {
                    Some(n) if !denied(&self.conn, Some(&n.origin), "")? => {
                        // The summary's time is the row's own, which put it in the range, and a v1
                        // touch stays one (rule 10; Codex on #468).
                        let kind = if kind.as_deref() == Some("touch") {
                            "touch"
                        } else {
                            "removed"
                        };
                        found.push((n, ts, kind.into()));
                    }
                    Some(_) => {}
                    None => passed += 1,
                }
            }
        }
        found.sort_by(|a, b| (&a.0.device, a.0.seq).cmp(&(&b.0.device, b.0.seq)));
        Ok((found, passed))
    }

    /// Slice 3b-2: the repository labels records hold whose hash is `repo`: its key, as every
    /// source's records name it. Through `records_repo` (`repo_index`).
    fn repo_labels(&self, repo: &str) -> Result<Vec<String>> {
        let labels: Vec<String> = self
            .conn
            .prepare("SELECT DISTINCT repo FROM records WHERE type = 'event' AND repo IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(labels
            .into_iter()
            .filter(|l| crate::forget::repo(l) == repo)
            .collect())
    }

    /// The index of the events' repositories, built the first time something reads them by
    /// repository: an exclusion (`sessions_in`) or a forget's preview; never by `open` or a hook.
    pub(crate) fn repo_index(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS records_repo ON records(repo) WHERE type = 'event'",
        )?;
        Ok(())
    }

    /// Slice 3b: a session's, a repository's or a time range's preview lines (spec 6.2): its first
    /// record's time, its records by source and device, its first prompt's first characters, and
    /// how many records that name no session would keep a copy (a session's: its agent's live
    /// records; a repository's or a time range's: its own); a repository's keys, the paths its
    /// records were recorded at, and how many records' repository is masked.
    pub(crate) fn scope_summary(&self, preview: &mut crate::forget::Preview) -> Result<()> {
        let found = match &preview.target {
            crate::forget::Target::Session { agent, session } => {
                self.session_records(agent, session)?
            }
            crate::forget::Target::Repo { .. } | crate::forget::Target::Range { .. } => {
                self.scope_records(&preview.target)?.0
            }
            _ => return Ok(()),
        };
        let mut sources = std::collections::BTreeMap::<String, usize>::new();
        let mut devices = std::collections::HashSet::new();
        for (n, _, _) in &found {
            *sources.entry(n.source.clone()).or_default() += 1;
            devices.insert(n.device.clone());
        }
        if let Some((n, _, _)) = found
            .iter()
            .filter(|(_, _, kind)| kind == "prompt")
            .min_by_key(|(_, ts, _)| *ts)
        {
            preview.sample = self
                .after(&n.device, n.seq - 1, 1)?
                .into_iter()
                .find_map(|r| match r.item {
                    Item::Event(e) if r.seq == n.seq => Some(e.body.chars().take(120).collect()),
                    _ => None,
                });
        }
        let mut scope = crate::forget::ScopePreview {
            // Rule 10: a v1 touch keeps no time of the session (CodeRabbit on #465).
            first: found
                .iter()
                .filter(|(_, _, kind)| kind != "touch")
                .map(|(_, ts, _)| *ts)
                .min(),
            sources: sources.into_iter().collect(),
            devices: devices.len(),
            idless: found.iter().filter(|(n, _, _)| n.session.is_none()).count(),
            ..Default::default()
        };
        match &preview.target {
            crate::forget::Target::Session { agent, .. } => {
                let live = LIVE.map(|s| format!("'{s}'")).join(",");
                let idless: i64 = self.conn.query_row(
                    &format!(
                        "SELECT COUNT(*) FROM records r WHERE r.type = 'event' AND r.agent = ?1
                           AND r.source IN ({live}) AND (r.session IS NULL OR r.session = 'unknown'
                           OR r.session LIKE 'unknown-%' OR EXISTS(SELECT 1 FROM ledger l
                             WHERE l.device = r.device AND l.seq = r.seq AND l.field = 'session'))"
                    ),
                    [agent],
                    |r| r.get(0),
                )?;
                scope.agent = Some(agent.clone());
                scope.idless = usize::try_from(idless).unwrap_or(0);
            }
            crate::forget::Target::Repo { repo, .. } => {
                scope.keys = self.repo_labels(repo)?;
                // Where the records it takes were recorded: no forgotten record's, nor one whose
                // repository label is masked (Codex on #468).
                scope.paths = self
                    .conn
                    .prepare(
                        "SELECT DISTINCT r.cwd FROM records r WHERE r.type = 'event'
                           AND r.cwd IS NOT NULL AND r.repo IN (SELECT value FROM json_each(?1))
                           AND NOT EXISTS (SELECT 1 FROM ledger l WHERE l.device = r.device
                             AND l.seq = r.seq AND l.field = 'repo')
                           AND NOT EXISTS (SELECT 1 FROM import_origins o
                             JOIN denied_records d ON d.origin = o.origin
                             WHERE o.device = r.device AND o.seq = r.seq)
                         ORDER BY r.cwd LIMIT 5",
                    )?
                    .query_map([serde_json::to_string(&scope.keys)?], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let masked: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM (SELECT DISTINCT device, seq FROM ledger
                       WHERE field = 'repo')",
                    [],
                    |r| r.get(0),
                )?;
                scope.masked = usize::try_from(masked).unwrap_or(0);
            }
            _ => {}
        }
        preview.scope = Some(scope);
        Ok(())
    }

    /// Slice 3b: the session a record was recorded in, by its agent and its hash: an import's as
    /// its importer kept it, a live record's from its label when that is its agent's own id.
    pub(crate) fn session_of(&self, device: &str, seq: i64) -> Result<(String, String)> {
        use rusqlite::OptionalExtension;
        let row: Option<(Option<String>, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT agent, session, source FROM records WHERE device = ?1 AND seq = ?2
                   AND type = 'event'",
                params![device, seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((Some(agent), label, source)) = row else {
            anyhow::bail!("no record {device}:{seq} of an agent to name a session by");
        };
        // An import names its session only by the identity its importer kept: the session's other
        // imports are found by it, and one from before import identities would be left out of its
        // own session's forget (Codex on #465).
        let session = if is_live(&source) {
            match label {
                Some(label) => self.verified_session(device, seq, &agent, &label)?,
                None => None,
            }
        } else {
            self.origin_of(device, seq)?
                .and_then(|(_, session, _)| session)
        };
        let session = session.with_context(|| {
            format!("record {device}:{seq} names no session its agent sent: nothing was registered")
        })?;
        Ok((agent, session))
    }

    /// The record at `device`:`seq` as forget names it: an import's identity as its importer kept
    /// it, or a live record's computed (slice 3a); none for a record with neither text nor an
    /// identity (restored without its body, and with no metadata or labels to name it by).
    fn named(&self, device: &str, r: &Record) -> Result<Option<Named>> {
        use rusqlite::OptionalExtension;
        let seq = r.seq;
        let sample = match &r.item {
            Item::Event(e) => Some(e.body.chars().take(120).collect()),
            _ => None,
        };
        if let Some((origin, session, ambiguous)) = self.origin_of(device, seq)? {
            let (source, kind, ts) = match &r.item {
                Item::Event(e) => (e.source.clone(), e.kind.clone(), e.ts),
                _ => self.original_metadata(device, seq)?.with_context(|| {
                    format!(
                        "record {device}:{seq} has no original import metadata: nothing was registered"
                    )
                })?,
            };
            let family = Family::of(&source).with_context(|| {
                format!("record {device}:{seq} has an unknown source: nothing was registered")
            })?;
            let session = session.with_context(|| {
                format!(
                    "record {device}:{seq} has no native session identity: nothing was registered"
                )
            })?;
            let agent = self
                .conn
                .query_row(
                    "SELECT agent FROM records WHERE device = ?1 AND seq = ?2",
                    params![device, seq],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            return Ok(Some(Named {
                device: device.into(),
                seq,
                // Rule 10: only a record the transcript cut counts keeps its time.
                ts: (family != Family::Transcript && kind != "touch").then_some(ts),
                origin,
                session: Some(session),
                family,
                agent,
                ambiguous,
                source,
                sample,
            }));
        }
        let row = self
            .conn
            .query_row(
                "SELECT type, ts, source, agent, session FROM records WHERE device = ?1 AND seq = ?2",
                params![device, seq],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((kind, ts, source, agent, label)) = row else {
            return Ok(None);
        };
        match (kind.as_str(), is_live(&source), agent, label) {
            ("event", true, Some(agent), Some(label)) => Ok(Some(Named {
                device: device.into(),
                seq,
                origin: crate::forget::live_origin(device, seq, ts),
                session: self.verified_session(device, seq, &agent, &label)?,
                family: Family::Live,
                agent: Some(agent),
                ambiguous: false,
                ts: Some(ts),
                source,
                sample,
            })),
            ("event", ..) => anyhow::bail!(
                "record {device}:{seq} has no import identity: nothing was registered"
            ),
            // A live record restored without its body keeps no labels; a legacy stub, no metadata.
            _ => Ok(None),
        }
    }

    /// The indexes slice 3a's copies are read through. Unindexed, finding a session's live records
    /// reads every record (2.9 s for 325,577 on an evaluation home of 2.3 GB), and a start reads
    /// them again under raw's write lock, past a hook's 2 s store wait. Made by a forget's first
    /// preview, outside that transaction; never by `open` or a hook.
    pub(crate) fn session_indexes(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS records_sessions ON records(agent, session, source)
               WHERE type = 'event';
             CREATE INDEX IF NOT EXISTS import_origins_session ON import_origins(native_session);",
        )?;
        Ok(())
    }

    /// A live record's session hash, when its label is the agent's own id (D2 keeps a masked live
    /// label from being matched): neither `unknown` (an event with no id is named for its device,
    /// `hook::own_session`) nor masked by the gate (a ledger row for it).
    fn verified_session(
        &self,
        device: &str,
        seq: i64,
        agent: &str,
        label: &str,
    ) -> Result<Option<String>> {
        if label == "unknown" || label.starts_with("unknown-") {
            return Ok(None);
        }
        let masked: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ledger WHERE device = ?1 AND seq = ?2 AND field = 'session')",
            params![device, seq],
            |r| r.get(0),
        )?;
        Ok((!masked).then(|| crate::forget::session(agent, label)))
    }

    /// Slice 3a: the records that may hold a copy of `picked`'s: the records with text of the same
    /// sessions from another source, whichever device's in this store (a copied raw.db records its
    /// later ones under another). One source picked in a session takes its other two; two or more
    /// take every source's.
    fn copies(&self, picked: &[Named]) -> Result<Vec<Named>> {
        let mut families =
            std::collections::HashMap::<&str, std::collections::HashSet<Family>>::new();
        for n in picked {
            if let Some(session) = &n.session {
                families.entry(session).or_default().insert(n.family);
            }
        }
        let chosen: std::collections::HashSet<(&str, i64)> =
            picked.iter().map(|n| (n.device.as_str(), n.seq)).collect();
        let takes = |session: &str, family: Family| {
            families
                .get(session)
                .is_some_and(|f| f.len() > 1 || !f.contains(&family))
        };
        let mut out = Vec::new();
        // Imported records, by the session their importer hashed: a rescan's tombstone hides one but
        // leaves its text, which a restored Removed row no longer has.
        let mut st = self.conn.prepare(
            "SELECT o.device, o.seq, o.origin, o.ambiguous, r.source, r.kind, r.ts, r.agent
             FROM import_origins o JOIN records r ON r.device = o.device AND r.seq = o.seq
             WHERE o.native_session = ?1 AND r.type = 'event'",
        )?;
        for &session in families.keys() {
            let rows = st.query_map([session], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?;
            for row in rows {
                let (device, seq, origin, ambiguous, source, kind, ts, agent) = row?;
                let Some(family) = Family::of(&source) else {
                    continue;
                };
                if chosen.contains(&(device.as_str(), seq))
                    || kind == "touch"
                    || !takes(session, family)
                    || denied(&self.conn, Some(&origin), "")?
                {
                    continue;
                }
                out.push(Named {
                    device,
                    seq,
                    ts: (family != Family::Transcript).then_some(ts),
                    origin,
                    session: Some(session.to_owned()),
                    family,
                    agent,
                    ambiguous,
                    source,
                    sample: None,
                });
            }
        }
        // Live records, by their labels' hash: a label equal to the agent's own id is unmasked,
        // and one the ledger masks is a rule's marker, no session's (Greptile on #468). Both reads
        // go through `records_sessions` (`session_indexes`).
        let live = LIVE.map(|s| format!("'{s}'")).join(",");
        let labels: Vec<(String, String)> = self
            .conn
            .prepare(&format!(
                "SELECT DISTINCT agent, session FROM records WHERE type = 'event'
                   AND source IN ({live}) AND agent IS NOT NULL AND session IS NOT NULL"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut st = self.conn.prepare(&format!(
            "SELECT device, seq, ts, source FROM records WHERE type = 'event' AND agent = ?1
               AND session = ?2 AND source IN ({live})
               AND NOT EXISTS (SELECT 1 FROM ledger l WHERE l.device = records.device
                 AND l.seq = records.seq AND l.field = 'session')"
        ))?;
        for (agent, label) in labels {
            let session = crate::forget::session(&agent, &label);
            if !takes(&session, Family::Live) || label == "unknown" || label.starts_with("unknown-")
            {
                continue;
            }
            let rows = st.query_map(params![agent, label], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (device, seq, ts, source) = row?;
                let origin = crate::forget::live_origin(&device, seq, ts);
                if chosen.contains(&(device.as_str(), seq))
                    || denied(&self.conn, Some(&origin), "")?
                {
                    continue;
                }
                out.push(Named {
                    device,
                    seq,
                    origin,
                    session: Some(session.clone()),
                    family: Family::Live,
                    agent: Some(agent.clone()),
                    ambiguous: false,
                    ts: Some(ts),
                    source,
                    sample: None,
                });
            }
        }
        out.sort_by(|a, b| (&a.device, a.seq).cmp(&(&b.device, b.seq)));
        Ok(out)
    }

    /// What slice 3a still refuses before anything is registered: a picked record whose copies
    /// cannot be found (a live record whose session is not its agent's own id, or an import while
    /// such a live record of its agent exists), and an ambiguous import identity in a session no
    /// live or v1 record of the request closes to every later transcript batch
    /// (`check_cross_source_forget`).
    fn check_named(&self, device: &str, picked: &[Named], copies: &[Named]) -> Result<()> {
        for n in picked {
            anyhow::ensure!(
                n.session.is_some(),
                "record {device}:{} names no session of its agent's own (masked, or none sent): \
                 the copies another source may hold cannot be found; nothing was registered",
                n.seq
            );
        }
        let all: Vec<&Named> = picked.iter().chain(copies).collect();
        let live = LIVE.map(|s| format!("'{s}'")).join(",");
        // Named for its device (`hook::own_session`), through `records_sessions`; or masked, from
        // the ledger's few rows. Any device's: a copied store keeps the old device's records.
        let unknown = |agent: Option<&str>| -> Result<bool> {
            let (a, ra) = match agent {
                Some(_) => ("AND agent = ?1", "AND r.agent = ?1"),
                None => ("", ""),
            };
            let sql = format!(
                "SELECT EXISTS(SELECT 1 FROM records WHERE type = 'event' {a}
                   AND (session = 'unknown' OR (session >= 'unknown-' AND session < 'unknown.'))
                   AND source IN ({live}))
                 OR EXISTS(SELECT 1 FROM ledger l JOIN records r
                   ON r.device = l.device AND r.seq = l.seq
                   WHERE l.field = 'session' AND r.type = 'event' AND r.source IN ({live}) {ra})"
            );
            let mut st = self.conn.prepare_cached(&sql)?;
            Ok(match agent {
                Some(agent) => st.query_row([agent], |r| r.get(0))?,
                None => st.query_row([], |r| r.get(0))?,
            })
        };
        // Only a picked import's: a copy goes for the picked record it may copy, and its own copies
        // elsewhere hold none of that record's text.
        let mut asked = std::collections::HashSet::new();
        for n in picked.iter().filter(|n| n.family != Family::Live) {
            if asked.insert(n.agent.clone()) {
                let found = unknown(n.agent.as_deref())?;
                anyhow::ensure!(
                    !found,
                    "this selection may have unverified native copies: live records whose session \
                     is not known may hold them; nothing was registered"
                );
            }
        }
        for n in all.iter().filter(|n| n.ambiguous) {
            let closed = all
                .iter()
                .any(|o| o.family != Family::Transcript && o.session == n.session);
            anyhow::ensure!(
                closed,
                "record {device}:{} has an ambiguous or unverified import identity: nothing was registered",
                n.seq
            );
        }
        Ok(())
    }

    /// Registers `preview` as job `job` in one raw write transaction (D1 rule 1): read again and
    /// compared under the write lock, so a record replaced since, or another forget since,
    /// makes it stale and nothing is written. The request, for the logs.
    pub(crate) fn forget_start(
        &mut self,
        preview: &crate::forget::Preview,
        job: &mut dyn FnMut() -> Result<String>,
        started: i64,
    ) -> Result<Vec<crate::forget::Request>> {
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
        // A binary before slice 3a cannot compute a live record's identity: it would hold a
        // version 1 request of one as applied, the record still shown. Version 3 it skips as
        // unknown (rule 9; Codex on #460).
        let live = preview.sources.iter().any(|s| is_live(s));
        // Slice 3b: a session's records go in pages of at most MAX_RECORDS, one request and job
        // each, all in this transaction (rule 9).
        let (v, pages) = match &preview.target {
            crate::forget::Target::Uid { .. } => (2, vec![Vec::new()]),
            crate::forget::Target::Session { .. } => (
                4,
                preview
                    .records
                    .chunks(crate::forget::MAX_RECORDS)
                    .map(<[_]>::to_vec)
                    .collect(),
            ),
            // Slice 3b-2: a repository's or a time range's records and their copies, likewise.
            crate::forget::Target::Repo { .. } | crate::forget::Target::Range { .. } => (
                5,
                [preview.records.as_slice(), &preview.copies]
                    .concat()
                    .chunks(crate::forget::MAX_RECORDS)
                    .map(<[_]>::to_vec)
                    .collect(),
            ),
            _ => (
                if preview.copies.is_empty() && !live {
                    1
                } else {
                    3
                },
                vec![[preview.records.as_slice(), &preview.copies].concat()],
            ),
        };
        let mut requests = Vec::with_capacity(pages.len());
        for records in pages {
            let request = crate::forget::Request {
                v,
                home: self.home_id.clone(),
                job: job()?,
                started,
                target: preview.target.clone(),
                records,
            };
            request.check()?;
            apply_request(&tx, &self.device, &request)?;
            requests.push(request);
        }
        tx.commit()?;
        Ok(requests)
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
        Ok(self.forget_requests_read()?.0)
    }

    /// The registered requests this oboete reads, and the jobs whose request it cannot: one a
    /// later oboete wrote (a target or a version this one does not know) is passed over, as its
    /// log line is, and the reconcile says so (D1 rule 9; Greptile on #465).
    pub(crate) fn forget_requests_read(
        &self,
    ) -> Result<(Vec<crate::forget::Request>, Vec<String>)> {
        let mut st = self
            .conn
            .prepare("SELECT id, request FROM forget_jobs ORDER BY started, id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let (mut out, mut unreadable) = (Vec::new(), Vec::new());
        for row in rows {
            let (id, request) = row?;
            match readable_request(&request) {
                Some(r) => out.push(r),
                None => unreadable.push(id),
            }
        }
        Ok((out, unreadable))
    }

    /// How many records forget denies: the derived writers' fence (rule 12).
    pub fn denied_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(DENIED_COUNT, [], |r| r.get(0))?)
    }

    /// What a derived writer is fenced by (`FENCE_COUNT`).
    pub fn fence_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(FENCE_COUNT, [], |r| r.get(0))?)
    }

    /// Slice 3e-1: while raw.db holds more ranges a rescan masked than the last purge that did
    /// every step paid, the masks are owed to the stored bytes: raw.db's rows, what was made from
    /// them, knowledge.db, the backups and the copies of raw.db. How many it holds then. A store
    /// from before this version, or rebuilt from segments (a new meta), has paid none.
    pub(crate) fn masks_owed(&self) -> Result<Option<String>> {
        let (held, paid): (i64, Option<String>) = self
            .conn
            .query_row(MASKS, [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let paid = paid.and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
        Ok((held > paid).then(|| held.to_string()))
    }

    /// Once a purge that read `seen` (`masks_owed`) has done every step: a range that came
    /// meanwhile is owed still. The uids kept for it (`keep_masked_uids`) go with it: every window
    /// that named one was judged by then.
    pub(crate) fn masks_paid(&self, seen: &str) -> Result<()> {
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('rescan_masks_paid', ?1)",
            [seen],
        )?;
        tx.execute("DELETE FROM meta WHERE key = 'rescan_masked_uids'", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Slice 3e-1 (#483): the uid of each claim made from a masked record's text before its mask,
    /// with the time of that mask, kept in meta `rescan_masked_uids` from before any line of the
    /// claim loses its text until the masks are paid: a purge stopped after it rewrote the claim's
    /// op or segment line, but before a segment's or a copy's window that named it, leaves the next
    /// one the uid to judge that window by. A uid holds no text (MUST-M18). With `uids` added,
    /// each at the later of its times (`later`): what is kept now.
    pub(crate) fn keep_masked_uids(
        &self,
        uids: &std::collections::HashMap<String, i64>,
    ) -> Result<std::collections::HashMap<String, i64>> {
        use rusqlite::OptionalExtension;
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        let kept: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'rescan_masked_uids'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        // One that does not parse stops the purge: paying the masks without the uids would leave a
        // window's text in a backup (CodeRabbit on #497).
        let mut kept: std::collections::HashMap<String, i64> = match kept {
            Some(text) => serde_json::from_str(&text)
                .context("raw.db's meta rescan_masked_uids does not parse")?,
            None => std::collections::HashMap::new(),
        };
        if uids.iter().any(|(uid, at)| kept.get(uid) < Some(at)) {
            for (uid, at) in uids {
                later(&mut kept, uid, *at);
            }
            tx.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('rescan_masked_uids', ?1)",
                [serde_json::to_string(&kept)?],
            )?;
            tx.commit()?;
        }
        Ok(kept)
    }

    /// Slice 3e-4 (#478): whether the text of raw.db's ops is owed a scan with the redaction rules
    /// of `version`: they were last scanned with others (or never: a store from before this
    /// version), or a purge that masked some has not finished.
    pub(crate) fn ops_text_owed(&self, version: &str) -> Result<bool> {
        let (scanned, pending): (Option<String>, bool) = self.conn.query_row(
            "SELECT (SELECT value FROM meta WHERE key = 'ops_text_scanned'),
                    EXISTS(SELECT 1 FROM meta WHERE key = 'ops_text_pending')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(pending || scanned.as_deref() != Some(version))
    }

    /// Slice 3e-4: whether a purge masked op text it has not finished with: knowledge.db and the
    /// backups owe it.
    pub(crate) fn ops_text_pending(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'ops_text_pending')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Slice 3e-4: up to `limit` ops after `after` (a device and an op seq, in that order) whose
    /// text `rules` mask now (`masked_op_text`), each with its device, op seq, its body as read and
    /// as masked, in one read with no write lock. A purge pages through the log with it, so a rule
    /// that matches much of it holds one page at a time (Codex on #487).
    pub(crate) fn op_text_masks(
        &self,
        rules: &crate::redact::Rules,
        after: Option<&(String, i64)>,
        limit: usize,
    ) -> Result<Vec<OpText>> {
        let (device, op_seq) = after.map_or(("", i64::MIN), |(d, s)| (d.as_str(), *s));
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, type, body FROM ops WHERE (device, op_seq) > (?1, ?2)
             ORDER BY device, op_seq",
        )?;
        let mut rows = st.query(params![device, op_seq])?;
        let mut found = Vec::new();
        while found.len() < limit
            && let Some(row) = rows.next()?
        {
            let Some(kind) = OpKind::from_name(row.get_ref(2)?.as_str()?) else {
                continue;
            };
            let body: String = row.get(3)?;
            if let Some(now) = masked_op_text(kind, &body, rules) {
                found.push((row.get(0)?, row.get(1)?, body, now));
            }
        }
        Ok(found)
    }

    /// Slice 3e-4: `found` (`op_text_masks`) written, in transactions of `RECORD_PURGE_BATCH` ops
    /// under dispatch, each op only while its body is the one read, with the pending mark: the
    /// rebuild and the backups owe it until `ops_text_scanned`. How many ops it rewrote.
    pub(crate) fn mask_op_text(
        &mut self,
        found: &[OpText],
        rules: &crate::redact::Rules,
    ) -> Result<Option<usize>> {
        let mut rewritten = 0;
        for batch in found.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home)
                .context("the text of the ops was not masked")?;
            // Owner decision 43: under the hold a save that changes the rules also takes, the
            // rules are still the ones the page was read with, or nothing more is masked (Codex on
            // #502).
            if crate::redact::Rules::load(&self.home)
                .map(|now| now.version() != rules.version())
                .unwrap_or(true)
            {
                return Ok(None);
            }
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            tx.execute(
                "INSERT OR IGNORE INTO meta(key, value) VALUES('ops_text_pending', '1')",
                [],
            )?;
            {
                let mut st = tx.prepare_cached(
                    "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3 AND body = ?4",
                )?;
                for (device, op_seq, was, now) in batch {
                    rewritten += st.execute(params![now, device, op_seq, was])?;
                }
            }
            tx.commit()?;
        }
        Ok(Some(rewritten))
    }

    /// Owner decision 43: the rules the text of the ops this handle appends is gated with. Turned
    /// off, they gate nothing, so each append owes the op text scan in its own transaction
    /// (`owe_op_text_scan`). Each writer of op text calls it with its rules before it appends.
    pub(crate) fn gates_with(&mut self, rules: &crate::redact::Rules) {
        self.gated_off = rules.is_off();
    }

    /// Slice 3e-4: a scan of the op text begun in this raw.db, which `ops_text_scanned` records
    /// only while it holds the mark: a raw.db a restore made meanwhile (a new meta) holds the ops
    /// its segments held, and stays owed. The mark is this scan's own (the value returned), so a
    /// scan whose mark an append under rules turned off took away cannot record under the mark of
    /// a scan begun after (owner decision 43; Codex's security review of the switch); a scan begun
    /// later replaces an earlier one's, which then records nothing.
    pub(crate) fn op_text_scan_begun(&self) -> Result<String> {
        static SCANS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mark = format!(
            "{}-{}-{}",
            crate::db::now_ms(),
            std::process::id(),
            SCANS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('ops_text_scanning', ?1)",
            [&mark],
        )?;
        Ok(mark)
    }

    /// Slice 3e-4: the text of raw.db's ops masked with the rules of `version` everywhere a purge
    /// puts it, recorded with the marks taken away, in one transaction, when the scan was begun in
    /// this raw.db (`op_text_scan_begun`) and its mark (`mark`) is still the one there. Whether it
    /// was.
    pub(crate) fn ops_text_scanned(&self, version: &str, mark: &str) -> Result<bool> {
        let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
        let begun = tx.execute(
            "DELETE FROM meta WHERE key = 'ops_text_scanning' AND value = ?1",
            [mark],
        )? == 1;
        if begun {
            tx.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('ops_text_scanned', ?1)",
                [version],
            )?;
            tx.execute("DELETE FROM meta WHERE key = 'ops_text_pending'", [])?;
        }
        tx.commit()?;
        Ok(begun)
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
        forgotten_set(&self.conn)
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

    /// Milestone 5 slice 3c: every forgotten record (an event whose identity a denial names, D2)
    /// rewritten as a Removed row, in transactions of `RECORD_PURGE_BATCH` taken in registration's
    /// order (dispatch, then raw), each checking the identity again: device, seq, ts, source and
    /// kind stay (its live identity, `original_metadata`); its body, labels and ledger rows go (a
    /// ledger field names keys of the body). How many it rewrote.
    pub(crate) fn purge_records(&mut self) -> Result<usize> {
        denied_session_index(&self.conn)?;
        let keys: Vec<(String, i64)> = self
            .conn
            .prepare(&format!(
                "SELECT o.device, o.seq FROM {FORGOTTEN_RECORDS} WHERE r.type = 'event'
                 ORDER BY o.device, o.seq"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut rewritten = 0;
        for batch in keys.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home)
                .context("the forgotten records were not purged")?;
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            {
                let mut removed = tx.prepare(&format!(
                    "UPDATE records SET {REMOVED_ROW}
                     WHERE device = ?1 AND seq = ?2 AND type = 'event' AND EXISTS (
                       SELECT 1 FROM import_origins o JOIN denied_records d ON d.origin = o.origin
                       WHERE o.device = records.device AND o.seq = records.seq)"
                ))?;
                let mut ledger = tx.prepare("DELETE FROM ledger WHERE device = ?1 AND seq = ?2")?;
                for (device, seq) in batch {
                    if removed.execute(params![device, seq])? == 1 {
                        ledger.execute(params![device, seq])?;
                        rewritten += 1;
                    }
                }
            }
            tx.commit()?;
        }
        Ok(rewritten)
    }

    /// Slice 3d-1: the claims made from forgotten records whose ops still hold text, each uid with
    /// its job, read in one pass over the claim ops: a claim op that quotes a forgotten record (any
    /// quote, spec 6.2 step 3; its row Removed or not), with the job of the first such record's
    /// denial, and an op of a uid a record job's forget op tombstones (a restore from older ops
    /// segments gives one its text back). With the keys of every op of those uids that holds text:
    /// all their claim ops, and their Correction ops.
    pub(crate) fn record_claims(&self, forgotten: &Forgotten) -> Result<RecordClaims> {
        let mut claims = RecordClaims {
            uids: forgotten.uids.clone(),
            ..RecordClaims::default()
        };
        // A store that never forgot a record, as most: its claim ops are not read.
        if forgotten.is_empty() {
            return Ok(claims);
        }
        let mut all: std::collections::HashMap<String, Vec<(String, i64)>> =
            std::collections::HashMap::new();
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, ts, body FROM ops WHERE type = 'claim'
             ORDER BY device, op_seq",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(3)?)
            else {
                continue;
            };
            let Some(uid) = crate::claims::op_uid(&body) else {
                continue;
            };
            let (key, ts): ((String, i64), i64) = ((row.get(0)?, row.get(1)?), row.get(2)?);
            match quoted_entry(forgotten, ts, &body) {
                // Slice 3e-2: a preference written again from its directive's masked text holds
                // no byte a mask hides, however soon after the mask it was written.
                Some((_, job, _)) if job == MASK_JOB && self.reads_now(&body)? => {}
                // Slice 3e-1: this op alone, its uid not tombstoned.
                Some((_, job, _)) if job == MASK_JOB => {
                    claims.masked.push(key.clone());
                    // Its latest mask: a window that showed it before then showed its text
                    // (Codex on #480).
                    later(
                        &mut claims.masked_uids,
                        &uid,
                        latest_mask(forgotten, ts, &body),
                    );
                }
                Some((_, job, _)) => {
                    claims
                        .uids
                        .entry(uid.clone())
                        .or_insert_with(|| job.clone());
                    let texts = claims.texts.entry(uid.clone()).or_default();
                    for text in claim_text(&body, true) {
                        word(texts, ts, text);
                    }
                }
                None => {}
            }
            all.entry(uid).or_default().push(key);
        }
        drop(rows);
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, json_extract(body, '$.uid'), json_extract(body, '$.body'), ts
             FROM ops WHERE type = 'correction' ORDER BY device, op_seq",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(uid) = row.get::<_, Option<String>>(2)? {
                // Slice 3d-4: the body an owner's correction gave such a claim is what a window
                // that named it was shown too (Codex on #481).
                if claims.uids.contains_key(&uid)
                    && let Ok(text) = row.get_ref(3)?.as_str()
                {
                    let texts = claims.texts.entry(uid.clone()).or_default();
                    word(texts, row.get(4)?, text.into());
                }
                all.entry(uid).or_default().push((row.get(0)?, row.get(1)?));
            }
        }
        claims.keys = all
            .into_iter()
            .filter(|(uid, _)| claims.uids.contains_key(uid))
            .collect();
        claims.uids.retain(|uid, _| claims.keys.contains_key(uid));
        Ok(claims)
    }

    /// Slice 3d: what forget took, read once for the purges of what was made from it: each
    /// forgotten record with its denial's job and when its forget tombstone was written (none
    /// when a restore from a segment written before tombstone lines kept their time lost it: then
    /// every op counts as made before it, Codex on #480), and the uids the record jobs' forget ops
    /// tombstone (`record_derived_uids`).
    pub(crate) fn forgotten_records(&self) -> Result<Forgotten> {
        let mut records: std::collections::HashMap<String, Vec<(i64, String, Option<i64>)>> =
            std::collections::HashMap::new();
        let mut st = self.conn.prepare(
            "SELECT o.device, o.seq, d.job,
               NULLIF((SELECT MIN(t.ts) FROM records t WHERE t.type = 'tombstone'
                  AND t.target_device = o.device AND t.target_seq = o.seq
                  AND t.source = 'forget' AND t.target_offset IS NULL), 0)
             FROM denied_records d JOIN import_origins o ON o.origin = d.origin",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            records
                .entry(row.get(0)?)
                .or_default()
                .push((row.get(1)?, row.get(2)?, row.get(3)?));
        }
        drop(rows);
        for seqs in records.values_mut() {
            seqs.sort();
            seqs.dedup_by_key(|r| r.0);
        }
        Ok(Forgotten {
            records,
            uids: self.record_derived_uids()?,
            ..Forgotten::default()
        })
    }

    /// Slice 3e-1: `forgotten_records`, with every record a rescan masked (`Forgotten::with_masks`),
    /// by the time of its latest mask: none when a restore lost the time of one of them, which
    /// went as late as can be, whatever the others' (Codex on #480).
    pub(crate) fn forgotten_and_masked(&self) -> Result<Forgotten> {
        let masks = self
            .conn
            .prepare(
                "SELECT target_device, target_seq, CASE WHEN MIN(ts) = 0 THEN NULL ELSE MAX(ts) END
                 FROM records WHERE type = 'tombstone' AND target_offset IS NOT NULL
                 GROUP BY target_device, target_seq",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(self.forgotten_records()?.with_masks(masks))
    }

    /// Slice 3e-1: what step 4 masks the backup's record lines and the copies' rows by (`Masks`).
    pub(crate) fn masks(&self) -> Result<Masks> {
        let mut masks = Masks::new();
        let mut st = self.conn.prepare(
            "SELECT t.target_device, t.target_seq, t.target_offset, t.target_length,
               (SELECT r.ts FROM records r WHERE r.device = t.target_device
                  AND r.seq = t.target_seq AND r.type = 'event')
             FROM records t WHERE t.type = 'tombstone' AND t.target_offset IS NOT NULL",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let mask = masks
                .entry((r.get(0)?, r.get(1)?))
                .or_insert_with(|| (None, Vec::new()));
            mask.0 = r.get(4)?;
            mask.1.push((r.get(2)?, r.get(3)?));
        }
        Ok(masks)
    }

    /// Slice 3d-2: the window and turn ops that showed the curator a forgotten record or a claim
    /// made from one (`forgotten.uids`) and still hold their text, each with the job that takes
    /// it, read in one pass over each kind (docs/milestone-5-plan.md, 3d-2); and the `Judge` that
    /// read them, which judges an op raw.db does not hold by the same rules. Slice 3d-4: with the
    /// claim ops of their batches a record job takes (rule c), read before any of it is written.
    pub(crate) fn record_ops(&self, forgotten: Forgotten) -> Result<(RecordOps, Judge)> {
        let mut found = RecordOps::default();
        let mut judge = Judge {
            forgotten,
            ..Judge::default()
        };
        if judge.forgotten.is_empty() {
            return Ok((found, judge));
        }
        judge.copied = self.copied_claims()?;
        judge.remade = self.remade(&judge.copied)?;
        found.at = crate::db::now_ms();
        // Every window op's span, in op order, with when it was written and the job that took its
        // text, already or in this pass: what a turn read (rule b). Again while a pass finds a
        // claim copied, or another wording of one: a window of another device may name one found
        // after it in op order.
        let mut said = Wordings::new();
        loop {
            let before = said.clone();
            (found.windows, found.claims) = Default::default();
            (judge.spans, judge.batches) = Default::default();
            self.window_ops(&mut found, &mut judge, &mut said)?;
            if said == before {
                break;
            }
        }
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, ts, body FROM ops WHERE type = 'turn' ORDER BY device, op_seq",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let (device, op_seq, ts): (String, i64, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(3)?)
            else {
                continue;
            };
            if let Some(job) = judge.turn(&device, ts, &body) {
                found.turns.push((device, op_seq, job));
            }
        }
        drop(rows);
        Ok((found, judge))
    }

    /// Slice 3d-4: one pass of `record_ops` over the window ops, in op order, each with the claim
    /// ops of its batch (written after it, in its append: `curate::cover`), settled
    /// (`Raw::settle`) before the next window is judged: a later one that names a claim found
    /// copied is judged by it (rule b').
    fn window_ops(
        &self,
        found: &mut RecordOps,
        judge: &mut Judge,
        said: &mut Wordings,
    ) -> Result<()> {
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, type, ts, batch, body FROM ops WHERE type IN ('window', 'claim')
             ORDER BY device, op_seq",
        )?;
        let mut rows = st.query([])?;
        let mut window: Option<Judged> = None;
        while let Some(row) = rows.next()? {
            let (device, op_seq, ts, batch): (String, i64, i64, i64) =
                (row.get(0)?, row.get(1)?, row.get(3)?, row.get(4)?);
            if row.get_ref(2)?.as_str()? == "claim" {
                if let Some(w) = window
                    .as_mut()
                    .filter(|w| w.takes && w.device == device && w.batch == batch)
                {
                    w.claims.push((op_seq, ts, row.get(5)?));
                }
                continue;
            }
            if let Some(w) = window.take() {
                self.settle(w, found, judge, said)?;
            }
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(5)?)
            else {
                continue;
            };
            let gone = body["forgotten"].as_str().map(str::to_owned);
            let job = gone.clone().or_else(|| judge.window(&device, ts, &body));
            if let (Some(from), Some(to)) = (body["from_seq"].as_i64(), body["to_seq"].as_i64()) {
                judge
                    .spans
                    .entry((device.clone(), from, to))
                    .or_default()
                    .push((op_seq, ts, job.clone()));
            }
            if let Some(job) = job {
                window = Some(Judged {
                    takes: job != MASK_JOB,
                    fresh: gone.is_none(),
                    device,
                    op_seq,
                    ts,
                    batch,
                    job,
                    body,
                    claims: Vec::new(),
                });
            }
        }
        if let Some(w) = window {
            self.settle(w, found, judge, said)?;
        }
        Ok(())
    }

    /// Slice 3d-4: window op `w`, judged, after the claim ops of its batch. One that holds its text
    /// is rewritten; when a record job takes it (a mask's is #478's), so are the claims of its
    /// batch that hold theirs and that 3d-1 and 3e-1 do not take (rule c), and of those, the ones
    /// whose body or reason repeats what its curator was shown of the forgotten records
    /// (`repeats`) are `copied`: a window that named one before then is judged by it (rule b'),
    /// and what it holds of them is what was shown. A window rewritten before (a purge before
    /// 3d-4) has its claims taken with no copy looked for: what it showed is gone.
    fn settle(
        &self,
        w: Judged,
        found: &mut RecordOps,
        judge: &mut Judge,
        said: &mut Wordings,
    ) -> Result<()> {
        if w.fresh {
            found
                .windows
                .push((w.device.clone(), w.op_seq, w.job.clone()));
        }
        if !w.takes {
            return Ok(());
        }
        judge
            .batches
            .insert((w.device.clone(), w.batch), w.job.clone());
        let (mut ops, mut kept) = (Vec::new(), Vec::new());
        for (op_seq, ts, body) in w.claims {
            // A body that does not parse may hold text: taken.
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&body) else {
                ops.push(op_seq);
                continue;
            };
            let uid = crate::claims::op_uid(&body);
            if forgotten_body(&body)
                || quoted_entry(&judge.forgotten, ts, &body).is_some()
                || uid
                    .as_ref()
                    .is_some_and(|u| judge.forgotten.uids.contains_key(u))
            {
                continue;
            }
            ops.push(op_seq);
            if let Some(uid) = uid.filter(|_| w.fresh) {
                kept.push((uid, ts, body));
            }
        }
        if ops.is_empty() {
            return Ok(());
        }
        let mut copied = Vec::new();
        if !kept.is_empty() {
            let mut shown =
                self.stored_texts(&w.device, &judge.shown_records(&w.device, w.ts, &w.body))?;
            let named = items(&w.body["candidates"])
                .iter()
                .chain(items(&w.body["carried"]));
            for uid in named.filter_map(|u| u.as_str()) {
                let texts = judge
                    .forgotten
                    .texts
                    .get(uid)
                    .into_iter()
                    .chain(said.get(uid));
                // Those written by then (Codex on #481).
                shown.extend(
                    texts
                        .flatten()
                        .filter(|(ts, _)| *ts <= w.ts)
                        .map(|(_, t)| t.clone()),
                );
            }
            for (uid, ts, body) in kept {
                if repeats(&claim_text(&body, false), &shown) {
                    // Each wording a window gave the uid, not the first one's (Codex on #481).
                    let texts = said.entry(uid.clone()).or_default();
                    for text in claim_text(&body, true) {
                        word(texts, ts, text);
                    }
                    let at = judge
                        .copied
                        .entry(uid.clone())
                        .or_insert_with(|| (w.job.clone(), found.at));
                    if at.1 < found.at {
                        // Copied again since an earlier purge took a copy, after a recuration wrote
                        // it again, maybe: a new copy no recuration has written again yet, by this
                        // job (Codex's security review on #481).
                        *at = (w.job.clone(), found.at);
                        judge.remade.remove(&uid);
                    }
                    copied.push(uid);
                }
            }
        }
        found.claims.insert(
            (w.device, w.op_seq),
            BatchClaims {
                job: w.job,
                ops,
                copied,
            },
        );
        Ok(())
    }

    /// Slice 3d-4: each claim a purge took for repeating what a window showed its curator of a
    /// forgotten record (`copied` on the window op it rewrote), with the window's job and when it
    /// lost its text (`copied_at`), the latest when more windows name it.
    fn copied_claims(&self) -> Result<std::collections::HashMap<String, (String, i64)>> {
        let mut copied = std::collections::HashMap::new();
        // `instr`, never a JSON function: a body that does not parse cannot fail the purge.
        let mut st = self.conn.prepare(
            "SELECT body FROM ops WHERE type = 'window' AND instr(body, '\"copied\":') > 0",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(0)?)
            else {
                continue;
            };
            let (Some(job), Some(at)) = (body["forgotten"].as_str(), body["copied_at"].as_i64())
            else {
                continue;
            };
            for uid in items(&body["copied"]).iter().filter_map(|u| u.as_str()) {
                let e = copied
                    .entry(uid.to_owned())
                    .or_insert_with(|| (job.to_owned(), at));
                e.1 = e.1.max(at);
            }
        }
        Ok(copied)
    }

    /// Slice 3d-4: of the claims `copied` took, when each uid was first written again after it lost
    /// its text (a recuration makes it again from its record): a window that named it from then on
    /// showed that claim.
    fn remade(
        &self,
        copied: &std::collections::HashMap<String, (String, i64)>,
    ) -> Result<std::collections::HashMap<String, i64>> {
        let mut remade = std::collections::HashMap::new();
        let Some(since) = copied.values().map(|c| c.1).min() else {
            return Ok(remade);
        };
        let mut st = self
            .conn
            .prepare("SELECT ts, body FROM ops WHERE type = 'claim' AND ts > ?1")?;
        let mut rows = st.query([since])?;
        while let Some(row) = rows.next()? {
            let ts: i64 = row.get(0)?;
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(1)?)
            else {
                continue;
            };
            if let Some(uid) = crate::claims::op_uid(&body)
                && copied.get(&uid).is_some_and(|c| ts > c.1)
            {
                let t = remade.entry(uid).or_insert(ts);
                *t = (*t).min(ts);
            }
        }
        Ok(remade)
    }

    /// Slice 3d-4: the text of `device`'s events at `seqs` as raw.db holds it, forgotten or not
    /// (`consumer::fts::text`): what a window showed its curator of them, read before their rows
    /// lose it.
    fn stored_texts(&self, device: &str, seqs: &[i64]) -> Result<Vec<String>> {
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {RECORD} FROM records WHERE device = ?1 AND type = 'event'
               AND seq IN (SELECT value FROM json_each(?2))"
        ))?;
        let rows = st.query_map(params![device, serde_json::to_string(seqs)?], record)?;
        let mut texts = Vec::new();
        for r in rows {
            if let Item::Event(e) = r?.item {
                texts.push(crate::consumer::fts::text(&e.body));
            }
        }
        Ok(texts)
    }

    /// Slice 3d-2: how step 4 judges the backup's ops and the copies' (`Judge`), with the bodies
    /// of the window and turn ops a purge rewrote in raw.db.
    pub(crate) fn judge(&self, masks: bool) -> Result<Judge> {
        let mut forgotten = if masks {
            self.forgotten_and_masked()?
        } else {
            self.forgotten_records()?
        };
        // #483: and the masked claims' uids a purge kept, whose own lines may be rewritten.
        if masks {
            forgotten.masked = self.keep_masked_uids(&forgotten.masked)?;
        }
        let (_, mut judge) = self.record_ops(forgotten)?;
        judge.rewritten = self.forgotten_op_bodies()?;
        if masks {
            judge.now = self
                .preferences_now()?
                .into_iter()
                .map(|(at, (_, body))| (at, body))
                .collect();
        }
        Ok(judge)
    }

    /// Slice 3e-2: the preference claim ops (`oboete pref add`) whose body, their one quote, reads
    /// as their directive reads now, by device and op seq, with their uids and bodies: one a purge
    /// wrote again from a directive's masked text, or one never masked, holds no byte a mask hides.
    pub(crate) fn preferences_now(
        &self,
    ) -> Result<std::collections::HashMap<(String, i64), (String, serde_json::Value)>> {
        let mut now = std::collections::HashMap::new();
        // `instr`, never a JSON function: a body that does not parse cannot fail the purge.
        let mut st = self.conn.prepare(
            "SELECT device, op_seq, body FROM ops WHERE type = 'claim'
               AND instr(body, '\"oboete pref add\"') > 0",
        )?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(2)?)
            else {
                continue;
            };
            if self.reads_now(&body)?
                && let Some(uid) = crate::claims::op_uid(&body)
            {
                now.insert((row.get(0)?, row.get(1)?), (uid, body));
            }
        }
        Ok(now)
    }

    /// Slice 3e-2: whether claim op `body` is a preference as `oboete pref add` writes it
    /// (`claims::preference_op`), field for field, its one quote its record's whole text as it
    /// reads now (`consumer::claims::live`): it holds nothing else. An op of another shape may hold
    /// more, in its `why` or in a field no op of ours has (Codex on #485); a quote of the start of
    /// a directive has the same uid and would stand in for its preference (CodeRabbit on #485).
    fn reads_now(&self, body: &serde_json::Value) -> Result<bool> {
        let Ok(c) = serde_json::from_value::<crate::claims::ClaimOp>(body.clone()) else {
            return Ok(false);
        };
        let [e] = c.evidence.as_slice() else {
            return Ok(false);
        };
        let written = serde_json::to_value(crate::claims::preference_op(e.clone()))?;
        Ok(*body == written
            && crate::consumer::claims::live(self, e)?.is_some_and(|event| {
                crate::curate::long_text(&event).is_some_and(|text| text.len() == e.quote.len())
            }))
    }

    /// Slice 3e-2 (#186): the preference claim ops to append for the directives (`oboete pref
    /// add`) a rescan masked: each as `pref add` wrote it, from the directive's text as it reads
    /// now, so with the same uid (MUST-M18: a mask keeps every byte's place, so the quote keeps
    /// its offset and length), when no op of that uid reads so already and the uid is not
    /// forgotten. Its derivation makes the preference current again with its masked text, the
    /// owner's corrections still on it. By state: a purge that stopped after taking the old op's
    /// text writes it on its next run.
    pub(crate) fn masked_preferences(
        &self,
        forgotten: &Forgotten,
    ) -> Result<Vec<(OpKind, serde_json::Value)>> {
        let now: std::collections::HashSet<String> = self
            .preferences_now()?
            .into_values()
            .map(|(uid, _)| uid)
            .collect();
        let mut ops = Vec::new();
        for (device, records) in &forgotten.records {
            for (seq, ..) in records.iter().filter(|r| r.1 == MASK_JOB) {
                let read = self
                    .after(device, seq - 1, 1)?
                    .into_iter()
                    .find(|r| r.seq == *seq);
                let Some(Item::Event(e)) = read.map(|r| r.item) else {
                    continue;
                };
                let Some(quote) = (e.kind == "directive")
                    .then(|| crate::curate::long_text(&e))
                    .flatten()
                else {
                    continue;
                };
                let evidence = crate::claims::Evidence {
                    device: device.clone(),
                    seq: *seq,
                    offset: 0,
                    length: i64::try_from(quote.len())?,
                    sentence: 0,
                    quote,
                    claim_at: None,
                };
                let uid = crate::claims::uid("preference", &evidence);
                if !now.contains(&uid) && !self.forgotten(&uid)? {
                    let op = crate::claims::preference_op(evidence);
                    ops.push((OpKind::Claim, serde_json::to_value(op)?));
                }
            }
        }
        Ok(ops)
    }

    /// Slice 3d-2: the window and turn ops of `ops` rewritten without their text, as
    /// `forgotten_window_body` and `forgotten_turn_body` say, in transactions of
    /// `RECORD_PURGE_BATCH` ops under dispatch. Slice 3d-4: each window op with the claims of its
    /// batch (`RecordOps::claims`), in its transaction, and its `copied`; the claims of a window
    /// rewritten before, alone. How many it rewrote.
    pub(crate) fn purge_record_ops(&mut self, ops: &RecordOps) -> Result<usize> {
        use rusqlite::OptionalExtension;
        let windows: std::collections::HashSet<(&str, i64)> = ops
            .windows
            .iter()
            .map(|(device, op_seq, _)| (device.as_str(), *op_seq))
            .collect();
        let all: Vec<(&str, i64, &str, OpKind)> = ops
            .windows
            .iter()
            .map(|(device, op_seq, job)| (device.as_str(), *op_seq, job.as_str(), OpKind::Window))
            .chain(
                ops.claims
                    .iter()
                    .filter(|((device, op_seq), _)| !windows.contains(&(device.as_str(), *op_seq)))
                    .map(|((device, op_seq), c)| {
                        (device.as_str(), *op_seq, c.job.as_str(), OpKind::Claim)
                    }),
            )
            .chain(ops.turns.iter().map(|(device, op_seq, job)| {
                (device.as_str(), *op_seq, job.as_str(), OpKind::Turn)
            }))
            .collect();
        let mut rewritten = 0;
        for batch in all.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home).context(
                "the window and turn ops made from the forgotten records were not purged",
            )?;
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            for &(device, op_seq, job, kind) in batch {
                let claims = ops.claims.get(&(device.to_owned(), op_seq));
                if kind != OpKind::Claim {
                    let body: Option<String> = tx
                        .query_row(
                            "SELECT body FROM ops WHERE device = ?1 AND op_seq = ?2 AND type = ?3",
                            params![device, op_seq, kind.name()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let Some(Ok(body)) =
                        body.map(|b| serde_json::from_str::<serde_json::Value>(&b))
                    else {
                        continue;
                    };
                    let body = match kind {
                        OpKind::Window => {
                            let mut body = forgotten_window_body(&body, job);
                            if let Some(c) = claims.filter(|c| !c.copied.is_empty()) {
                                body["copied"] = c.copied.clone().into();
                                body["copied_at"] = ops.at.into();
                            }
                            body
                        }
                        _ => forgotten_turn_body(&body, job),
                    };
                    rewritten += tx.execute(
                        "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3 AND body <> ?1",
                        params![body.to_string(), device, op_seq],
                    )?;
                }
                let Some(c) = claims.filter(|_| kind != OpKind::Turn) else {
                    continue;
                };
                let body = serde_json::json!({ "forgotten": c.job }).to_string();
                let mut st = tx.prepare_cached(
                    "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3 AND type = 'claim'
                       AND body <> ?1",
                )?;
                for claim in &c.ops {
                    rewritten += st.execute(params![body, device, claim])?;
                }
            }
            tx.commit()?;
        }
        Ok(rewritten)
    }

    /// Slice 3d-2: the window and turn ops a purge rewrote (`forgotten` in their bodies), by key,
    /// with their bodies: what the backup's ops segments and the copies of raw.db are made to hold.
    pub(crate) fn forgotten_op_bodies(&self) -> Result<ForgottenOps> {
        Ok(self
            .conn
            .prepare(
                "SELECT device, op_seq, body FROM ops WHERE type IN ('window', 'turn')
                   AND json_extract(body, '$.forgotten') IS NOT NULL",
            )?
            .query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Slice 3d-1: every uid a record job's forget op tombstones (its job a denial's: a restore
    /// from segments alone brings the denials back without the job rows), with that job.
    pub(crate) fn record_derived_uids(&self) -> Result<std::collections::HashMap<String, String>> {
        record_derived_uids(&self.conn)
    }

    /// Slice 3d-1: each uid of `claims` tombstoned by a forget op of its job, unless a record
    /// job's is there already, and every op of it that holds text given the body
    /// `{"forgotten": "<job>"}`, as a uid's purge rewrites them (2b), in transactions of
    /// `RECORD_PURGE_BATCH` uids under dispatch. How many ops it rewrote.
    pub(crate) fn purge_record_claims(&mut self, claims: &RecordClaims) -> Result<usize> {
        let uids: Vec<(&String, &String)> = claims.uids.iter().collect();
        let mut rewritten = 0;
        for batch in uids.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home)
                .context("the claims made from the forgotten records were not purged")?;
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            // A uid job's forget op, or a record job's whose denial a restore from segments alone
            // did not bring back, does not do: once its ops are rewritten, `record_derived_uids`
            // is how step 4 and a later purge find the uid (Codex on #471).
            let derived = record_derived_uids(&tx)?;
            for (uid, job) in batch {
                if !derived.contains_key(*uid) {
                    let body = serde_json::json!({ "uid": uid, "job": job }).to_string();
                    insert_ops(&tx, &self.device, &[(OpKind::Forget.name(), body)])?;
                }
                let body = serde_json::json!({ "forgotten": job }).to_string();
                let mut st = tx.prepare_cached(
                    "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3
                       AND body <> ?1",
                )?;
                for (device, op_seq) in claims.keys.get(*uid).into_iter().flatten() {
                    rewritten += st.execute(params![body, device, op_seq])?;
                }
            }
            tx.commit()?;
        }
        Ok(rewritten)
    }

    /// Slice 3e-1: the claim ops `ops` (`RecordClaims::masked`) given the body
    /// `{"forgotten": "rescan"}`, their uids not tombstoned, in transactions of
    /// `RECORD_PURGE_BATCH` ops under dispatch. How many it rewrote.
    pub(crate) fn purge_masked_claims(&mut self, ops: &[(String, i64)]) -> Result<usize> {
        let body = serde_json::json!({ "forgotten": MASK_JOB }).to_string();
        let mut rewritten = 0;
        for batch in ops.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home)
                .context("the claims made from the masked records were not purged")?;
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            {
                let mut st = tx.prepare_cached(
                    "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3 AND type = 'claim'
                       AND body <> ?1",
                )?;
                for (device, op_seq) in batch {
                    rewritten += st.execute(params![body, device, op_seq])?;
                }
            }
            tx.commit()?;
        }
        Ok(rewritten)
    }

    /// Slice 3e-1: each record a rescan masked, its row's body with every range of it as `*`
    /// (`masked`: as many bytes, so every offset into it holds) where it still holds one of them,
    /// and its ledger rows without their field, as an export writes them, in transactions of
    /// `RECORD_PURGE_BATCH` records under dispatch. A Removed row has nothing to mask. How many
    /// rows it rewrote.
    pub(crate) fn mask_rows(&mut self) -> Result<usize> {
        let mut masks: Vec<_> = self.masks()?.into_iter().collect();
        masks.sort_by(|a, b| a.0.cmp(&b.0));
        let mut rewritten = 0;
        for batch in masks.chunks(RECORD_PURGE_BATCH) {
            let _dispatch = crate::dispatch::exclusive(&self.home)
                .context("the masked records were not purged")?;
            self.current()?;
            let tx = begin_batch(&self.conn, crate::db::OPEN_WRITE_WAIT)?;
            for ((device, seq), (_, ranges)) in batch {
                use rusqlite::OptionalExtension;
                let row: Option<(Vec<u8>, String)> = tx
                    .query_row(
                        "SELECT body, enc FROM records
                         WHERE device = ?1 AND seq = ?2 AND type = 'event'",
                        params![device, seq],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let Some((body, enc)) = row else { continue };
                if let Some((body, enc)) = masked_row(body, &enc, ranges)? {
                    rewritten += tx.execute(
                        "UPDATE records SET body = ?1, enc = ?2 WHERE device = ?3 AND seq = ?4",
                        params![body, enc, device, seq],
                    )?;
                }
                tx.execute(
                    "UPDATE ledger SET field = '~tombstoned'
                     WHERE device = ?1 AND seq = ?2 AND field <> '~tombstoned'",
                    params![device, seq],
                )?;
            }
            tx.commit()?;
        }
        Ok(rewritten)
    }

    /// Slice 3e-1: whether a row a rescan masked still holds a byte under one of its masks: a
    /// restore since the purge masked them put it back (Codex on #480).
    pub(crate) fn rows_hold_masks(&self) -> Result<bool> {
        use rusqlite::OptionalExtension;
        let mut st = self.conn.prepare_cached(
            "SELECT body, enc FROM records WHERE device = ?1 AND seq = ?2 AND type = 'event'",
        )?;
        for ((device, seq), (_, ranges)) in self.masks()? {
            let row: Option<(Vec<u8>, String)> = st
                .query_row(params![device, seq], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((body, enc)) = row
                && masked_row(body, &enc, &ranges)?.is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The jobs a forgotten record of which still has its body (slice 3c: their step 2 is not
    /// done), in one read.
    pub(crate) fn record_jobs_holding(&self) -> Result<std::collections::HashSet<String>> {
        Ok(self
            .conn
            .prepare(&format!(
                "SELECT DISTINCT d.job FROM {FORGOTTEN_RECORDS} WHERE r.type = 'event'"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Slice 3c-2: whether rows of forgotten records were rewritten and the backup's segments and
    /// the copies of raw.db set aside may still hold their text, or the sessions' shown values the
    /// uids of the claims made from them (3d-1). Kept in raw.db, so a purge that stops before steps
    /// 4 and 5 are done owes them after a denial with no job row too (a restore from segments
    /// alone, Codex on #470 and #471), which has no step to owe them by.
    pub(crate) fn record_backups_owed(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'record_backups_owed')",
            [],
            |r| r.get(0),
        )?)
    }

    /// `record_backups_owed` set (before the rows are rewritten) or cleared (once step 5 is done).
    pub(crate) fn owe_record_backups(&self, owed: bool) -> Result<()> {
        if owed {
            self.conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('record_backups_owed', '1')",
                [],
            )?;
        } else {
            self.conn
                .execute("DELETE FROM meta WHERE key = 'record_backups_owed'", [])?;
        }
        Ok(())
    }

    /// Every forgotten record's identity (slice 3c-2): what a backup's line and a copy's row are
    /// judged by.
    pub(crate) fn forgotten_origins(&self) -> Result<ForgottenOrigins> {
        let mut st = self.conn.prepare(
            "SELECT d.origin, o.origin IS NOT NULL, o.native_session, o.ambiguous
             FROM denied_records d LEFT JOIN import_origins o ON o.origin = d.origin",
        )?;
        let mut rows = st.query([])?;
        let mut out = ForgottenOrigins::new();
        while let Some(r) = rows.next()? {
            let held = if r.get(1)? {
                Some((r.get(2)?, r.get(3)?))
            } else {
                None
            };
            out.insert(r.get(0)?, held);
        }
        Ok(out)
    }

    /// Each forgotten record's vector key (slice 3c).
    pub(crate) fn forgotten_record_keys(&self) -> Result<std::collections::HashSet<String>> {
        forgotten_record_keys(&self.conn)
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
            if let Some(r) = readable_request(&request) {
                out.push((r, step));
            }
        }
        Ok(out)
    }

    /// Records that `job` has finished `step`; a step it passed already stays.
    /// A job's step set to `step` exactly, down too: a purge that does step 2 again for a job
    /// whose ops hold its text again owes the steps after it again (Codex on #453).
    pub(crate) fn restart_forget_step(&self, job: &str, step: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE forget_jobs SET step = ?2 WHERE id = ?1",
            params![job, step],
        )?;
        Ok(())
    }

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

    /// The home this store is in.
    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    /// docs/store-format.md: whether a newer oboete has stamped raw.db since this handle opened
    /// it, so a long-lived process stops writing with its own meaning of the home.
    pub(crate) fn newer_format(&self) -> bool {
        format_of(&self.conn).is_err_and(|e| e.is::<NewerFormat>())
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
        self.append_with_ledger(e, &[], "")?.context(
            "a record of a forgotten session from before its forget, or of a place never recorded: not recorded",
        )
    }

    /// `append`, with the event's redaction ledger rows in the same transaction; `ruleset` is the
    /// version of the rules that found them (`redact::Rules::version`). None, and nothing recorded,
    /// for a record of a forgotten session from before its forget (slice 3b, D3): a hook call that
    /// took its time before the registration and waited for this lock behind it; and for one of a
    /// repository or folder never recorded (slice 4a), excluded before this write took the lock.
    pub fn append_with_ledger(
        &mut self,
        e: &Event,
        ledger: &[(String, crate::redact::Finding)],
        ruleset: &str,
    ) -> Result<Option<i64>> {
        self.append_under(e, ledger, ruleset, None, None)
    }

    /// `append_with_ledger` for an event a hook captured under `settings`: what is never recorded,
    /// stored as given (owner, 2026-10-13), is matched in the write transaction as the rules now
    /// read it too, as the hook's own check reads it, so an exclusion committed after that check
    /// refuses the event though a rule masks its labels (slice 4a, Codex on #491).
    pub fn append_captured(
        &mut self,
        e: &Event,
        ledger: &[(String, crate::redact::Finding)],
        settings: &crate::capture::Settings,
        read: (&str, Option<&str>),
    ) -> Result<Option<i64>> {
        self.append_under(
            e,
            ledger,
            settings.rules.version(),
            Some(settings),
            Some(read),
        )
    }

    /// `read`: the repository and the folder as the hook read them, before the gate (security
    /// review of #489: the transaction asks what the hook's own check asks).
    fn append_under(
        &mut self,
        e: &Event,
        ledger: &[(String, crate::redact::Finding)],
        ruleset: &str,
        settings: Option<&crate::capture::Settings>,
        read: Option<(&str, Option<&str>)>,
    ) -> Result<Option<i64>> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // docs/store-format.md: a handle opened before a newer oboete stamped raw.db appends
        // nothing after (CodeRabbit on #503).
        format_of(&tx)?;
        let never = capture_exclusions_in(&tx)?;
        let now = settings.map(|s| crate::capture::never_recorded(&never, s));
        if scope_tombstoned(
            &tx,
            &self.device,
            &crate::forget::session(&e.agent, &e.session),
            e,
            ledger,
        )? || never.refuses(now.as_ref(), (e.repo.as_deref(), e.cwd.as_deref()), read)
        {
            return Ok(None);
        }
        let seq = insert_event(&tx, &self.device, e, ledger, ruleset)?;
        tx.commit()?;
        Ok(Some(seq))
    }

    /// The events one hook call kept when its write failed (docs/unwritten.md U3), with their
    /// ledger rows and the kept file's `hash` when it has one, in one transaction: all of them or
    /// none. False, and nothing appended, when that file was written back before (#454: a call
    /// stopped between that commit and the file's removal left it behind). What is never recorded
    /// is also matched as labelled under `settings` now, as the hook's own check does (slice 4a,
    /// Codex on #489).
    pub fn append_kept(
        &mut self,
        events: &[crate::capture::Captured],
        ruleset: &str,
        hash: Option<&str>,
        settings: &crate::capture::Settings,
    ) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        format_of(&tx)?;
        if let Some(hash) = hash
            && tx.execute(
                "INSERT OR IGNORE INTO unwritten_written(hash, at) VALUES(?1, ?2)",
                params![hash, crate::db::now_ms()],
            )? == 0
        {
            return Ok(false);
        }
        let never = capture_exclusions_in(&tx)?;
        let now = crate::capture::never_recorded(&never, settings);
        for c in events {
            // Slice 3b (D3): a forgotten session's record from before its forget is not recorded,
            // nor one of a place excluded since (slice 4a).
            let session = crate::forget::session(&c.event.agent, &c.event.session);
            // A kept call's file holds the labels alone, never the place before the gate.
            let labels = (c.event.repo.as_deref(), c.event.cwd.as_deref());
            if !scope_tombstoned(&tx, &self.device, &session, &c.event, &c.ledger)?
                && !never.refuses(Some(&now), labels, None)
            {
                insert_event(&tx, &self.device, &c.event, &c.ledger, ruleset)?;
            }
        }
        tx.commit()?;
        Ok(true)
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
        self.append_imported_with(batch, origins, ruleset, checkpoint, None)
    }

    /// `append_imported_origins` for records an importer labelled under `settings`: what is never
    /// recorded is also matched as their labels are made now, so a rule added after an exclusion
    /// masks both alike, as the hook's check does (slice 4a, Codex on #489).
    pub fn append_imported_under(
        &mut self,
        batch: &[crate::capture::Captured],
        origins: &[ImportIdentity],
        settings: &crate::capture::Settings,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Vec<i64>> {
        let ruleset = settings.rules.version();
        self.append_imported_with(batch, origins, ruleset, checkpoint, Some(settings))
    }

    fn append_imported_with(
        &mut self,
        batch: &[crate::capture::Captured],
        origins: &[ImportIdentity],
        ruleset: &str,
        checkpoint: Option<&Checkpoint>,
        settings: Option<&crate::capture::Settings>,
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
        let never = capture_exclusions_in(&tx)?;
        let now = settings.map(|settings| crate::capture::never_recorded(&never, settings));
        let mut seqs = Vec::with_capacity(batch.len());
        for (index, c) in batch.iter().enumerate() {
            // Slice 4a: a place never recorded; the checkpoint still moves past it.
            if never.refuses(
                now.as_ref(),
                (c.event.repo.as_deref(), c.event.cwd.as_deref()),
                None,
            ) {
                continue;
            }
            let identity = origins.get(index);
            let origin = identity.map(|i| i.origin.as_str());
            let session = identity.map_or_else(
                || crate::forget::session(&c.event.agent, &c.event.session),
                |i| i.session.clone(),
            );
            // Slice 3b (D3): a record of a forgotten scope from before its forget is not imported,
            // before an ambiguous group or a denial can refuse the batch for it: a later turn of the
            // session in the same batch is still recorded (Greptile on #465).
            if scope_tombstoned(&tx, &self.device, &session, &c.event, &c.ledger)? {
                continue;
            }
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
                check_cross_source_forget(&tx, &session, &c.event.source)?;
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
    /// Milestone 5 slice 3a: whether a forget denied a record of `agent`'s `session`, by its
    /// labels' hash.
    pub fn session_forgotten(&self, agent: &str, session: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM denied_records WHERE session = ?1)",
            [crate::forget::session(agent, session)],
            |r| r.get(0),
        )?)
    }

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
                // only the original metadata the forget preview needs for its session cutoff, and a
                // live record's identity (its time, slice 3a).
                if matches!(r.item, Item::Removed)
                    && let Some((source, kind, ts)) = self.original_metadata(&r.device, r.seq)?
                {
                    v["source"] = serde_json::json!(source);
                    v["kind"] = serde_json::json!(kind);
                    v["ts"] = serde_json::json!(ts);
                }
                // A tombstone's time, by which a purge judges what was made before it (Codex on
                // #480): a restore keeps it.
                if matches!(r.item, Item::Tombstone(_)) {
                    v["ts"] = self
                        .conn
                        .prepare_cached("SELECT ts FROM records WHERE device = ?1 AND seq = ?2")?
                        .query_row(params![r.device, r.seq], |x| x.get::<_, i64>(0))?
                        .into();
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

    /// A record's original metadata, without any body or labels: an import's, and since slice 3a a
    /// live record's too, whose identity its time is part of. Older Removed stubs lack kind; their
    /// restore timestamp and source must never be guessed into a transcript cut.
    fn original_metadata(&self, device: &str, seq: i64) -> Result<Option<(String, String, i64)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT source, kind, ts FROM records WHERE device=?1 AND seq=?2
             AND type IN ('event', 'removed')
             AND source IN ('oboete-v1', 'transcript', 'hook', 'replay')
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
        self.append_ops_from(ops, None, None, None)
    }

    /// `append_ops` for claude-mem's work state lists, imported under `settings`: a list is also
    /// refused when its project's name labelled under them, as the import's own check labels it,
    /// is never recorded, so an exclusion a rule masks that commits during the import refuses the
    /// lists after it (Codex on #489).
    pub fn append_imported_ops(
        &mut self,
        ops: &[(OpKind, serde_json::Value)],
        settings: &crate::capture::Settings,
    ) -> Result<Vec<i64>> {
        self.append_ops_from(ops, None, Some(settings), None)
    }

    /// `append_ops` for ops written from `cwd`, the label of the writer's working directory. A
    /// work state op of a repository never recorded, or written from a folder never recorded, is
    /// refused here (`NeverRecorded`), after every exclusion committed before this write (slice
    /// 4a, Codex on #489).
    fn append_ops_from(
        &mut self,
        ops: &[(OpKind, serde_json::Value)],
        cwd: Option<&str>,
        settings: Option<&crate::capture::Settings>,
        read: Option<(&str, Option<&str>)>,
    ) -> Result<Vec<i64>> {
        let bodies = within_batch_cap(ops)?;
        let tx = begin_batch(&self.conn, Duration::ZERO)?;
        format_of(&tx)?;
        let lists = ops.iter().any(|(kind, _)| *kind == OpKind::WorkState);
        let never = lists.then(|| capture_exclusions_in(&tx)).transpose()?;
        let now = never
            .as_ref()
            .zip(settings)
            .map(|(never, s)| crate::capture::never_recorded(never, s));
        for (kind, body) in ops {
            anyhow::ensure!(
                !forgotten_op(&tx, *kind, body)?,
                "a forget request invalidated this derived batch; it was not recorded"
            );
            let (repo, imported) = (
                body["repo"].as_str().unwrap_or_default(),
                !body["source"].is_null(),
            );
            let labelled = |settings| crate::capture::exclusion_label("repo", repo, settings);
            if let Some(never) = &never
                && *kind == OpKind::WorkState
                && (never.refuses(now.as_ref(), (None, cwd), read)
                    || never.covers_work_state(repo, imported)
                    || now
                        .as_ref()
                        .is_some_and(|now| now.covers_work_state(repo, imported))
                    || imported
                        && settings.is_some_and(|s| never.covers_work_state(&labelled(s), true)))
            {
                return Err(NeverRecorded.into());
            }
        }
        let seqs = insert_ops(&tx, &self.device, &bodies)?;
        if self.gated_off || settings.is_some_and(|s| s.rules.is_off()) {
            owe_op_text_scan(&tx)?;
        }
        tx.commit()?;
        Ok(seqs)
    }

    /// `append_ops` for a writer of derived text (milestone 5 D1 rule 12): only while forget
    /// denies as many records as when its inputs were read (`denied`), compared inside the write;
    /// else nothing is written and the error is `curate::ListChanged`.
    pub fn append_ops_fenced(
        &mut self,
        ops: &[(OpKind, serde_json::Value)],
        fence: i64,
    ) -> Result<Vec<i64>> {
        let bodies = within_batch_cap(ops)?;
        let tx = begin_batch(&self.conn, Duration::ZERO)?;
        format_of(&tx)?;
        let now: i64 = tx.query_row(FENCE_COUNT, [], |r| r.get(0))?;
        if now != fence {
            return Err(crate::curate::ListChanged.into());
        }
        for (kind, body) in ops {
            anyhow::ensure!(
                !forgotten_op(&tx, *kind, body)?,
                "a forget request invalidated this derived batch; it was not recorded"
            );
        }
        let seqs = insert_ops(&tx, &self.device, &bodies)?;
        if self.gated_off {
            owe_op_text_scan(&tx)?;
        }
        tx.commit()?;
        Ok(seqs)
    }

    /// Up to `limit` ops of `device` after `op_seq`, in op_seq order.
    pub fn ops_after(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<Op>> {
        ops_after_in(&self.conn, device, op_seq, limit)
    }

    pub fn skip_window_ops_after(
        &self,
        device: &str,
        op_seq: i64,
        limit: usize,
    ) -> Result<Vec<Op>> {
        skip_window_ops_after_in(&self.conn, device, op_seq, limit)
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

    /// Slice 4a: what is never recorded now (`CaptureExclusions`).
    pub fn capture_exclusions(&self) -> Result<CaptureExclusions> {
        capture_exclusions_in(&self.conn)
    }

    /// `exclusions`, with each entry never recorded as the rules now read it (a repository stored
    /// as given is so labelled in the records from after a rule that masks its name), and each
    /// repository label the records hold that the rules now read as one never recorded (an entry
    /// stored masked, by the page or before exclusions were stored as given, and its records from
    /// before the rule, which hold the label unmasked; Codex on #489). The settings are read only
    /// while a repository is never recorded; one that does not load sends nothing.
    pub fn exclusions_now(&self) -> Result<Vec<String>> {
        let (mut sent, never) = exclusion_sets(&self.conn)?;
        if !never.repos.is_empty() {
            let settings = crate::capture::Settings::load(&self.home)?;
            let now = crate::capture::never_recorded(&never, &settings);
            self.repo_index()?;
            let mut st = self.conn.prepare(
                "SELECT DISTINCT repo FROM records WHERE type = 'event' AND repo IS NOT NULL",
            )?;
            for label in st.query_map([], |r| r.get::<_, String>(0))? {
                let label = label?;
                if now
                    .repos
                    .contains(&crate::capture::exclusion_label("repo", &label, &settings))
                    || may_name(&label, never.repos.iter().chain(&now.repos))
                {
                    sent.insert(label);
                }
            }
            // And the entries as the rules now read them, for a key read so (Codex and Cursor on
            // #489: an import's project, a search's checkout).
            sent.extend(now.repos);
        }
        sent.extend(never.repos);
        Ok(sent.into_iter().collect())
    }

    /// Appends an exclusion op, or its undo (spec 5.5, D13), with a clock past every exclusion op
    /// this store holds, as a Lamport clock: it takes effect after each op its device had seen,
    /// whatever the clocks of the devices that wrote them. A copied store keeps its old device's
    /// ops under a new device (Codex on #304).
    pub fn exclude(&mut self, repo: &str, undo: bool) -> Result<i64> {
        self.exclusion(serde_json::json!({ "repo": repo, "undo": undo }))
    }

    /// Slice 4a (spec 6.1): an exclusion op of level `capture`, or its undo, as `exclude` appends
    /// one: `("repo", label)` stops every device recording that repository, and sending it;
    /// `("folder", cwd label)` stops this device recording under that folder.
    pub fn exclude_capture(&mut self, (key, value): (&str, &str), undo: bool) -> Result<i64> {
        let mut op = serde_json::json!({ "undo": undo, "level": "capture" });
        op[key] = value.into();
        self.exclusion(op)
    }

    fn exclusion(&mut self, mut op: serde_json::Value) -> Result<i64> {
        let _dispatch = crate::dispatch::exclusive(&self.home)?;
        let seen: Option<i64> = self.conn.query_row(
            "SELECT MAX(COALESCE(json_extract(body, '$.clock'), ts)) FROM ops
             WHERE type = 'exclusion'",
            [],
            |r| r.get(0),
        )?;
        op["clock"] = seen
            .map_or(i64::MIN, |c| c + 1)
            .max(crate::db::now_ms())
            .into();
        Ok(self.append_ops(&[(OpKind::Exclusion, op)])?[0])
    }

    /// docs/work-state.md L4: one write of an agent's work state, `{repo, list, fields, clock}`,
    /// as this device's next op; `list` and `fields` have passed the gate (L5). Its clock is one
    /// past every work state clock the store holds and at least now, as an exclusion's; an
    /// imported entry's time, another clock's, lifts none (docs/claude-mem-import.md I5). No
    /// dispatch lock: a work state write orders against no provider call. `cwd` is the label of
    /// the writer's working directory: refused (`NeverRecorded`) in a folder never recorded, as in
    /// a repository never recorded (slice 4a).
    pub fn work_state(
        &mut self,
        repo: &str,
        list: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        cwd: Option<&str>,
    ) -> Result<i64> {
        let seen: Option<i64> = self.conn.query_row(
            "SELECT MAX(COALESCE(json_extract(body, '$.clock'), ts)) FROM ops
             WHERE type = 'work_state' AND json_extract(body, '$.source') IS NULL",
            [],
            |r| r.get(0),
        )?;
        let clock = seen.map_or(i64::MIN, |c| c + 1).max(crate::db::now_ms());
        let op = serde_json::json!({"repo": repo, "list": list, "fields": fields, "clock": clock});
        Ok(self.append_ops_from(&[(OpKind::WorkState, op)], cwd, None, None)?[0])
    }

    /// `work_state` for a writer that holds its capture settings and its repository and folder as
    /// read before the gate (`read`): the write's transaction asks what the writer's own check
    /// asks (`CaptureExclusions::refuses`; security review of #489).
    pub fn work_state_under(
        &mut self,
        repo: &str,
        list: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        cwd: &str,
        settings: &crate::capture::Settings,
        read: (&str, Option<&str>),
    ) -> Result<i64> {
        let seen: Option<i64> = self.conn.query_row(
            "SELECT MAX(COALESCE(json_extract(body, '$.clock'), ts)) FROM ops
             WHERE type = 'work_state' AND json_extract(body, '$.source') IS NULL",
            [],
            |r| r.get(0),
        )?;
        let clock = seen.map_or(i64::MIN, |c| c + 1).max(crate::db::now_ms());
        let op = serde_json::json!({"repo": repo, "list": list, "fields": fields, "clock": clock});
        let ops = [(OpKind::WorkState, op)];
        Ok(self.append_ops_from(&ops, Some(cwd), Some(settings), Some(read))?[0])
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
        self.repo_index()?;
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

    /// #167 (Codex on #473): when the last of `records` went, the latest of their first
    /// whole-record tombstones; none when one of them has none (it was never here). A tombstone a
    /// restore from segments brought back without its time (0) went as late as can be.
    pub(crate) fn gone_at<'a>(
        &self,
        records: impl IntoIterator<Item = (&'a String, i64)>,
    ) -> Result<Option<i64>> {
        let mut st = self.conn.prepare_cached(
            "SELECT COUNT(*), MIN(NULLIF(ts, 0)) FROM records WHERE type = 'tombstone'
               AND target_device = ?1 AND target_seq = ?2 AND target_offset IS NULL",
        )?;
        let mut last = None;
        for (device, seq) in records {
            let (n, at): (i64, Option<i64>) =
                st.query_row(params![device, seq], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if n == 0 {
                return Ok(None);
            }
            last = last.max(Some(at.unwrap_or(i64::MAX)));
        }
        Ok(last)
    }

    /// #167 (Codex on #473): the time and body of each correction op whose text names `uid`.
    pub(crate) fn corrections_naming(&self, uid: &str) -> Result<Vec<(i64, serde_json::Value)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts, body FROM ops WHERE type = 'correction' AND instr(body, ?1) > 0",
        )?;
        let rows = st.query_map([uid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut found = Vec::new();
        for row in rows {
            let (ts, body) = row?;
            if let Ok(body) = serde_json::from_str(&body) {
                found.push((ts, body));
            }
        }
        Ok(found)
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
        let session = crate::forget::session(agent, session);
        check_cross_source_forget(&self.conn, &session, "transcript")?;
        // Slice 3c (rule 10): a forgotten live record's row loses its labels when it is purged,
        // so its time counts from its denial, and the cut does not move later; with every live
        // record of the session purged, from its denials alone. A denial's time is a live
        // record's (or a historical v1 request's), and it counts when raw.db no longer holds the row.
        let denied: Option<i64> = self.conn.query_row(
            "SELECT MIN(ts) FROM denied_records WHERE session = ?1",
            [&session],
            |r| r.get(0),
        )?;
        Ok(match (recorded, denied) {
            (Some(recorded), Some(denied)) => Some(recorded.min(denied)),
            (recorded, denied) => recorded.or(denied),
        })
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
        // The rebuild is this binary's: its format (docs/store-format.md).
        conn.execute_batch(&format!("PRAGMA user_version = {FORMAT}"))?;
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
                     VALUES(?1, ?2, 'tombstone', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        device,
                        seq,
                        i("ts").unwrap_or(0),
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
        link_live_denials(&self.conn)?;
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

/// Slice 3a's link of a forgotten live record to its identity, made again for the denials a
/// restore brought back from tombstone lines: a segment written before the forget holds the
/// record without it. A denial whose record this store holds, live, at the time that gives the
/// same identity gets it, so the purge finds the record's text by the state when no log brings the
/// request back (Codex on #469).
fn link_live_denials(conn: &Connection) -> Result<()> {
    let live = LIVE.map(|s| format!("'{s}'")).join(",");
    let rows: Vec<(String, String, i64, String, i64)> = conn
        .prepare(&format!(
            "SELECT d.origin, d.device, d.seq, d.session, r.ts FROM denied_records d
             JOIN records r ON r.device = d.device AND r.seq = d.seq
             WHERE r.type IN ('event', 'removed') AND r.source IN ({live})
               AND r.kind IS NOT NULL AND r.kind <> ''
               AND NOT EXISTS (SELECT 1 FROM import_origins o
                               WHERE o.device = d.device AND o.seq = d.seq)"
        ))?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (origin, device, seq, session, ts) in rows {
        if crate::forget::live_origin(&device, seq, ts) == origin {
            conn.execute(
                "INSERT OR IGNORE INTO import_origins(device, seq, origin, native_session, ambiguous)
                 VALUES(?1, ?2, ?3, ?4, 0)",
                params![device, seq, origin, session],
            )?;
        }
    }
    Ok(())
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
    use rusqlite::OptionalExtension;
    // D5: one forget op per uid, whichever request or log line brings it first.
    if let crate::forget::Target::Uid { uid } = &r.target
        && !forgotten_uid(conn, uid)?
    {
        let body = serde_json::json!({"uid": uid, "job": r.job}).to_string();
        insert_ops(conn, device, &[(OpKind::Forget.name(), body)])?;
    }
    // Slice 3b (D3): the scope's records from before the registration that arrive later are
    // refused, by every request and log line that brings a page of it: a session's on any device,
    // a repository's on the device that registered it, a time range's on its device.
    let scope = match &r.target {
        crate::forget::Target::Session { session, .. } => {
            Some(("session", session.as_str(), None, None, None))
        }
        crate::forget::Target::Repo { repo, device } => {
            Some(("repo", repo.as_str(), Some(device.as_str()), None, None))
        }
        crate::forget::Target::Range {
            device,
            since,
            until,
        } => Some((
            "range",
            "",
            Some(device.as_str()),
            Some(*since),
            Some(*until),
        )),
        _ => None,
    };
    if let Some((kind, key, device, since, until)) = scope {
        conn.execute(
            "INSERT OR IGNORE INTO denied_scopes(job, kind, key, before, device, since, until)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![r.job, kind, key, r.started, device, since, until],
        )?;
    }
    if !r.records.is_empty() {
        denied_session_index(conn)?;
    }
    for rec in &r.records {
        conn.execute(
            "INSERT OR IGNORE INTO denied_records(origin, device, seq, ts, session, job)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![rec.origin, rec.device, rec.seq, rec.ts, rec.session, r.job],
        )?;
        // Slice 3a: a live record's identity is computed from where and when it was recorded. The
        // request names where; the record there is the one only when its time gives the same
        // identity, and from then on its identity is kept as an import's is.
        let live = LIVE.map(|s| format!("'{s}'")).join(",");
        let ts: Option<i64> = conn
            .query_row(
                &format!(
                    "SELECT ts FROM records WHERE device = ?1 AND seq = ?2
                       AND type IN ('event', 'removed') AND source IN ({live})
                       AND kind IS NOT NULL AND kind <> ''"
                ),
                params![rec.device, rec.seq],
                |row| row.get(0),
            )
            .optional()?;
        if ts.is_some_and(|ts| crate::forget::live_origin(&rec.device, rec.seq, ts) == rec.origin) {
            conn.execute(
                "INSERT OR IGNORE INTO import_origins(device, seq, origin, native_session, ambiguous)
                 VALUES(?1, ?2, ?3, ?4, 0)",
                params![rec.device, rec.seq, rec.origin, rec.session],
            )?;
        }
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

/// A job's request as this oboete reads it, or None when it cannot (`forget_requests_read`).
fn readable_request(request: &str) -> Option<crate::forget::Request> {
    let r: crate::forget::Request = serde_json::from_str(request).ok()?;
    r.check().ok()?;
    Some(r)
}

/// Slice 3b (D3): whether `e`, recorded on `device`, comes from before a registered forget of its
/// scope: its session (`session`, its agent's hash of the session's id), its repository on the
/// device that registered that forget, or a time range of its device it falls in. A repository
/// label its `ledger` masks is no repository's, as the preview takes none (Greptile on #468).
fn scope_tombstoned(
    conn: &Connection,
    device: &str,
    session: &str,
    e: &Event,
    ledger: &[(String, crate::redact::Finding)],
) -> Result<bool> {
    let masked = ledger.iter().any(|(field, _)| field == "repo");
    let repo = e
        .repo
        .as_deref()
        .filter(|_| !masked)
        .map(crate::forget::repo);
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM denied_scopes WHERE before >= ?1 AND (
           (kind = 'session' AND key = ?2)
           OR (kind = 'repo' AND key = ?3 AND device = ?4)
           OR (kind = 'range' AND device = ?4 AND since <= ?1 AND ?1 < until)))",
        params![e.ts, session, repo, device],
        |r| r.get(0),
    )?)
}

/// Milestone 5 D5 (2c, decision 5): each copy of raw.db set aside (`raw.db.quarantined-*`,
/// `raw.db.restored.quarantined-*`, and a stopped restore's `raw.db.restored` beside the live
/// store), with every op of a uid in `jobs` rewritten as `Raw::purge_uid` rewrites it, and (slice
/// 3c-2) every row of a forgotten record (its identity in `origins`) as `Raw::purge_records`
/// rewrites it, under secure_delete, and its WAL truncated; one that does not open, or cannot be rewritten or
/// truncated, is removed with its sidecars: it may hold the text, and no reader uses it. A
/// restore's partial build (`raw.db.restoring`, made again by each restore) is removed. How many
/// copies it rewrote and how many it removed. Run under the worker lock, as a restore is, so no
/// copy moves meanwhile.
pub(crate) fn purge_copies(
    home: &Path,
    jobs: &std::collections::HashMap<String, String>,
    origins: &ForgottenOrigins,
    masks: &Masks,
    judge: &Judge,
) -> Result<(usize, usize)> {
    let (mut rewritten, mut removed) = (0, 0);
    for copy in copies_set_aside(home)? {
        // A restore's partial build is made again by each restore: removed, not rewritten.
        let done = if copy.ends_with("raw.db.restoring") {
            None
        } else {
            rewrite_copy(&copy, jobs, origins, masks, judge).ok()
        };
        match done {
            Some(0) => {}
            Some(_) => rewritten += 1,
            None => {
                remove_with_sidecars(&copy)?;
                removed += 1;
            }
        }
    }
    // What was removed stays removed past a power loss before step 4 is recorded, as in the
    // backup location (Greptile on #453).
    #[cfg(unix)]
    if removed > 0 {
        std::fs::File::open(home)?.sync_all()?;
    }
    Ok((rewritten, removed))
}

/// The copies of raw.db set aside in `home` (2c, decision 5), the live store never among them, and
/// a restore's partial build last.
pub(crate) fn copies_set_aside(home: &Path) -> Result<Vec<std::path::PathBuf>> {
    let live = path(home);
    // A link, or another name of raw.db itself, is no copy: opened, it is the live store under a
    // name whose -wal is not its own (Codex on #458). Off Unix no identity is read.
    let live_id = crate::worker::file_id(std::fs::metadata(&live));
    let alias = |copy: &Path| -> Result<bool> {
        Ok(match std::fs::symlink_metadata(copy) {
            Ok(meta) => {
                meta.file_type().is_symlink()
                    || live_id.is_some()
                        && crate::worker::file_id(std::fs::metadata(copy)) == live_id
            }
            // Only its sidecars are left.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        })
    };
    let mut copies = Vec::new();
    for entry in std::fs::read_dir(home)? {
        let file = entry?.path();
        let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A sidecar names its copy, which goes even when only its sidecars are left: a quarantine
        // moves `-wal` before the database, and a restore's cleanup removes the database before
        // its journal; either may stop between them (Codex on #458).
        let name = SIDECARS
            .iter()
            .find_map(|s| name.strip_suffix(s))
            .unwrap_or(name);
        let copy = home.join(name);
        // Only the names a quarantine gives (`backup::fresh_name`'s stamp): a file of another
        // name is not oboete's to rewrite or remove (Codex on #453, for the segments).
        let stamped = |prefix: &str| {
            name.strip_prefix(prefix)
                .is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()))
        };
        let set_aside = stamped("raw.db.quarantined-")
            || stamped("raw.db.restored.quarantined-")
            || name == "raw.db.restored";
        if set_aside && copy != live && !copies.contains(&copy) && !alias(&copy)? {
            copies.push(copy);
        }
    }
    let partial = home.join("raw.db.restoring");
    let left = std::iter::once("").chain(SIDECARS).any(|suffix| {
        let mut name = partial.as_os_str().to_owned();
        name.push(suffix);
        std::path::Path::new(&name).exists()
    });
    if left && !alias(&partial)? {
        copies.push(partial);
    }
    Ok(copies)
}

/// The files SQLite binds to a database's name.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// One copy's ops of the uids in `jobs` rewritten, in one transaction under secure_delete, and its
/// WAL truncated: how many. Each op is judged as a backup line is (`backup::purged_line`): one
/// whose holding a uid's text cannot be told (a kind it cannot name, a body that does not parse,
/// or a claim, correction or import whose body names no uid and is not a purge's) fails it, and
/// the copy is removed (Greptile on #458).
fn rewrite_copy(
    copy: &Path,
    jobs: &std::collections::HashMap<String, String>,
    origins: &ForgottenOrigins,
    masks: &Masks,
    judge: &Judge,
) -> Result<usize> {
    let mut conn = Connection::open_with_flags(copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    // No one reads a copy: one in use is removed rather than waited for.
    conn.busy_timeout(Duration::ZERO)?;
    conn.execute_batch("PRAGMA secure_delete = ON")?;
    let tx = conn.transaction()?;
    let has = |table: &str| -> rusqlite::Result<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |r| r.get(0),
        )
    };
    let mut rewritten = 0;
    // Slice 3c-2: its rows of forgotten records, judged as an export's line is: a live record by
    // its device, seq and time, an import by its identity; an import without one stays (no forget
    // can name it).
    if !origins.is_empty() && has("records")? {
        let identities = if has("import_origins")? {
            "(SELECT o.origin FROM import_origins o WHERE o.device = r.device AND o.seq = r.seq)"
        } else {
            "NULL"
        };
        let mut keys = Vec::new();
        {
            let mut st = tx.prepare(&format!(
                "SELECT r.device, r.seq, r.ts, r.source, {identities} FROM records r
                 WHERE r.type = 'event'"
            ))?;
            let mut rows = st.query([])?;
            while let Some(row) = rows.next()? {
                let (device, seq, ts, source): (String, i64, i64, String) =
                    (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
                let identity = if is_live(&source) {
                    crate::forget::live_origin(&device, seq, ts)
                } else {
                    match row.get::<_, Option<String>>(4)? {
                        Some(origin) => origin,
                        None => continue,
                    }
                };
                if origins.contains_key(&identity) {
                    keys.push((device, seq));
                }
            }
        }
        let ledger = has("ledger")?;
        for (device, seq) in keys {
            rewritten += tx.execute(
                &format!(
                    "UPDATE records SET {REMOVED_ROW} WHERE device = ?1 AND seq = ?2 AND type = 'event'"
                ),
                params![device, seq],
            )?;
            if ledger {
                tx.execute(
                    "DELETE FROM ledger WHERE device = ?1 AND seq = ?2",
                    params![device, seq],
                )?;
            }
        }
    }
    // Slice 3e-1: its rows of records a rescan masked, as `Raw::mask_rows` writes raw.db's.
    if !masks.is_empty() && has("records")? {
        use rusqlite::OptionalExtension;
        // A copy older than the column has none to write (Task 10 added `enc`, the ledger `field`).
        let column = |table: &str, name: &str| -> rusqlite::Result<bool> {
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
                [table, name],
                |r| r.get(0),
            )
        };
        let ledger = has("ledger")? && column("ledger", "field")?;
        let enc = column("records", "enc")?;
        let read = format!(
            "SELECT body, {}, ts FROM records WHERE device = ?1 AND seq = ?2 AND type = 'event'",
            if enc { "enc" } else { "'plain'" }
        );
        for ((device, seq), (ts, ranges)) in masks {
            let row: Option<(Vec<u8>, String, i64)> = tx
                .query_row(&read, params![device, seq], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let Some((body, stored, _)) = row.filter(|r| ts.is_none_or(|ts| r.2 == ts)) else {
                continue;
            };
            if let Some((body, now)) = masked_row(body, &stored, ranges)? {
                rewritten += if enc {
                    tx.execute(
                        "UPDATE records SET body = ?1, enc = ?2 WHERE device = ?3 AND seq = ?4",
                        params![body, now, device, seq],
                    )?
                } else {
                    tx.execute(
                        "UPDATE records SET body = ?1 WHERE device = ?2 AND seq = ?3",
                        params![body, device, seq],
                    )?
                };
            }
            if ledger {
                tx.execute(
                    "UPDATE ledger SET field = '~tombstoned'
                     WHERE device = ?1 AND seq = ?2 AND field <> '~tombstoned'",
                    params![device, seq],
                )?;
            }
        }
    }
    // A copy from before the op log holds no uid's op (CodeRabbit on #458). Its ops are read and
    // rewritten a page at a time, in (device, op_seq) order, so a rule that matches much of the
    // log holds one page's bodies (Codex on #487).
    let mut keys: Vec<(String, i64, String)> = Vec::new();
    let mut after = (String::new(), i64::MIN);
    let mut more =
        (!jobs.is_empty() || !judge.forgotten.is_empty() || judge.text.is_some()) && has("ops")?;
    while more {
        let mut st = tx.prepare_cached(
            "SELECT device, op_seq, type, ts, body, batch FROM ops
             WHERE (device, op_seq) > (?1, ?2) ORDER BY device, op_seq LIMIT ?3",
        )?;
        let mut rows = st.query(params![after.0, after.1, RECORD_PURGE_BATCH as i64])?;
        let mut read = 0;
        while let Some(row) = rows.next()? {
            read += 1;
            let kind = OpKind::from_name(&row.get::<_, String>(2)?)
                .context("an op of a kind that cannot be named")?;
            let key: (String, i64) = (row.get(0)?, row.get(1)?);
            after = key.clone();
            let (ts, batch): (i64, Option<i64>) = (row.get(3)?, row.get(5)?);
            let text: String = row.get(4)?;
            // Slice 3d-2: a window or turn op given the body raw.db holds now, or one raw.db does
            // not hold judged by its own (Codex and CodeRabbit on #472).
            // A body that does not parse may hold a card's or a summary's text: it fails the copy,
            // as a claim's does (Codex on #472).
            let now = if matches!(kind, OpKind::Window | OpKind::Turn) {
                let body =
                    serde_json::from_str(&text).context("an op whose body does not parse")?;
                judge.purged(kind, (&key.0, key.1), ts, batch, &body)
            } else if matches!(kind, OpKind::Claim | OpKind::Correction | OpKind::Import) {
                let body: serde_json::Value =
                    serde_json::from_str(&text).context("an op whose body does not parse")?;
                match op_uid(kind, &body) {
                    Some(uid) => match jobs.get(&uid) {
                        Some(job) => Some(serde_json::json!({ "forgotten": job }).to_string()),
                        // A claim op raw.db does not hold that quotes a forgotten record, or of a
                        // window's batch a record job took (3d-4).
                        None => judge.purged(kind, (&key.0, key.1), ts, batch, &body),
                    },
                    None => {
                        anyhow::ensure!(forgotten_body(&body), "an op that names no uid");
                        None
                    }
                }
            } else {
                None
            };
            // Slice 3e-4: and the text left, masked with the rules now as raw.db's.
            let now = match &judge.text {
                Some(rules) => masked_op_text(kind, now.as_deref().unwrap_or(&text), rules).or(now),
                None => now,
            };
            if let Some(now) = now {
                keys.push((key.0, key.1, now));
            }
        }
        drop(rows);
        drop(st);
        for (device, op_seq, body) in keys.drain(..) {
            rewritten += tx.execute(
                "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3 AND body <> ?1",
                params![body, device, op_seq],
            )?;
        }
        more = read == RECORD_PURGE_BATCH;
    }
    tx.commit()?;
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    anyhow::ensure!(busy == 0, "{} is in use", copy.display());
    Ok(rewritten)
}

/// `db`'s sidecars, then `db`: a removal that stops between them leaves `db`, which the next
/// purge finds again (Greptile, Codex on #458). Each name is `db`'s own bytes and the suffix: a
/// path need not be UTF-8 (Codex on #458).
fn remove_with_sidecars(db: &Path) -> Result<()> {
    for suffix in SIDECARS.into_iter().chain(std::iter::once("")) {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        let file = std::path::PathBuf::from(name);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {}", file.display())),
        }
    }
    Ok(())
}

/// Slice 3c: the forgotten records, `r` each one's row, `o` its identity and `d` the denial
/// naming it.
const FORGOTTEN_RECORDS: &str = "denied_records d
    JOIN import_origins o ON o.origin = d.origin
    JOIN records r ON r.device = o.device AND r.seq = o.seq";

/// Slice 3d: what forget took (`Raw::forgotten_records`): each forgotten record by device, in seq
/// order, with its denial's job and when its forget tombstone was written (none for a preview's
/// records, not yet forgotten), and the uids of the claims made from them, with their jobs.
#[derive(Debug, Default)]
pub(crate) struct Forgotten {
    records: std::collections::HashMap<String, Vec<(i64, String, Option<i64>)>>,
    pub(crate) uids: std::collections::HashMap<String, String>,
    /// Slice 3e-1: the uids of the claims made from a masked record's text before its mask, with
    /// the time of the mask: a window that showed one before then showed its text.
    pub(crate) masked: std::collections::HashMap<String, i64>,
    /// Slice 3d-4: the text of the claims that quote a forgotten record (`RecordClaims::texts`).
    pub(crate) texts: Wordings,
}

/// Slice 3e-1: the job a masked record is `Forgotten`'s by, and that takes the text of what was
/// made from it before its mask (`{"forgotten": "rescan"}`): a rescan's range, which no forget job
/// names.
pub(crate) const MASK_JOB: &str = "rescan";

impl Forgotten {
    /// A preview's: `records` as they would be forgotten, and the claims that quote them.
    pub(crate) fn of(
        records: impl IntoIterator<Item = (String, i64)>,
        uids: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut f = Self::default();
        for (device, seq) in records {
            f.records
                .entry(device)
                .or_default()
                .push((seq, String::new(), None));
        }
        for seqs in f.records.values_mut() {
            seqs.sort();
            seqs.dedup_by_key(|r| r.0);
        }
        f.uids = uids.into_iter().map(|u| (u, String::new())).collect();
        f
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.uids.is_empty()
    }

    /// Slice 3e-1: `masks` (device, seq and the time of the record's latest mask, none when a
    /// restore lost it, as `forgotten_records` reads one) as `MASK_JOB`'s, each record a forget
    /// took aside: it is forgotten whole.
    fn with_masks(mut self, masks: Vec<(String, i64, Option<i64>)>) -> Self {
        let masks: Vec<_> = masks
            .into_iter()
            .filter(|(device, seq, _)| self.entry(device, *seq).is_none())
            .collect();
        for (device, seq, at) in masks {
            self.records
                .entry(device)
                .or_default()
                .push((seq, MASK_JOB.to_owned(), at));
        }
        for seqs in self.records.values_mut() {
            seqs.sort_by_key(|r| r.0);
        }
        self
    }

    /// The forgotten record `seq` of `device`.
    pub(crate) fn entry(&self, device: &str, seq: i64) -> Option<&(i64, String, Option<i64>)> {
        let seqs = self.records.get(device)?;
        seqs.binary_search_by_key(&seq, |r| r.0)
            .ok()
            .map(|i| &seqs[i])
    }

    /// The forgotten records of `device` from `from` to `to`.
    pub(crate) fn between(
        &self,
        device: &str,
        from: i64,
        to: i64,
    ) -> &[(i64, String, Option<i64>)] {
        let Some(seqs) = self.records.get(device).filter(|_| from <= to) else {
            return &[];
        };
        let start = seqs.partition_point(|r| r.0 < from);
        let end = seqs.partition_point(|r| r.0 <= to);
        &seqs[start..end]
    }
}

/// Slice 3d-2: how long after a record's forget tombstone an op that does not list it may still
/// have been read before it: a registration's time is taken before its transaction. Later, the
/// fence (D1 rule 12) and the op's `removed` say it was not shown.
const SHOWN_MARGIN_MS: i64 = 60_000;

/// Slice 3d-2: how long before a turn op was written its cards may have been read (its call).
const READ_MARGIN_MS: i64 = 600_000;

/// Slice 3d-2: whether an op written at `ts`, which lists `removed` (K4), may have shown the
/// forgotten record `r`: not when it lists it whole (it was gone when the op was cut), nor when it
/// was written well after the record's forget (an op with more than `MAX_REMOVED` removals lists
/// none). Otherwise a recuration of the span after the purge would be taken again, and again.
fn shown_then(removed: &[serde_json::Value], ts: i64, r: &(i64, String, Option<i64>)) -> bool {
    let whole = removed
        .iter()
        .any(|x| x[0].as_i64() == Some(r.0) && x[1].is_null());
    !whole && r.2.is_none_or(|at| ts <= at + SHOWN_MARGIN_MS)
}

/// Slice 3d-2: the window and turn ops that hold what a curator was shown of a forgotten record or
/// claim (`Raw::record_ops`): each its device, op seq and the job that takes it.
#[derive(Debug, Default)]
pub(crate) struct RecordOps {
    pub(crate) windows: Vec<(String, i64, String)>,
    pub(crate) turns: Vec<(String, i64, String)>,
    /// Slice 3d-4 (rule c): the claim ops of the batch of each window op of `windows` a record job
    /// takes, and of each window op a record job's purge rewrote before 3d-4 whose claims still
    /// hold text, by the window op's device and op seq; and when this read them, which `copied`
    /// is written with (`copied_at`).
    pub(crate) claims: std::collections::HashMap<(String, i64), BatchClaims>,
    pub(crate) at: i64,
}

/// Slice 3d-4: the claim ops of a window op's batch a purge takes (`RecordOps::claims`): the job
/// that takes them, their op seqs, and the uids of those that repeated what the window showed its
/// curator of the forgotten records (`copied`).
#[derive(Debug, Default)]
pub(crate) struct BatchClaims {
    pub(crate) job: String,
    pub(crate) ops: Vec<i64>,
    pub(crate) copied: Vec<String>,
}

/// Slice 3d-4: a window op `Raw::window_ops` judged, with the claim ops of its batch read after it
/// (op seq, time and body) when a record job takes it.
struct Judged {
    device: String,
    op_seq: i64,
    ts: i64,
    batch: i64,
    job: String,
    fresh: bool,
    takes: bool,
    body: serde_json::Value,
    claims: Vec<(i64, i64, String)>,
}

/// Slice 3d-4: how many bytes of what its curator was shown a claim of a window's batch must
/// repeat to be followed (`copied`): measured on an evaluation home, 32 kept the windows a
/// forget of one record reaches to at most 13 (docs/milestone-5-plan.md, 3d-4).
const COPIED_BYTES: usize = 32;

/// Slice 3d-4: what a claim op's `body` says: its body and reason, with its quotes when `quotes`.
fn claim_text(body: &serde_json::Value, quotes: bool) -> Vec<String> {
    let quoted = items(&body["evidence"])
        .iter()
        .filter(|_| quotes)
        .map(|e| &e["quote"]);
    [&body["body"], &body["why"]]
        .into_iter()
        .chain(quoted)
        .filter_map(|t| t.as_str().filter(|t| !t.is_empty()))
        .map(str::to_owned)
        .collect()
}

/// Slice 3d-4: whether one of `said` repeats a run of `COPIED_BYTES` bytes or more of one of
/// `shown`.
fn repeats(said: &[String], shown: &[String]) -> bool {
    let runs: Vec<&[u8]> = said
        .iter()
        .flat_map(|t| t.as_bytes().windows(COPIED_BYTES))
        .collect();
    !runs.is_empty()
        && aho_corasick::AhoCorasick::new(&runs)
            .is_ok_and(|runs| shown.iter().any(|s| runs.is_match(s.as_str())))
}

/// Slice 3d-2: the window and turn ops a purge rewrote, by device and op seq, with their bodies.
pub(crate) type ForgottenOps = std::collections::HashMap<(String, i64), String>;

/// Slice 3d-2: each window op's span (device, first and last seq) with its ops in op order: op seq,
/// when it was written, and the job that took its text.
type Spans = std::collections::HashMap<(String, i64, i64), Vec<(i64, i64, Option<String>)>>;

/// The items of a list field of an op's body; none when it is not a list.
fn items(v: &serde_json::Value) -> &[serde_json::Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

/// Slice 3d-2: how a purge judges a window, turn or claim op by its own body, as
/// `Raw::record_ops` judges raw.db's: by what forget took, and by every window op's span raw.db
/// holds (a turn's rule b). With the bodies of the window and turn ops a purge rewrote in raw.db,
/// it is what step 4 makes the backup's ops segments and the copies of raw.db hold: an op raw.db
/// does not hold (a segment set aside, a copy older or newer than raw.db) is judged by the same
/// rules (Codex and CodeRabbit on #472).
#[derive(Debug, Default)]
pub(crate) struct Judge {
    rewritten: ForgottenOps,
    forgotten: Forgotten,
    spans: Spans,
    /// Slice 3d-4: each claim taken for repeating what a window showed its curator of a forgotten
    /// record, with the window's job and when it lost its text (rule b').
    copied: std::collections::HashMap<String, (String, i64)>,
    /// Slice 3d-4: the batch (device and first op seq) of each window op a record job took, with
    /// that job: its claim ops lose their text (rule c).
    batches: std::collections::HashMap<(String, i64), String>,
    /// Slice 3d-4: when each of `copied` was written again (`Raw::remade`).
    remade: std::collections::HashMap<String, i64>,
    /// Slice 3e-2: the preference claim ops in raw.db that read as their directive reads now
    /// (`Raw::preferences_now`), with their bodies: a line or a copy's op at the same key keeps its
    /// text when it holds the same body, not another op a restore gave that op seq to (Codex on
    /// #485).
    now: std::collections::HashMap<(String, i64), serde_json::Value>,
    /// Slice 3e-4 (#478): the redaction rules every op's text is masked with, as raw.db's, when it
    /// is owed a scan with them (`masked_op_text`).
    pub(crate) text: Option<crate::redact::Rules>,
}

impl Judge {
    /// Rules (a) and (b) for a curated window op of `device` written at `ts` that holds its text:
    /// the job of the first forgotten record or claim it showed its curator, a forget's before a
    /// mask's (a forget takes the claims it wrote too, 3d-4).
    fn window(&self, device: &str, ts: i64, body: &serde_json::Value) -> Option<String> {
        if body["outcome"] != "curated" || body.get("forgotten").is_some() {
            return None;
        }
        let (from, to) = (body["from_seq"].as_i64()?, body["to_seq"].as_i64()?);
        let records = || self.shown_in(device, from, to, ts, body);
        let shown = || {
            items(&body["candidates"])
                .iter()
                .chain(items(&body["carried"]))
                .filter_map(|u| u.as_str())
        };
        records()
            .find(|r| r.1 != MASK_JOB)
            .map(|r| r.1.clone())
            .or_else(|| shown().find_map(|u| self.forgotten.uids.get(u)).cloned())
            // Slice 3d-4 (b'): a claim taken for repeating what its window showed, named before
            // it lost that text, and before a recuration wrote it again.
            .or_else(|| {
                shown().find_map(|u| {
                    let (job, at) = self.copied.get(u)?;
                    (ts <= at.saturating_add(SHOWN_MARGIN_MS)
                        && self.remade.get(u).is_none_or(|again| ts < *again))
                    .then(|| job.clone())
                })
            })
            .or_else(|| records().next().map(|r| r.1.clone()))
            // Slice 3e-1: a claim made from a masked record's text, shown before the mask.
            .or_else(|| {
                shown()
                    .filter_map(|u| self.forgotten.masked.get(u))
                    .any(|at| ts <= at.saturating_add(SHOWN_MARGIN_MS))
                    .then(|| MASK_JOB.to_owned())
            })
    }

    /// Rule (a): the forgotten records a window op of `device` over `from` to `to`, written at
    /// `ts`, showed in its span or its goals.
    fn shown_in<'a>(
        &'a self,
        device: &'a str,
        from: i64,
        to: i64,
        ts: i64,
        body: &'a serde_json::Value,
    ) -> impl Iterator<Item = &'a (i64, String, Option<i64>)> + 'a {
        let removed = items(&body["removed"]);
        let goals = items(&body["goals"]).iter().filter_map(|g| g.as_i64());
        self.forgotten
            .between(device, from, to)
            .iter()
            .chain(goals.filter_map(move |g| self.forgotten.entry(device, g)))
            .filter(move |r| shown_then(removed, ts, r))
    }

    /// Slice 3d-4: the records of a forget (not a mask's) window op `body` of `device`, written at
    /// `ts`, showed its curator (rule a), by seq.
    fn shown_records(&self, device: &str, ts: i64, body: &serde_json::Value) -> Vec<i64> {
        let (Some(from), Some(to)) = (body["from_seq"].as_i64(), body["to_seq"].as_i64()) else {
            return Vec::new();
        };
        self.shown_in(device, from, to, ts, body)
            .filter(|r| r.1 != MASK_JOB)
            .map(|r| r.0)
            .collect()
    }

    /// Rules (a) and (b) for a turn op of `device` written at `ts` that holds its text.
    fn turn(&self, device: &str, ts: i64, body: &serde_json::Value) -> Option<String> {
        if body.get("forgotten").is_some() {
            return None;
        }
        let (from, through) = (body["from"].as_i64()?, body["through"].as_i64()?);
        let read: Vec<(i64, i64)> = items(&body["read"])
            .iter()
            .filter_map(|r| Some((r[0].as_i64()?, r[1].as_i64()?)))
            .collect();
        let removed = items(&body["removed"]);
        let shown = |r: &&(i64, String, Option<i64>)| shown_then(removed, ts, r);
        let goals = items(&body["goals"]).iter().filter_map(|g| g.as_i64());
        // (a) T7's set: its own records (of any session: a Removed row has no labels), the spans it
        // read and their goals.
        std::iter::once((from, through))
            .chain(read.iter().copied())
            .flat_map(|(a, b)| self.forgotten.between(device, a, b))
            .chain(goals.filter_map(|g| self.forgotten.entry(device, g)))
            .find(shown)
            .map(|r| r.1.clone())
            // (b) A card it read of a forgotten window: one written before it, whose span's next
            // window that is not forgotten came after its read (`READ_MARGIN_MS` before it).
            .or_else(|| {
                read.iter().find_map(|&(a, b)| {
                    let ops = self.spans.get(&(device.to_owned(), a, b))?;
                    ops.iter().enumerate().find_map(|(n, (_, at, gone))| {
                        let gone = gone.as_ref().filter(|_| *at <= ts)?;
                        let next = ops[n + 1..].iter().find(|(_, _, j)| j.is_none());
                        next.is_none_or(|(_, then, _)| *then > ts - READ_MARGIN_MS)
                            .then(|| gone.clone())
                    })
                })
            })
    }

    /// The body an op of `kind` at `at` (device, op seq), written at `ts` in `batch`, is to hold
    /// once a purge takes its text: a window or turn op's as raw.db holds it rewritten, else by the
    /// rules; a claim op's that quotes a forgotten record, the job of the first such record's
    /// denial, or of a window op's batch a record job took, that job (3d-4, rule c).
    pub(crate) fn purged(
        &self,
        kind: OpKind,
        at: (&str, i64),
        ts: i64,
        batch: Option<i64>,
        body: &serde_json::Value,
    ) -> Option<String> {
        match kind {
            OpKind::Window | OpKind::Turn => {
                if let Some(now) = self.rewritten.get(&(at.0.to_owned(), at.1)) {
                    return Some(now.clone());
                }
                let body = match kind {
                    OpKind::Window => forgotten_window_body(body, &self.window(at.0, ts, body)?),
                    _ => forgotten_turn_body(body, &self.turn(at.0, ts, body)?),
                };
                Some(body.to_string())
            }
            OpKind::Claim => {
                if self
                    .now
                    .get(&(at.0.to_owned(), at.1))
                    .is_some_and(|now| now == body)
                {
                    return None;
                }
                let job = match quoted_entry(&self.forgotten, ts, body) {
                    Some(r) => &r.1,
                    None => self.batches.get(&(at.0.to_owned(), batch?))?,
                };
                Some(serde_json::json!({ "forgotten": job }).to_string())
            }
            _ => None,
        }
    }

    /// What the backup's ops segments and the copies of raw.db hold that raw.db does not, read
    /// before any of their ops is judged (Codex on #472): the uids of the claims among them made
    /// from a forgotten record, which a window's candidates and carried name, then the spans of
    /// the window ops among them, which a turn reads (rule b), with the job that takes each.
    pub(crate) fn widen(&mut self, home: &Path) -> Result<()> {
        let (mut uids, mut masked) = (
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        );
        let mut claim = |kind: OpKind, _: (&str, i64), ts: i64, _, body: &serde_json::Value| {
            if kind == OpKind::Claim
                && let (Some(uid), Some(r)) =
                    (op_uid(kind, body), quoted_entry(&self.forgotten, ts, body))
            {
                // Slice 3e-1: a masked record's claim is not forgotten by its uid.
                match r {
                    (_, job, _) if job == MASK_JOB => {
                        later(&mut masked, &uid, latest_mask(&self.forgotten, ts, body));
                    }
                    (_, job, _) => {
                        uids.entry(uid).or_insert_with(|| job.clone());
                    }
                }
            }
        };
        crate::backup::each_op(home, &mut claim)?;
        each_copy_op(home, &mut claim)?;
        for (uid, job) in uids {
            self.forgotten.uids.entry(uid).or_insert(job);
        }
        for (uid, at) in masked {
            later(&mut self.forgotten.masked, &uid, at);
        }
        let held: std::collections::HashSet<(String, i64)> = self
            .spans
            .iter()
            .flat_map(|((device, ..), ops)| ops.iter().map(|op| (device.clone(), op.0)))
            .collect();
        let (mut windows, mut batches) = (Vec::new(), Vec::new());
        let mut window = |kind: OpKind,
                          at: (&str, i64),
                          ts: i64,
                          batch: Option<i64>,
                          body: &serde_json::Value| {
            if kind != OpKind::Window || held.contains(&(at.0.to_owned(), at.1)) {
                return;
            }
            let (Some(from), Some(to)) = (body["from_seq"].as_i64(), body["to_seq"].as_i64())
            else {
                return;
            };
            let job = match body["forgotten"].as_str() {
                Some(job) => Some(job.to_owned()),
                None => self.window(at.0, ts, body),
            };
            // Slice 3d-4: the claims of its batch with it, with no copy looked for (rule c).
            if let (Some(job), Some(batch)) = (job.as_ref().filter(|j| *j != MASK_JOB), batch) {
                batches.push(((at.0.to_owned(), batch), job.clone()));
            }
            windows.push(((at.0.to_owned(), from, to), (at.1, ts, job)));
        };
        crate::backup::each_op(home, &mut window)?;
        each_copy_op(home, &mut window)?;
        for (batch, job) in batches {
            self.batches.entry(batch).or_insert(job);
        }
        for (span, op) in windows {
            let ops = self.spans.entry(span).or_default();
            if !ops.iter().any(|o| o.0 == op.0) {
                ops.push(op);
                ops.sort_by_key(|o| o.0);
            }
        }
        Ok(())
    }

    /// Slice 3e-1 (#483): the masked claims' uids, with their masks' times: raw.db's, those a purge
    /// kept, and once `widen` has read them, those only the backups hold.
    pub(crate) fn masked(&self) -> &std::collections::HashMap<String, i64> {
        &self.forgotten.masked
    }

    /// The uids of the claims made from forgotten records, with their jobs: raw.db's, and once
    /// `widen` has read them, those only the backups hold.
    pub(crate) fn uids(&self) -> &std::collections::HashMap<String, String> {
        &self.forgotten.uids
    }
}

/// What reads each op of a store or a segment: its kind, device and op seq, time, batch (none for
/// a segment line without one) and body.
pub(crate) type EachOp<'a> =
    dyn FnMut(OpKind, (&str, i64), i64, Option<i64>, &serde_json::Value) + 'a;

/// Each op of every copy of raw.db set aside (a restore's partial build aside: it is removed, not
/// read), as `rewrite_copy` reads it. A copy that does not open, or an op that does not parse, is
/// passed over: `purge_copies` removes the copy then.
fn each_copy_op(home: &Path, f: &mut EachOp) -> Result<()> {
    for copy in copies_set_aside(home)? {
        if copy.ends_with("raw.db.restoring") {
            continue;
        }
        let read = |f: &mut EachOp| -> Result<()> {
            let conn =
                Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            conn.busy_timeout(Duration::ZERO)?;
            let mut st = conn.prepare("SELECT device, op_seq, type, ts, body, batch FROM ops")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                let (device, kind, body): (String, String, String) =
                    (r.get(0)?, r.get(2)?, r.get(4)?);
                if let (Some(kind), Ok(body)) = (
                    OpKind::from_name(&kind),
                    serde_json::from_str::<serde_json::Value>(&body),
                ) {
                    f(kind, (&device, r.get(1)?), r.get(3)?, r.get(5)?, &body);
                }
            }
            Ok(())
        };
        let _ = read(f);
    }
    Ok(())
}

/// What a window op keeps when a purge takes its text (slice 3d-2): its span and what the
/// consumers read of it as a span, never what the curator wrote or was shown.
const WINDOW_STRUCTURE: [&str; 16] = [
    "outcome",
    "reason",
    "from_seq",
    "from_offset",
    "to_seq",
    "to_offset",
    "covers",
    "elided",
    "shortened",
    "excluded",
    "goals",
    "removed",
    "refused",
    "recurate",
    // Slice 3d-4: the claims of its batch taken for repeating what it showed, and when.
    "copied",
    "copied_at",
];

/// Slice 3d-2: window op `body` without its text: `WINDOW_STRUCTURE` and the job that took it.
pub(crate) fn forgotten_window_body(body: &serde_json::Value, job: &str) -> serde_json::Value {
    let mut kept = serde_json::Map::new();
    for key in WINDOW_STRUCTURE {
        if let Some(v) = body.get(key) {
            kept.insert(key.to_owned(), v.clone());
        }
    }
    kept.insert("forgotten".to_owned(), job.into());
    serde_json::Value::Object(kept)
}

/// Slice 3d-2: turn op `body` without its text: a skip with no fields and no labels (as `fitted`
/// makes an op over the cap, and 3c a Removed row), its records, spans and removals kept, so the
/// phase does not ask for it again.
pub(crate) fn forgotten_turn_body(body: &serde_json::Value, job: &str) -> serde_json::Value {
    let mut b = body.clone();
    if let Some(o) = b.as_object_mut() {
        o.insert("fields".to_owned(), serde_json::json!({}));
        o.insert("skipped".to_owned(), true.into());
        o.insert("agent".to_owned(), "".into());
        o.insert("session".to_owned(), "".into());
        o.insert("repo".to_owned(), serde_json::Value::Null);
        o.insert("forgotten".to_owned(), job.into());
    }
    b
}

/// Slice 3e-1: a row's stored body (`enc` as stored) with each of `ranges` masked (`masked`), stored
/// as `compress_through` would store it: none when it held none of them.
fn masked_row(
    body: Vec<u8>,
    enc: &str,
    ranges: &[(i64, i64)],
) -> Result<Option<(Vec<u8>, &'static str)>> {
    let plain = if enc == "zstd" { unzstd(&body)? } else { body };
    let text = String::from_utf8_lossy(&plain).into_owned();
    let now = ranges.iter().fold(text.clone(), |t, &(offset, length)| {
        masked(&t, offset, length)
    });
    if now == text {
        return Ok(None);
    }
    let z = (enc == "zstd")
        .then(|| zstd::bulk::compress(now.as_bytes(), 3))
        .transpose()?
        .filter(|z| z.len() < now.len());
    Ok(Some(match z {
        Some(z) => (z, "zstd"),
        None => (now.into_bytes(), "plain"),
    }))
}

/// Slice 3e-1: each record a rescan masked, by its device and seq: the time of its row when raw.db
/// holds it (a backup line or a copy's row of another record at a seq used again is not it), and
/// its ranges (`Raw::masks`).
pub(crate) type Masks = std::collections::HashMap<(String, i64), (Option<i64>, Vec<(i64, i64)>)>;

/// Slice 3d-1: the claims made from forgotten records whose ops hold text (`Raw::record_claims`).
#[derive(Debug, Default)]
pub(crate) struct RecordClaims {
    /// Each uid, with the record job whose forget op tombstones it (or will).
    pub(crate) uids: std::collections::HashMap<String, String>,
    /// The ops of each uid that hold text: its claim ops and Correction ops.
    pub(crate) keys: std::collections::HashMap<String, Vec<(String, i64)>>,
    /// Slice 3e-1: the claim ops made from a masked record's text before its mask, each by its
    /// device and op seq, and their uids with the time of the mask (`Forgotten::masked`).
    pub(crate) masked: Vec<(String, i64)>,
    pub(crate) masked_uids: std::collections::HashMap<String, i64>,
    /// Slice 3d-4: the text of each claim that quotes a forgotten record, by uid (`claim_text`):
    /// what a window that named it showed its curator, read before its purge takes it.
    pub(crate) texts: Wordings,
}

/// Slice 3d-4: each wording of a claim, by uid, with when it was written: a window was shown those
/// written before it, not a later correction or derivation (Codex on #481).
pub(crate) type Wordings = std::collections::HashMap<String, Vec<(i64, String)>>;

/// Adds `text`, written at `ts`, to `wordings` once.
fn word(wordings: &mut Vec<(i64, String)>, ts: i64, text: String) {
    if !wordings.iter().any(|(t, w)| *t == ts && *w == text) {
        wordings.push((ts, text));
    }
}

/// The entry of the first forgotten record claim op `body` quotes, else of the first masked one
/// (slice 3e-1) the op, written at `ts`, came before the mask of (`SHOWN_MARGIN_MS`): what takes
/// its text. A forget comes first: it tombstones the uid.
fn quoted_entry<'a>(
    forgotten: &'a Forgotten,
    ts: i64,
    body: &serde_json::Value,
) -> Option<&'a (i64, String, Option<i64>)> {
    // Each masked record it quotes by its own mask, not the first one's (Codex on #480).
    let quoted = |masked: bool| {
        items(&body["evidence"]).iter().find_map(|e| {
            let r = forgotten.entry(e["device"].as_str()?, e["seq"].as_i64()?)?;
            let takes = if masked {
                masked_after(r, ts)
            } else {
                r.1 != MASK_JOB
            };
            takes.then_some(r)
        })
    };
    quoted(false).or_else(|| quoted(true))
}

/// Slice 3e-1: whether `r` is a masked record whose mask came after an op written at `ts`
/// (`SHOWN_MARGIN_MS`).
fn masked_after(r: &(i64, String, Option<i64>), ts: i64) -> bool {
    r.1 == MASK_JOB
        && r.2
            .is_none_or(|at| ts <= at.saturating_add(SHOWN_MARGIN_MS))
}

/// Slice 3e-1: the latest mask of the masked records claim op `body`, written at `ts`, quotes and
/// came before: a window that named its uid before then showed the text of one of them, not only
/// the first's (Codex on #480). A mask a restore lost the time of is as late as can be.
fn latest_mask(forgotten: &Forgotten, ts: i64, body: &serde_json::Value) -> i64 {
    items(&body["evidence"])
        .iter()
        .filter_map(|e| forgotten.entry(e["device"].as_str()?, e["seq"].as_i64()?))
        .filter(|r| masked_after(r, ts))
        .map(|r| r.2.unwrap_or(i64::MAX))
        .max()
        .unwrap_or(i64::MAX)
}

/// Slice 3e-1: `uid`'s mask time in `masked`, the later of the one it has and `at`.
fn later(masked: &mut std::collections::HashMap<String, i64>, uid: &str, at: i64) {
    let t = masked.entry(uid.to_owned()).or_insert(at);
    *t = (*t).max(at);
}

/// Each forgotten record's identity (slice 3c-2), with the native session and ambiguity its
/// record's `import_identity` has in an export now: none when this store holds no record of it.
pub(crate) type ForgottenOrigins =
    std::collections::HashMap<String, Option<(Option<String>, bool)>>;

/// What a forgotten record's row becomes (slice 3c): a Removed row that keeps its device, seq, ts,
/// source and kind (a live record's identity, `original_metadata`), with no text or label.
const REMOVED_ROW: &str = "type = 'removed', enc = 'plain', body = NULL, original_bytes = NULL,
    agent = NULL, session = NULL, repo = NULL, branch = NULL, head = NULL, gitdir = NULL, cwd = NULL";

/// Forgotten records rewritten in one transaction (slice 3c): a hook waits behind one no longer
/// than behind a page's registration.
pub(crate) const RECORD_PURGE_BATCH: usize = 500;

/// The index the session checks of the denials read (`check_cross_source_forget`, `transcript_cut`,
/// #467), made by the first denial and the purge, as the forget preview makes its own (spec 1.6:
/// raw.db opened alone has none on a session).
fn denied_session_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS denied_records_session ON denied_records(session)",
    )?;
    Ok(())
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

/// The uid whose text an op of `kind` holds (D5): a Claim's as the consumer derives it, a
/// Correction's or an Import's from its body; none for the other kinds.
pub(crate) fn op_uid(kind: OpKind, body: &serde_json::Value) -> Option<String> {
    match kind {
        OpKind::Claim => crate::claims::op_uid(body),
        OpKind::Correction | OpKind::Import => body["uid"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// A Claim or Correction op anchored on a record forget denies, by the record's origin (D1 rule
/// 5), or a Claim, Correction or Import op of a forgotten uid (D5): never recorded. Window and
/// turn ops are fenced by `Reading` (rule 12).
fn forgotten_op(conn: &Connection, kind: OpKind, body: &serde_json::Value) -> Result<bool> {
    if let Some(uid) = op_uid(kind, body)
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

/// Slice 3e-4: an op whose text the rules now mask, its device, op seq, body as read and as masked.
pub(crate) type OpText = (String, i64, String, String);

/// Slice 3e-4 (#478): the keys of an op's body whose values oboete gives (identifiers and the
/// words of its own vocabulary), or a record holds verbatim (a quote, which its record's mask
/// covers): no rule masks them in an op. A user's rule for 64 hex digits would otherwise change the
/// uid a correction, a forget or an import names (as `view::gated_with_ids` keeps them), and one
/// for a repository which op is whose. Inside `fields` (a work state's, a turn's) every value is
/// text. A key is matched at any depth outside `fields`: an op field added later under one of these
/// names keeps its value unmasked, so a new field is checked against this list (CodeRabbit on #487).
/// A window's `reason` is one only when oboete set the window aside (`curate::set_aside`, which
/// `oboete recurate` reads): a provider's failure is free text, masked (Codex on #487).
const OP_NAMES: &[&str] = &[
    "agent",
    "candidates",
    "carried",
    "copied",
    "device",
    "dropped",
    // A folder never recorded, by its path as an event's `cwd` label holds it (slice 4a, #489).
    "folder",
    "forgotten",
    "id",
    "job",
    "kind",
    // An exclusion op's level (slice 4a, #489).
    "level",
    "list",
    "lowered",
    "outcome",
    "provider",
    "quote",
    "recipe",
    "repo",
    "scope",
    "session",
    "source",
    "source_id",
    "speaker",
    "status",
    "supersedes",
    "type",
    "uid",
];

/// Slice 3e-4 (#478): an op's `body` of `kind` with each string `rules` find something in masked,
/// one `*` for each character a finding touches, but the values of `OP_NAMES`; none when they find
/// nothing or the body does not parse. A forget's op (a uid and a job) and a migration's (a
/// checkpoint and hashes) hold no text. Unlike a record's masks, which keep its bytes for the
/// ledger's offsets, these never lengthen a text: a correction or a claim stays under its
/// character cap (Codex on #487).
pub(crate) fn masked_op_text(
    kind: OpKind,
    body: &str,
    rules: &crate::redact::Rules,
) -> Option<String> {
    fn walk(v: &mut serde_json::Value, rules: &crate::redact::Rules, names: bool) -> bool {
        match v {
            serde_json::Value::String(text) => {
                let found = crate::redact::ranges(text, rules);
                let now: String = text
                    .char_indices()
                    .map(|(at, c)| {
                        let end = at + c.len_utf8();
                        match found.iter().any(|&(start, stop)| at < stop && start < end) {
                            true => '*',
                            false => c,
                        }
                    })
                    .collect();
                let changed = now != *text;
                *text = now;
                changed
            }
            serde_json::Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| walk(item, rules, names) | changed),
            serde_json::Value::Object(map) => map.iter_mut().fold(false, |changed, (key, item)| {
                // Any depth: see `OP_NAMES` before naming a new op field.
                let name = OP_NAMES.contains(&key.as_str())
                    || key == "reason" && item.as_str().is_some_and(crate::curate::set_aside);
                if names && name {
                    changed
                } else {
                    walk(item, rules, names && key != "fields") | changed
                }
            }),
            _ => false,
        }
    }
    if matches!(kind, OpKind::Forget | OpKind::Migration) {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_str(body).ok()?;
    walk(&mut v, rules, true).then(|| v.to_string())
}

/// `body` with the bytes from `offset` for `length` replaced by `*`, widened to whole characters
/// and cut at the body's end.
pub(crate) fn masked(body: &str, offset: i64, length: i64) -> String {
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

/// Slice 3e-2: `ops` split, in order, into appends within one batch's caps (`MAX_BATCH_OPS`,
/// `MAX_BATCH_BYTES`) for ops written apart (Codex on #485); `append_ops` checks each op's own cap.
pub(crate) fn append_batches(
    ops: &[(OpKind, serde_json::Value)],
) -> Vec<&[(OpKind, serde_json::Value)]> {
    let mut batches = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (i, (_, body)) in ops.iter().enumerate() {
        let size = body.to_string().len();
        if i > start && (i - start == MAX_BATCH_OPS || bytes + size > MAX_BATCH_BYTES) {
            batches.push(&ops[start..i]);
            (start, bytes) = (i, 0);
        }
        bytes += size;
    }
    if start < ops.len() {
        batches.push(&ops[start..]);
    }
    batches
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
/// Owner decision 43: ops whose text was gated with the redaction rules turned off were gated by
/// none, so the scan of the op text is owed again under whatever rules there are when it runs,
/// also when their version is the one the last scan recorded; a scan begun before records nothing
/// (`Raw::ops_text_scanned`). In the transaction that appends them, so no stop between the two
/// leaves them counted as scanned.
fn owe_op_text_scan(tx: &Connection) -> Result<()> {
    tx.execute(
        "DELETE FROM meta WHERE key IN ('ops_text_scanned', 'ops_text_scanning')",
        [],
    )?;
    Ok(())
}

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

    fn format_in(raw: &Raw) -> i64 {
        raw.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// docs/store-format.md: a raw.db a newer oboete stamped is refused by the writable open and
    /// the read-only one, and neither changes a byte of it (a reader may leave an empty WAL).
    #[test]
    fn a_raw_db_a_newer_oboete_wrote_is_refused_and_left_as_it_was() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = open(p).unwrap();
        raw.append(&test_event(r#"{"prompt":"kept as it was"}"#))
            .unwrap();
        // Without a column this binary's migration adds back, and in rollback-journal mode, which
        // this binary's open sets to WAL: a refused open changes neither (Codex on #503).
        raw.conn
            .execute_batch(&format!(
                "ALTER TABLE import_origins DROP COLUMN ambiguous;
                 PRAGMA user_version = {};",
                FORMAT + 1
            ))
            .unwrap();
        drop(raw);
        let mode = || {
            Connection::open(p.join("raw.db"))
                .unwrap()
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap()
        };
        Connection::open(p.join("raw.db"))
            .unwrap()
            .execute_batch("PRAGMA journal_mode = DELETE")
            .unwrap();
        assert_eq!(mode(), "delete");
        let bytes = |name: &str| std::fs::read(p.join(name)).ok();
        let before = bytes("raw.db");
        for refused in [open(p).err(), read_only(p).err()] {
            let e = refused.expect("refused");
            let found = e.downcast_ref::<NewerFormat>().map(|n| n.found);
            assert_eq!(found, Some(FORMAT + 1), "{e:#}");
            assert!(e.to_string().contains("update oboete"), "{e:#}");
        }
        assert_eq!(bytes("raw.db"), before);
        assert!(bytes("raw.db-wal").is_none_or(|wal| wal.is_empty()));
        assert_eq!(mode(), "delete");
    }

    /// docs/store-format.md: a raw.db from before the stamp gets it at its first writable open,
    /// and an open of a stamped one writes nothing for it.
    #[test]
    fn a_raw_db_from_before_the_stamp_gets_it_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let raw = open(p).unwrap();
        assert_eq!(format_in(&raw), FORMAT);
        raw.conn.execute_batch("PRAGMA user_version = 0").unwrap();
        drop(raw);
        let mut changed = Vec::new();
        let raw = open_report(p, None, &mut |stage| {
            changed.push(stage);
            Ok(())
        })
        .unwrap();
        assert_eq!((format_in(&raw), changed), (FORMAT, vec!["stores_changed"]));
        assert!(!raw.newer_format());
        let mut changed = Vec::new();
        let again = open_report(p, None, &mut |stage| {
            changed.push(stage);
            Ok(())
        })
        .unwrap();
        assert_eq!((format_in(&again), changed), (FORMAT, vec![]));
        // A newer binary stamps it while this handle is open: the handle says so, and appends
        // neither an event nor an op (CodeRabbit on #503).
        again
            .conn
            .execute_batch(&format!("PRAGMA user_version = {}", FORMAT + 1))
            .unwrap();
        assert!(raw.newer_format());
        let mut raw = raw;
        let refused = |e: anyhow::Error| e.is::<NewerFormat>();
        assert!(refused(
            raw.append(&test_event("after the stamp")).unwrap_err()
        ));
        let op = (
            OpKind::Correction,
            serde_json::json!({"uid": "u", "status": "rejected"}),
        );
        assert!(refused(raw.append_ops(&[op]).unwrap_err()));
    }

    /// CodeRabbit on #503: a newer oboete that stamps raw.db while an open migrates it is refused
    /// before the open returns its handle.
    #[test]
    fn an_open_during_which_a_newer_oboete_stamps_raw_db_refuses_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(open(p).unwrap());
        BEFORE_OPEN_RETURNS.set(Some(|path| {
            Connection::open(path)
                .unwrap()
                .execute_batch(&format!("PRAGMA user_version = {}", FORMAT + 1))
                .unwrap();
        }));
        let e = open(p).err().expect("refused");
        assert!(e.is::<NewerFormat>(), "{e:#}");
    }

    /// Codex on #458: a copy's sidecars are named on its path's own bytes, which need not be
    /// UTF-8.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_under_a_path_that_is_not_utf8_goes_with_its_sidecars() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(std::ffi::OsStr::from_bytes(b"home-\xff"));
        std::fs::create_dir(&home).unwrap();
        let files = [
            "raw.db.quarantined-1",
            "raw.db.quarantined-1-wal",
            "raw.db.quarantined-1-shm",
            "raw.db.quarantined-1-journal",
        ]
        .map(|name| home.join(name));
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        remove_with_sidecars(&files[0]).unwrap();
        for f in &files {
            assert!(!f.exists(), "{f:?}");
        }
    }

    /// G22: a session's start reads the skipped windows through `ops_window_skips`, whose
    /// condition `SKIP_WINDOWS` repeats, and finds the ones curation writes.
    #[test]
    fn skipped_windows_are_read_through_their_index() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let plan: Vec<String> = raw
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {SKIP_WINDOWS}"))
            .unwrap()
            .query_map(params!["d", 0, 10], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|p| p.contains("USING INDEX ops_window_skips")),
            "{plan:?}"
        );
        let window = |more: serde_json::Value| {
            let mut op = serde_json::json!({"from_seq": 1, "to_seq": 1, "outcome": "curated"});
            op.as_object_mut()
                .unwrap()
                .extend(more.as_object().unwrap().clone());
            (OpKind::Window, op)
        };
        raw.append_ops(&[
            window(serde_json::json!({})),
            window(serde_json::json!({"outcome": "skipped", "reason": "x"})),
            window(serde_json::json!({"refused": true})),
            window(serde_json::json!({"recurate": true})),
            window(serde_json::json!({"refused": false, "recurate": false, "reason": "skipped"})),
        ])
        .unwrap();
        let device = raw.device().to_owned();
        let seqs: Vec<i64> = raw
            .skip_window_ops_after(&device, 0, 10)
            .unwrap()
            .iter()
            .map(|o| o.op_seq)
            .collect();
        assert_eq!(seqs, [2, 3, 4]);
    }

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
                support: Vec::new(),
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

    /// Slice 3e-2 (Codex on #485): ops written apart go in appends within one batch's caps, the
    /// bytes' as well as the count's, in order.
    #[test]
    fn append_batches_keep_each_append_within_its_caps() {
        let op = |n: usize| (OpKind::Claim, serde_json::json!({ "body": "x".repeat(n) }));
        let big: Vec<_> = (0..80).map(|_| op(60_000)).collect();
        let batches = append_batches(&big);
        assert_eq!(batches.len(), 2);
        for batch in &batches {
            within_batch_cap(batch).unwrap();
        }
        assert_eq!(batches.concat(), big);
        assert!(append_batches(&[]).is_empty());
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

    /// Slice 3e-4 (CodeRabbit on #487): an exclusion op's `folder` and `level`, slice 4a's, are
    /// names no rule masks, so an excluded folder keeps matching its events' `cwd` labels.
    #[test]
    fn an_exclusion_ops_folder_and_level_are_names() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'acme-[0-9]{6}|capture' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let body = serde_json::json!({
            "folder": "/w/acme-482915", "level": "capture", "undo": false, "clock": 1,
        });
        assert_eq!(
            masked_op_text(OpKind::Exclusion, &body.to_string(), &rules),
            None
        );
        let other = serde_json::json!({"note": "/w/acme-482915"}).to_string();
        assert!(masked_op_text(OpKind::Exclusion, &other, &rules).is_some());
    }

    /// Slice 3e-4 (Codex on #487): a window's `reason` is kept only when oboete set the window
    /// aside, which `oboete recurate --skipped` and `--source` read; a provider's failure is free
    /// text and a rule masks it.
    #[test]
    fn a_windows_reason_is_masked_unless_oboete_set_it_aside() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'acme-[0-9]{6}|excluded|transcript' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        for kept in ["excluded", "imported:transcript"] {
            let body = serde_json::json!({"outcome": "skipped", "reason": kept});
            assert_eq!(
                masked_op_text(OpKind::Window, &body.to_string(), &rules),
                None
            );
        }
        let failed = "every provider failed: groq: key file /w/acme-482915/key.md unreadable";
        let body = serde_json::json!({"outcome": "skipped", "reason": failed});
        let now = masked_op_text(OpKind::Window, &body.to_string(), &rules).unwrap();
        let now: serde_json::Value = serde_json::from_str(&now).unwrap();
        let reason = now["reason"].as_str().unwrap();
        assert!(!reason.contains("acme-482915") && reason.contains("/w/***********/key.md"));
        assert!(!crate::curate::set_aside(reason));
    }

    /// Slice 3e-4 (Codex on #487): a correction at its 1,000-character cap, in characters of
    /// three bytes, keeps its status and stays a correction once a rule masks part of its body:
    /// one `*` for each character.
    #[test]
    fn a_masked_correction_stays_under_its_character_cap() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = '機密[あ-ん]{8}' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let text = format!("機密{}{}", "あ".repeat(8), "い".repeat(990));
        assert_eq!(text.chars().count(), crate::claims::MAX_BODY_CHARS);
        let body = serde_json::json!({
            "uid": "a".repeat(64), "status": "retracted", "body": text,
            "anchor": {"device": "d", "seq": 1},
        });
        let now = masked_op_text(OpKind::Correction, &body.to_string(), &rules).unwrap();
        let op: crate::claims::CorrectionOp = serde_json::from_str(&now).unwrap();
        assert_eq!(op.fault(), None);
        assert_eq!(op.status.as_deref(), Some("retracted"));
        let masked = op.body.unwrap();
        assert_eq!(masked.chars().count(), crate::claims::MAX_BODY_CHARS);
        assert!(masked.starts_with(&"*".repeat(10)) && !masked.contains("機密"));
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

    /// Codex on #489: a folder excluded under one spelling covers another where the platform's file
    /// systems ignore letter case (Windows, macOS), and only there.
    #[test]
    fn a_folder_covers_another_spelling_of_itself_only_where_case_is_ignored() {
        let (stored, event) = if cfg!(windows) {
            (r"C:\Work\Private", r"c:\work\private\sub")
        } else {
            ("/Users/x/Work/Private", "/users/x/work/private/sub")
        };
        let list = CaptureExclusions {
            folders: [stored.to_owned()].into(),
            ..Default::default()
        };
        assert_eq!(
            list.covers(None, Some(event)),
            cfg!(any(windows, target_os = "macos"))
        );
    }

    /// Slice 4a (spec 6.1): what is never recorded is never sent either, and each undo takes its
    /// entry out of its own list only: a capture undo leaves `oboete exclude`'s exclusion, and an
    /// undo of that leaves what is never recorded. A folder is never a repository to send.
    #[test]
    fn what_is_never_recorded_and_what_is_not_sent_keep_their_own_undo() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude("a", false).unwrap();
        raw.exclude_capture(("repo", "a"), false).unwrap();
        raw.exclude_capture(("repo", "b"), false).unwrap();
        raw.exclude_capture(("folder", "/x/y"), false).unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["a", "b"]);
        raw.exclude_capture(("repo", "a"), true).unwrap();
        raw.exclude("b", true).unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["a", "b"]);
        let never = raw.capture_exclusions().unwrap();
        assert_eq!(
            (Vec::from_iter(never.repos), Vec::from_iter(never.folders)),
            (vec!["b".to_owned()], vec!["/x/y".to_owned()])
        );
        raw.exclude_capture(("folder", "/x/y"), true).unwrap();
        raw.exclude_capture(("repo", "b"), true).unwrap();
        assert_eq!(
            raw.capture_exclusions().unwrap(),
            CaptureExclusions::default()
        );
        assert_eq!(raw.exclusions().unwrap(), ["a"]);
    }

    /// Codex on #491 (owner, 2026-10-13): a repository stored as given is matched in the hook's
    /// write transaction as the rules now read it too, so an event the hook labelled under a rule
    /// that masks the name is refused there; an append with no settings compares the list as stored.
    #[test]
    fn a_hooks_append_matches_the_list_as_the_rules_now_read_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let given = "github.com/o/acme-482915";
        raw.exclude_capture(("repo", given), false).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let labelled = Event {
            repo: Some(crate::capture::exclusion_label("repo", given, &settings)),
            ..test_event("After the rule.")
        };
        assert_ne!(labelled.repo.as_deref(), Some(given));
        let elsewhere = ("github.com/o/open", None);
        assert_eq!(
            raw.append_captured(&labelled, &[], &settings, elsewhere)
                .unwrap(),
            None
        );
        assert!(
            raw.append_with_ledger(&labelled, &[], "")
                .unwrap()
                .is_some()
        );
    }

    /// Security review of #489: the hook's write transaction also asks about the folder as the hook
    /// read it, so an exclusion committed after the hook's check refuses an event below it though a
    /// rule that takes the folder's separator keeps the labels from matching.
    #[test]
    fn a_hooks_append_matches_the_folder_as_the_hook_read_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let below = "/x/private/sub";
        let labelled = Event {
            cwd: Some(crate::capture::exclusion_label("cwd", below, &settings)),
            ..test_event("Below the folder.")
        };
        let read = ("github.com/o/r", Some(below));
        assert_eq!(
            raw.append_captured(&labelled, &[], &settings, read)
                .unwrap(),
            None
        );
        let elsewhere = ("github.com/o/r", Some("/x/open/sub"));
        assert!(
            raw.append_captured(&labelled, &[], &settings, elsewhere)
                .unwrap()
                .is_some()
        );
    }

    /// Security review of #489: a work state write the MCP server makes asks in its transaction
    /// what the server's own check asks: the folder as read, and the labels against the list as
    /// given and as the rules now read it.
    #[test]
    fn a_work_state_write_asks_in_its_transaction_what_its_writer_asks() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let given = "github.com/o/acme-482915";
        raw.exclude_capture(("repo", given), false).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let repo = crate::capture::exclusion_label("repo", given, &settings);
        let fields = serde_json::json!({ "task": "Plan." });
        let fields = fields.as_object().unwrap();
        let read = (given, Some("/w/acme"));
        let refused = raw.work_state_under(&repo, "plan", fields, "/w/[x]", &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        // The list as the rules now read it, with the place read elsewhere.
        let elsewhere = ("github.com/o/open", Some("/w/open"));
        let refused = raw.work_state_under(&repo, "plan", fields, "/w/open", &settings, elsewhere);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        assert!(raw.work_state(&repo, "plan", fields, None).is_ok());
        // A folder whose separator a rule takes: only the place as read matches it.
        raw.exclude_capture(("folder", "/w/private"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let cwd = crate::capture::exclusion_label("cwd", "/w/private/sub", &settings);
        let read = ("github.com/o/open", Some("/w/private/sub"));
        let refused =
            raw.work_state_under("github.com/o/open", "plan", fields, &cwd, &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        // Another folder the rules read as an excluded one: only the list as read now matches it,
        // as the hook's check reads it.
        raw.exclude_capture(("folder", "/v/acme-111111"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let cwd = crate::capture::exclusion_label("cwd", "/v/acme-222222", &settings);
        let read = ("github.com/o/open", Some("/v/acme-222222"));
        let refused =
            raw.work_state_under("github.com/o/open", "plan", fields, &cwd, &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
    }

    /// Codex on #489: a repository never recorded under the label a rule now makes of it keeps its
    /// records from before the rule, which hold the label unmasked, from every provider too.
    #[test]
    fn records_from_before_a_rule_are_not_sent_once_their_repository_is_never_recorded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let secret = "github.com/o/acme-482915";
        raw.append(&Event {
            repo: Some(secret.into()),
            session: "old".into(),
            ..test_event("Old work.")
        })
        .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let now = crate::capture::exclusion_label("repo", secret, &settings);
        assert_ne!(now, secret);
        raw.exclude_capture(("repo", &now), false).unwrap();
        let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live).unwrap();
        assert!(reading.excluded.contains("claude\0old"), "{reading:?}");
        assert_eq!(raw.exclusions().unwrap(), [now]);
    }

    /// Slice 4a: an event of a repository or folder never recorded is refused by every append that
    /// writes one, once the exclusion is committed: a hook's, a kept call's written back, an
    /// import's (whose batch records the rest); a folder covers what is in it, component by
    /// component, and an undo records what comes after it.
    #[test]
    fn an_event_never_recorded_is_refused_by_every_append() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let at = |repo: &str, cwd: &str| Event {
            repo: Some(repo.into()),
            cwd: Some(cwd.into()),
            ..test_event("canary-4a")
        };
        raw.exclude_capture(("repo", "github.com/o/secret"), false)
            .unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        for e in [
            at("github.com/o/secret", "/w/secret"),
            at("/x/private", "/x/private"),
            at("/x/private/sub", "/x/private/sub/dir"),
        ] {
            assert_eq!(raw.append_with_ledger(&e, &[], "").unwrap(), None);
        }
        assert!(
            raw.append_with_ledger(&at("/x/privateer", "/x/privateer"), &[], "")
                .unwrap()
                .is_some()
        );
        let kept = |e: Event| crate::capture::Captured {
            event: e,
            ledger: Vec::new(),
        };
        assert!(
            raw.append_kept(
                &[kept(at("github.com/o/secret", "/w/secret"))],
                "",
                None,
                &Default::default()
            )
            .unwrap()
        );
        let (mut secret, mut open) = (imported("canary-4a"), imported("canary-4a"));
        secret.event.repo = Some("github.com/o/secret".into());
        open.event.repo = Some("github.com/o/open".into());
        assert_eq!(
            raw.append_imported(&[secret, open], "", None)
                .unwrap()
                .len(),
            1
        );
        raw.exclude_capture(("repo", "github.com/o/secret"), true)
            .unwrap();
        assert!(
            raw.append_with_ledger(&at("github.com/o/secret", "/w/secret"), &[], "")
                .unwrap()
                .is_some()
        );
        let repos: Vec<Option<String>> = raw
            .after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                Item::Event(e) => Some(e.repo),
                _ => None,
            })
            .collect();
        assert_eq!(
            repos,
            [
                Some("/x/privateer".to_owned()),
                Some("github.com/o/open".to_owned()),
                Some("github.com/o/secret".to_owned())
            ]
        );
    }

    /// Slice 4a (Codex on #489): a work state write is refused inside its transaction once its
    /// repository, the repository of the claude-mem project an imported list is filed under, or
    /// the folder it is written from is never recorded; elsewhere it is kept, and a list written
    /// here is matched by its own key alone.
    #[test]
    fn a_work_state_write_is_refused_in_its_transaction_where_nothing_is_recorded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let task = serde_json::json!({ "task": "canary-4a" });
        let task = task.as_object().unwrap();
        raw.exclude_capture(("repo", "github.com/o/secret"), false)
            .unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        let refused = |written: Result<Vec<i64>>| written.unwrap_err().is::<NeverRecorded>();
        let imported = |repo: &str| {
            let op = serde_json::json!({"repo": repo, "list": "plan", "fields": task,
                "clock": 1, "at": 1, "source": "claude-mem", "source_id": "w1"});
            vec![(OpKind::WorkState, op)]
        };
        let written = |raw: &mut Raw, repo: &str, cwd: Option<&str>| {
            raw.work_state(repo, "plan", task, cwd).map(|seq| vec![seq])
        };
        assert!(refused(written(&mut raw, "github.com/o/secret", None)));
        assert!(refused(written(
            &mut raw,
            "github.com/o/open",
            Some("/x/private/app")
        )));
        assert!(refused(raw.append_ops(&imported("claude-mem:secret/wt"))));
        assert!(written(&mut raw, "github.com/o/open", Some("/x/privateer")).is_ok());
        assert!(raw.append_ops(&imported("claude-mem:open")).is_ok());
        // A list written here by its own key alone, though that key has the shape of a claude-mem
        // project's (`ssh://claude-mem:secret/x`).
        assert!(written(&mut raw, "claude-mem:secret/x", None).is_ok());
        let lists = raw
            .ops_after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::WorkState)
            .count();
        assert_eq!(lists, 3);
    }

    /// Slice 4a (Codex on #489): an imported list is refused inside its transaction also when its
    /// project's name, labelled under the import's settings, is what an exclusion committed during
    /// the import stored: a rule masks part of that name.
    #[test]
    fn an_imported_list_is_refused_by_an_exclusion_its_rules_mask() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'secret-[0-9]{4}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let mut raw = open(home.path()).unwrap();
        let stored = crate::capture::exclusion_label("repo", "github.com/o/secret-1234", &settings);
        assert!(!stored.contains("secret-1234"), "{stored}");
        raw.exclude_capture(("repo", &stored), false).unwrap();
        let op = serde_json::json!({"repo": "claude-mem:secret-1234", "list": "plan",
            "fields": {"task": "canary-4a"}, "clock": 1, "at": 1, "source": "claude-mem",
            "source_id": "w1"});
        let ops = [(OpKind::WorkState, op)];
        let refused = raw.append_imported_ops(&ops, &settings).unwrap_err();
        assert!(refused.is::<NeverRecorded>(), "{refused}");
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
        raw.work_state("r", "release", &phase("one"), None).unwrap();
        raw.work_state("r", "release", &phase("two"), None).unwrap();
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
        other
            .work_state("r", "release", &phase("three"), None)
            .unwrap();
        other
            .work_state("s", "release", &phase("elsewhere"), None)
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

    /// Owner decision 43 (Codex on #502): a page of op text read under rules changed since (the
    /// switch turned off, say) is not masked: under the dispatch hold a save takes too, the rules
    /// are checked again, and nothing more is written.
    #[test]
    fn a_page_masked_after_the_rules_changed_is_left_as_it_was() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(&config, rule).unwrap();
        let mut raw = open(home.path()).unwrap();
        let body = serde_json::json!({"text": "deploy acme-482915"});
        raw.append_ops(&[(OpKind::Claim, body.clone())]).unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let page = raw.op_text_masks(&rules, None, 10).unwrap();
        assert_eq!(page.len(), 1);
        let off = rule.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        std::fs::write(&config, off).unwrap();
        assert_eq!(raw.mask_op_text(&page, &rules).unwrap(), None);
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(ops[0].body, body);
    }

    /// Owner decision 43 (Codex's security review of the switch): a scan whose mark an append
    /// under rules turned off took away records nothing, also when a scan begun after has put a
    /// mark of its own there, which that later scan alone records by.
    #[test]
    fn a_scan_records_only_under_its_own_mark() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let off = crate::redact::Rules::new(
            &crate::config::parse_capture(Some("[redaction]\nenabled = false\n"))
                .unwrap()
                .redaction,
        )
        .unwrap();
        let first = raw.op_text_scan_begun().unwrap();
        raw.gates_with(&off);
        raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "deploy notes"}))])
            .unwrap();
        let second = raw.op_text_scan_begun().unwrap();
        assert!(!raw.ops_text_scanned("v", &first).unwrap());
        assert!(raw.ops_text_owed("v").unwrap());
        assert!(raw.ops_text_scanned("v", &second).unwrap());
        assert!(!raw.ops_text_owed("v").unwrap());
    }

    /// Owner decision 43 (Codex's security review of the switch): a handle whose ops' text was
    /// gated with the rules off owes the op text scan in each append's own transaction, the
    /// fenced one of curation too; a handle gated with rules on owes nothing.
    #[test]
    fn an_append_gated_off_owes_the_scan_in_its_transaction() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let rules = |toml: &str| {
            crate::redact::Rules::new(&crate::config::parse_capture(Some(toml)).unwrap().redaction)
                .unwrap()
        };
        let claim = || [(OpKind::Claim, serde_json::json!({"text": "deploy notes"}))];
        let scanned = |raw: &Raw| {
            let mark = raw.op_text_scan_begun().unwrap();
            assert!(raw.ops_text_scanned("v", &mark).unwrap());
        };
        scanned(&raw);
        raw.gates_with(&rules("[redaction]\nenabled = true\n"));
        raw.append_ops(&claim()).unwrap();
        assert!(!raw.ops_text_owed("v").unwrap());
        raw.gates_with(&rules("[redaction]\nenabled = false\n"));
        raw.append_ops(&claim()).unwrap();
        assert!(raw.ops_text_owed("v").unwrap());
        scanned(&raw);
        let fence = raw.fence_count().unwrap();
        raw.append_ops_fenced(&claim(), fence).unwrap();
        assert!(raw.ops_text_owed("v").unwrap());
    }

    /// Codex's security review of #489: a repository label masked under a rule removed since
    /// cannot be read again, so its records are held back from sending once a repository it may
    /// be is never recorded; one of another owner leaves them be.
    #[test]
    fn a_label_masked_under_a_rule_removed_since_is_held_back_when_it_may_be_excluded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let masked = format!("github.com/o/{}", crate::redact::MASK);
        let seq = raw
            .append(&Event {
                repo: Some(masked.clone()),
                ..test_event("deploy notes")
            })
            .unwrap();
        raw.exclude_capture(("repo", "github.com/x/other"), false)
            .unwrap();
        assert!(!raw.exclusions_now().unwrap().contains(&masked));
        raw.exclude_capture(("repo", "github.com/o/acme-482915"), false)
            .unwrap();
        let list = raw.exclusions_now().unwrap();
        assert!(list.contains(&masked), "{list:?}");
        let device = raw.device().to_owned();
        let key = raw.session_key(&device, seq).unwrap().unwrap();
        assert!(raw.sessions_in(&list).unwrap().contains(&key));
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

    /// Slice 3d-4: a window that showed a masked record and a forgotten one is the forget's, which
    /// takes the claims it wrote too, whichever comes first in its span.
    #[test]
    fn a_window_that_showed_a_masked_and_a_forgotten_record_is_the_forgets() {
        let forgotten = Forgotten::of([("d".to_owned(), 2)], std::iter::empty())
            .with_masks(vec![("d".to_owned(), 1, Some(100))]);
        let judge = Judge {
            forgotten,
            ..Judge::default()
        };
        let body = serde_json::json!({"outcome": "curated", "from_seq": 1, "to_seq": 2});
        assert_eq!(judge.window("d", 50, &body).as_deref(), Some(""));
    }
}
