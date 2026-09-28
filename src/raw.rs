//! Design B's record store (docs/spec.md sections 1.6, 2.1, 2.5; docs/milestone-2-plan.md Task 1):
//! `raw.db`, one sequence per device holding events and tombstones. (device, seq) is the only
//! ordering and partition key; session, repo and branch are labels, never an index root.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;

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
  type TEXT NOT NULL,          -- 'window', 'claim', 'correction', 'digest'
  ts INTEGER NOT NULL,         -- unix ms, when it was appended
  body TEXT NOT NULL,          -- JSON, at most MAX_OP_BYTES
  batch INTEGER NOT NULL,      -- the first op_seq of the append it came in: a backup keeps it whole
  PRIMARY KEY (device, op_seq)
);
";

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

/// What an op records (milestone 3 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Window,
    Claim,
    Correction,
    Digest,
}

impl OpKind {
    fn name(self) -> &'static str {
        match self {
            OpKind::Window => "window",
            OpKind::Claim => "claim",
            OpKind::Correction => "correction",
            OpKind::Digest => "digest",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        [Self::Window, Self::Claim, Self::Correction, Self::Digest]
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
    /// The shared hold on `<home>/raw.lock` every open keeps (see `swap_lock`).
    _swap: std::fs::File,
}

/// Whether the home has a raw store: `raw.db`, or `raw.db.restored` alone (a restore stopped
/// mid-swap, which `open` finishes).
pub fn exists(home: &Path) -> bool {
    home.join("raw.db").exists() || home.join("raw.db.restored").exists()
}

/// `<home>/raw.db`: WAL, synchronous=FULL (and fullfsync on macOS), 2 s busy timeout.
pub fn open(home: &Path) -> Result<Raw> {
    let path = home.join("raw.db");
    crate::db::private(home, 0o700);
    let swap = swap_lock(home, false, OPEN_WAIT)?;
    // A restore that stopped after moving the damaged file aside and before renaming the rebuilt
    // one in: `raw.db.restored` is only ever a whole rebuild (it gets that name once its records
    // are committed), so the rename is finished here instead of creating an empty store.
    let restored = home.join("raw.db.restored");
    if !path.exists() && restored.exists() {
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
    crate::db::wal(&conn, "FULL")?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON;")?;
    conn.execute_batch(SCHEMA).context("raw schema")?;
    // A raw.db from before the ledger named its field (milestone 2 Task 1's schema).
    crate::db::ensure_column(&mut conn, "ledger", "field", "TEXT NOT NULL DEFAULT ''")
        .context("migrate ledger")?;
    for file in ["raw.db", "raw.db-wal", "raw.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    crate::db::ensure_device(&conn, &path).context("device id")?;
    let device = conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
        r.get(0)
    })?;
    Ok(Raw {
        conn,
        device,
        _swap: swap,
    })
}

/// How long an open waits for a restore to finish swapping the file, and how long a restore
/// waits for open stores to close. A hook's write fails after its wait (MUST-M16's marker).
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
            Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(if exclusive {
                "raw.db is open elsewhere; try again when agents and workers have stopped"
            } else {
                "raw.db is being restored"
            }),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// The exclusive hold a restore keeps while it swaps raw.db (released when dropped).
pub fn lock_for_swap(home: &Path) -> Result<std::fs::File> {
    swap_lock(home, true, SWAP_WAIT)
}

impl Raw {
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
        let seq = next_seq(&tx, &self.device)?;
        tx.execute(
            "INSERT INTO records(device, seq, type, ts, kind, agent, session, repo, branch, head,
                                 gitdir, cwd, source, body, original_bytes)
             VALUES(?1, ?2, 'event', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                self.device,
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
                    self.device,
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
        tx.commit()?;
        Ok(seq)
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

    /// The first prompt this device recorded in one agent's session, as `after` returns it: the
    /// session's goal for the curator (milestone 3 Task 7). A scan by label, as `turns`.
    pub fn first_prompt(&self, agent: &str, session: &str) -> Result<Option<Event>> {
        let seq: Option<i64> = self.conn.query_row(
            "SELECT MIN(seq) FROM records WHERE device = ?1 AND type = 'event' AND agent = ?2
               AND session = ?3 AND kind = 'prompt'",
            rusqlite::params![self.device, agent, session],
            |r| r.get(0),
        )?;
        let Some(seq) = seq else { return Ok(None) };
        Ok(self
            .after(&self.device, seq - 1, 1)?
            .into_iter()
            .find(|r| r.seq == seq)
            .and_then(|r| match r.item {
                Item::Event(e) => Some(*e),
                _ => None,
            }))
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

    /// This device's newest `limit` event records, the newest first, by their labels alone (no
    /// body): where the curation phase looks for a session whose digest is due (milestone 3 Task
    /// 9). Down the primary key: sessions have no index (spec 1.6).
    pub fn newest_labels(&self, limit: usize) -> Result<Vec<Labels>> {
        let mut st = self.conn.prepare(
            "SELECT agent, session, repo, seq, ts, kind FROM records
             WHERE device = ?1 AND type = 'event' ORDER BY seq DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![self.device, limit as i64], |r| {
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

    /// Typed prompts and replies of `agent`'s `session` on this device strictly between two seqs:
    /// none when a proposal was the session's last turn before a window (milestone 3 Task 8).
    pub fn turns_between(
        &self,
        agent: &str,
        session: &str,
        after: i64,
        before: i64,
    ) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM records WHERE device = ?1 AND seq > ?2 AND seq < ?3
               AND type = 'event' AND agent = ?4 AND session = ?5 AND kind IN ('prompt', 'reply')",
            params![self.device, after, before, agent, session],
            |r| r.get(0),
        )?)
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
                let line = line(r, rows);
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
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut op_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(op_seq), 0) FROM ops WHERE device = ?1",
            [&self.device],
            |r| r.get(0),
        )?;
        let (ts, batch) = (crate::db::now_ms(), op_seq + 1);
        let mut seqs = Vec::with_capacity(bodies.len());
        for (kind, body) in &bodies {
            op_seq += 1;
            tx.execute(
                "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![self.device, op_seq, kind, ts, body, batch],
            )?;
            seqs.push(op_seq);
        }
        tx.commit()?;
        Ok(seqs)
    }

    /// Up to `limit` ops of `device` after `op_seq`, in op_seq order.
    pub fn ops_after(&self, device: &str, op_seq: i64, limit: usize) -> Result<Vec<Op>> {
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

    /// The ops appended with the last window op (not a recuration) that covered this device's
    /// latest event of `agent`'s `session` before seq `before` that has text (a resumed session's
    /// `start` is covered on its own, by a window that holds none of its lines): the session's
    /// previous window,
    /// whose proposals its next window carries (milestone 3 Task 7, D12). Per session, since
    /// sessions interleave: another session's window may come between.
    pub fn previous_window_ops(&self, agent: &str, session: &str, before: i64) -> Result<Vec<Op>> {
        use rusqlite::OptionalExtension;
        // Down the primary key from `before`: the session's latest event is usually close.
        let seq: Option<i64> = self
            .conn
            .query_row(
                "SELECT seq FROM records WHERE device = ?1 AND seq < ?2 AND type = 'event'
                   AND agent = ?3 AND session = ?4 AND kind NOT IN ('start', 'end')
                 ORDER BY seq DESC LIMIT 1",
                params![self.device, before, agent, session],
                |r| r.get(0),
            )
            .optional()?;
        let Some(seq) = seq else {
            return Ok(Vec::new());
        };
        let batch: Option<i64> = self
            .conn
            .query_row(
                "SELECT batch FROM ops WHERE device = ?1 AND type = 'window'
                   AND COALESCE(json_extract(body, '$.recurate'), 0) = 0
                   AND json_extract(body, '$.from_seq') <= ?2
                   AND json_extract(body, '$.to_seq') >= ?2
                 ORDER BY op_seq DESC LIMIT 1",
                params![self.device, seq],
                |r| r.get(0),
            )
            .optional()?;
        let Some(batch) = batch else {
            return Ok(Vec::new());
        };
        Ok(self
            .ops_after(&self.device, batch - 1, 1_000)?
            .into_iter()
            .take_while(|o| o.batch == batch)
            .collect())
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
}

impl Rebuild {
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
        Ok(Self { conn })
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
    /// op past the restored records (their segment was damaged and skipped) goes, with every op
    /// after it: the curation checkpoint must never pass a seq the store does not hold, or the
    /// records that reuse those seqs would never be curated. Returns how many ops went.
    pub fn finish(self) -> Result<usize> {
        let dropped = self.conn.execute(
            "DELETE FROM ops WHERE op_seq >= (
               SELECT MIN(w.op_seq) FROM ops w WHERE w.device = ops.device AND w.type = 'window'
                 AND json_extract(w.body, '$.to_seq') >
                     (SELECT COALESCE(MAX(r.seq), 0) FROM records r WHERE r.device = w.device))",
            [],
        )?;
        self.conn.execute_batch("COMMIT")?;
        self.conn.close().map_err(|(_, e)| e)?;
        Ok(dropped)
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

/// The next seq of `device`, inside a write transaction.
fn next_seq(tx: &rusqlite::Transaction, device: &str) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?)
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
                        (0..50)
                            .map(|n| r.append(&test_event(&format!("{i}-{n}"))).unwrap())
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
}
