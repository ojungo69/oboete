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
  last_event_at INTEGER NOT NULL
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
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
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
}

/// Sessions with raw events, ended or idle for `settle_ms`, oldest first.
pub fn pending_sessions(
    conn: &Connection,
    now: i64,
    settle_ms: u64,
) -> Result<Vec<PendingSession>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent, s.repo FROM sessions s
         WHERE EXISTS (SELECT 1 FROM events e WHERE e.session_id = s.id)
           AND (s.ended_at IS NOT NULL OR s.last_event_at <= ?1)
         ORDER BY s.last_event_at ASC LIMIT 20",
    )?;
    let rows = stmt.query_map(params![now - settle_ms as i64], |r| {
        Ok(PendingSession {
            id: r.get(0)?,
            agent: r.get(1)?,
            repo: r.get(2)?,
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

/// One transaction: store the batch's knowledge and drop its raw events.
pub fn apply_batch(
    conn: &mut Connection,
    session_id: &str,
    repo: &str,
    provider: &str,
    summary: &str,
    observations: &[Observation],
    last_event_id: i64,
) -> Result<()> {
    let ts = now_ms();
    let tx = conn.transaction()?;
    if !summary.trim().is_empty() {
        tx.execute(
            "INSERT INTO summaries(session_id, repo, ts, body, provider) VALUES(?1,?2,?3,?4,?5)",
            params![session_id, repo, ts, summary, provider],
        )?;
    }
    for o in observations {
        tx.execute(
            "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![session_id, repo, ts, o.kind, o.title, o.body, provider],
        )?;
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

/// Calls made to `provider` since the last UTC midnight (the per-provider daily budget window).
pub fn calls_today(conn: &Connection, provider: &str) -> Result<u32> {
    let day_ms: i64 = 86_400_000;
    let midnight = now_ms() / day_ms * day_ms;
    let n: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM provider_calls WHERE provider=?1 AND ts>=?2 AND outcome IN ('ok','error','invalid')",
            params![provider, midnight],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    Ok(n)
}
