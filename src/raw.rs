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
  type TEXT NOT NULL,          -- 'event' or 'tombstone'
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
-- spec 2.2: rule, where and when, never the value.
CREATE TABLE IF NOT EXISTS ledger (
  device TEXT NOT NULL, seq INTEGER NOT NULL, rule TEXT NOT NULL,
  offset INTEGER NOT NULL, length INTEGER NOT NULL, ts INTEGER NOT NULL, ruleset TEXT NOT NULL
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

pub struct Raw {
    conn: Connection,
    device: String,
}

/// `<home>/raw.db`: WAL, synchronous=FULL (and fullfsync on macOS), 2 s busy timeout.
pub fn open(home: &Path) -> Result<Raw> {
    let path = home.join("raw.db");
    crate::db::private(home, 0o700);
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    crate::db::wal(&conn, "FULL")?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON;")?;
    conn.execute_batch(SCHEMA).context("raw schema")?;
    for file in ["raw.db", "raw.db-wal", "raw.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    crate::db::ensure_device(&conn, &path).context("device id")?;
    let device = conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
        r.get(0)
    })?;
    Ok(Raw { conn, device })
}

impl Raw {
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Append one event as this device's next seq. The write lock taken by `BEGIN IMMEDIATE`
    /// makes reading the last seq and inserting the next one atomic across processes.
    pub fn append(&mut self, e: &Event) -> Result<i64> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE device = ?1",
            [&self.device],
            |r| r.get(0),
        )?;
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
        tx.commit()?;
        Ok(seq)
    }

    /// This device's highest seq, 0 for an empty store.
    pub fn max_seq(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM records WHERE device = ?1",
            [&self.device],
            |r| r.get(0),
        )?)
    }

    /// Up to `limit` records of `device` after `seq`, in seq order.
    pub fn after(&self, device: &str, seq: i64, limit: usize) -> Result<Vec<Record>> {
        let mut st = self.conn.prepare(
            "SELECT device, seq, type, ts, kind, agent, session, repo, branch, head, gitdir, cwd,
                    source, body, original_bytes,
                    target_device, target_seq, target_offset, target_length
             FROM records WHERE device = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = st.query_map(
            params![device, seq, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                let item = if r.get::<_, String>(2)? == "tombstone" {
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
                    let body: Vec<u8> = r.get(13)?;
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
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// A prompt event with this body, for tests of every later consumer.
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
}
