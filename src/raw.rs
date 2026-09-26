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
-- spec 2.2: rule, where and when, never the value. `field` is where in the record: a JSON
-- pointer into the body (`/output`; `/trigger#key` for a key of that object) or a label column
-- (`cwd`). `offset` is where the mask starts in that field as stored; `length` is the secret's
-- own length; both in bytes. `ts` is when the mask was applied (a replay stamps the event with the
-- fixture's time, not this).
CREATE TABLE IF NOT EXISTS ledger (
  device TEXT NOT NULL, seq INTEGER NOT NULL, field TEXT NOT NULL, rule TEXT NOT NULL,
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
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
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
    Ok(Raw { conn, device })
}

impl Raw {
    pub fn device(&self) -> &str {
        &self.device
    }

    /// SQLite's `quick_check` on raw.db: an error names the first problem it reports.
    pub fn quick_check(&self) -> Result<()> {
        crate::db::quick_check(&self.conn, "raw.db")
    }

    /// Append one event as this device's next seq. The write lock taken by `BEGIN IMMEDIATE`
    /// makes reading the last seq and inserting the next one atomic across processes.
    pub fn append(&mut self, e: &Event) -> Result<i64> {
        self.append_with_ledger(e, &[])
    }

    /// `append`, with the event's redaction ledger rows in the same transaction.
    pub fn append_with_ledger(
        &mut self,
        e: &Event,
        ledger: &[(String, crate::redact::Finding)],
    ) -> Result<i64> {
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
                    crate::redact::ruleset()
                ],
            )?;
        }
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
                    target_device, target_seq, target_offset, target_length, enc
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
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
            let batch: Vec<(i64, Vec<u8>)> = self
                .conn
                .prepare(
                    "SELECT seq, body FROM records WHERE device = ?1 AND seq > ?2 AND seq <= ?3
                       AND type = 'event' AND enc = 'plain' ORDER BY seq LIMIT ?4",
                )?
                .query_map(params![device, from, through, COMPRESS_BATCH as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let Some(&(last, _)) = batch.last() else {
                return Ok(rewritten);
            };
            let mut smaller = Vec::new();
            for (seq, body) in &batch {
                let z = zstd::bulk::compress(body, 3)?;
                if z.len() < body.len() {
                    smaller.push((*seq, z));
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
            from = last;
        }
    }
}

/// Records `compress_through` reads per batch.
const COMPRESS_BATCH: usize = 200;

/// The most a stored body may decompress to. Far above any body capture writes (each string is
/// capped at `capture::MAX_FIELD_BYTES`); it stops a crafted frame, as a restored or synced record
/// could carry, from expanding without bound.
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
        r.append_with_ledger(&test_event("x"), &[("/prompt".into(), f)])
            .unwrap();
        let field: String = r
            .conn
            .query_row("SELECT field FROM ledger", [], |x| x.get(0))
            .unwrap();
        assert_eq!(field, "/prompt");
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
}
