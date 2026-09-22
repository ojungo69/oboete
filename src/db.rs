//! One SQLite file: `<home>/oboete.db` (WAL). Raw events live only until summarized.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions(
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  repo TEXT NOT NULL,
  cwd TEXT,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  last_event_at INTEGER NOT NULL,
  injected_at INTEGER
);
CREATE TABLE IF NOT EXISTS events(
  id INTEGER PRIMARY KEY,
  session_id TEXT NOT NULL,
  event TEXT NOT NULL,
  ts INTEGER NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_session ON events(session_id, id);
CREATE TABLE IF NOT EXISTS observations(
  id INTEGER PRIMARY KEY,
  session_id TEXT NOT NULL,
  repo TEXT NOT NULL,
  ts INTEGER NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  provider TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observations_repo ON observations(repo, ts);
CREATE TABLE IF NOT EXISTS summaries(
  id INTEGER PRIMARY KEY,
  session_id TEXT NOT NULL,
  repo TEXT NOT NULL,
  ts INTEGER NOT NULL,
  body TEXT NOT NULL,
  provider TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS summaries_repo ON summaries(repo, ts);
CREATE TABLE IF NOT EXISTS provider_calls(
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  provider TEXT NOT NULL,
  outcome TEXT NOT NULL,
  ms INTEGER NOT NULL,
  detail TEXT
);
CREATE INDEX IF NOT EXISTS provider_calls_day ON provider_calls(provider, ts);
";

pub fn open(home: &Path) -> Result<Connection> {
    let path = home.join("oboete.db");
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    // Switching a file to WAL takes an exclusive lock that the busy handler does not cover:
    // openers racing on a fresh or pre-WAL file wait for each other here instead.
    let mut tries = 0;
    loop {
        match conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
            Ok(()) => break,
            Err(e) if tries < 50 && e.to_string().contains("locked") => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e).context("journal mode"),
        }
    }
    conn.execute_batch(SCHEMA).context("schema")?;
    // CREATE TABLE IF NOT EXISTS leaves a table from an older build as it was; columns added since
    // are filled in here. ponytail: idempotent column checks; PRAGMA user_version once a migration
    // needs more than ADD COLUMN.
    ensure_column(&mut conn, "sessions", "injected_at", "INTEGER").context("migrate columns")?;
    ensure_fts(&mut conn).context("search index")?;
    Ok(conn)
}

/// The search index (FTS5 trigram: substring matching, CJK by character), built from what
/// observe already stored. The read check keeps the hook path free of write locks; the write
/// transaction re-checks, so concurrent first opens build it once and a killed one leaves
/// nothing behind.
fn ensure_fts(conn: &mut Connection) -> Result<()> {
    if table_exists(conn, "fts")? {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !table_exists(&tx, "fts")? {
        tx.execute_batch(
            "CREATE VIRTUAL TABLE fts USING fts5(
               title, body, doc UNINDEXED, kind UNINDEXED, repo UNINDEXED, ts UNINDEXED,
               tokenize='trigram'
             );
             INSERT INTO fts(title, body, doc, kind, repo, ts)
               SELECT title, body, 'o' || id, kind, repo, ts FROM observations;
             INSERT INTO fts(title, body, doc, kind, repo, ts)
               SELECT '', body, 's' || id, 'summary', repo, ts FROM summaries;",
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The read check keeps the hook path free of write locks; the write transaction re-checks,
/// so hooks that open an old database at the same moment do not race on the ALTER.
fn ensure_column(conn: &mut Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    if has_column(conn, table, column)? {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !has_column(&tx, table, column)? {
        tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    }
    tx.commit()?;
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .any(|c| c.as_deref() == Ok(column));
    Ok(found)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn upsert_session(
    conn: &Connection,
    id: &str,
    agent: &str,
    repo: &str,
    cwd: &str,
    ts: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions(id, agent, repo, cwd, started_at, last_event_at) VALUES(?1,?2,?3,?4,?5,?5)
         ON CONFLICT(id) DO UPDATE SET last_event_at=excluded.last_event_at",
        params![id, agent, repo, cwd, ts],
    )?;
    Ok(())
}

/// Context was handed to this session (Claude/Codex at SessionStart, Grok at its first tool call).
pub fn mark_injected(conn: &Connection, id: &str, ts: i64) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET injected_at=?2 WHERE id=?1",
        params![id, ts],
    )?;
    Ok(())
}

pub fn injected(conn: &Connection, id: &str) -> Result<bool> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT injected_at FROM sessions WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(v.is_some())
}

pub fn end_session(conn: &Connection, id: &str, ts: i64) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET ended_at=?2, last_event_at=?2 WHERE id=?1",
        params![id, ts],
    )?;
    Ok(())
}

pub fn insert_event(
    conn: &Connection,
    session_id: &str,
    event: &str,
    ts: i64,
    payload: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO events(session_id, event, ts, payload) VALUES(?1,?2,?3,?4)",
        params![session_id, event, ts, payload],
    )?;
    Ok(())
}

pub struct PendingSession {
    pub id: String,
    pub agent: String,
    pub repo: String,
    pub last_event_at: i64,
}

/// Sessions with raw events, ended or idle for `settle_ms`, oldest first.
pub fn pending_sessions(
    conn: &Connection,
    now: i64,
    settle_ms: u64,
) -> Result<Vec<PendingSession>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent, s.repo, s.last_event_at FROM sessions s
         WHERE EXISTS (SELECT 1 FROM events e WHERE e.session_id = s.id)
           AND (s.ended_at IS NOT NULL OR s.last_event_at <= ?1)
         ORDER BY s.last_event_at ASC LIMIT 20",
    )?;
    let rows = stmt.query_map(params![now - settle_ms as i64], |r| {
        Ok(PendingSession {
            id: r.get(0)?,
            agent: r.get(1)?,
            repo: r.get(2)?,
            last_event_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub struct RawEvent {
    pub id: i64,
    pub event: String,
    pub payload: String,
}

pub fn session_events(conn: &Connection, session_id: &str) -> Result<Vec<RawEvent>> {
    let mut stmt =
        conn.prepare("SELECT id, event, payload FROM events WHERE session_id=?1 ORDER BY id")?;
    let rows = stmt.query_map(params![session_id], |r| {
        Ok(RawEvent {
            id: r.get(0)?,
            event: r.get(1)?,
            payload: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub struct Observation {
    pub kind: String,
    pub title: String,
    pub body: String,
}

const FTS_INSERT: &str =
    "INSERT INTO fts(title, body, doc, kind, repo, ts) VALUES(?1,?2,?3,?4,?5,?6)";

/// One transaction: store the batch's knowledge (and its search rows) and drop its raw events.
/// Rows carry the session's time (`last_event_at`), not the time they were summarized.
pub fn apply_batch(
    conn: &mut Connection,
    s: &PendingSession,
    provider: &str,
    summary: &str,
    observations: &[Observation],
    last_event_id: i64,
) -> Result<()> {
    let (session_id, repo, ts) = (&s.id, &s.repo, s.last_event_at);
    let tx = conn.transaction()?;
    if !summary.trim().is_empty() {
        tx.execute(
            "INSERT INTO summaries(session_id, repo, ts, body, provider) VALUES(?1,?2,?3,?4,?5)",
            params![session_id, repo, ts, summary, provider],
        )?;
        let doc = format!("s{}", tx.last_insert_rowid());
        tx.execute(FTS_INSERT, params!["", summary, doc, "summary", repo, ts])?;
    }
    for o in observations {
        tx.execute(
            "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![session_id, repo, ts, o.kind, o.title, o.body, provider],
        )?;
        let doc = format!("o{}", tx.last_insert_rowid());
        tx.execute(FTS_INSERT, params![o.title, o.body, doc, o.kind, repo, ts])?;
    }
    tx.execute(
        "DELETE FROM events WHERE session_id=?1 AND id<=?2",
        params![session_id, last_event_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn record_call(
    conn: &Connection,
    provider: &str,
    outcome: &str,
    ms: i64,
    detail: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_calls(ts, provider, outcome, ms, detail) VALUES(?1,?2,?3,?4,?5)",
        params![now_ms(), provider, outcome, ms, detail],
    )?;
    Ok(())
}

/// Requests sent to `provider` since the last UTC midnight (the per-provider daily budget window).
/// A 429 that was waited out still counts: the budget bounds our requests, not our successes.
pub fn calls_today(conn: &Connection, provider: &str) -> Result<u32> {
    let day_ms: i64 = 86_400_000;
    let midnight = now_ms() / day_ms * day_ms;
    let n: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM provider_calls WHERE provider=?1 AND ts>=?2 AND outcome IN ('ok','error','invalid','wait')",
            params![provider, midnight],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_table_from_m0_gains_injected_at() {
        let dir = std::env::temp_dir().join(format!("oboete-db-m0-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Connection::open(dir.join("oboete.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL,
                 cwd TEXT, started_at INTEGER NOT NULL, ended_at INTEGER, last_event_at INTEGER NOT NULL);",
            )
            .unwrap();
        // Several agents' hooks can open the old database at the same moment.
        let openers: Vec<_> = (0..4)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || open(&d).map(drop))
            })
            .collect();
        for h in openers {
            h.join().unwrap().unwrap();
        }
        let conn = open(&dir).unwrap();
        upsert_session(&conn, "s1", "claude", "/r", "/r", 1).unwrap();
        assert!(!injected(&conn, "s1").unwrap());
        mark_injected(&conn, "s1", 2).unwrap();
        assert!(injected(&conn, "s1").unwrap());
        // Reopening a current database is a no-op.
        drop(conn);
        assert!(injected(&open(&dir).unwrap(), "s1").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn waited_429_counts_against_the_daily_budget() {
        let dir = std::env::temp_dir().join(format!("oboete-db-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = open(&dir).unwrap();
        record_call(&conn, "groq", "wait", 1, None).unwrap();
        record_call(&conn, "groq", "ok", 1, None).unwrap();
        record_call(&conn, "groq", "budget", 0, None).unwrap();
        assert_eq!(calls_today(&conn, "groq").unwrap(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
