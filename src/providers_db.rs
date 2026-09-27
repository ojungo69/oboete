//! Provider state beside raw.db and knowledge.db (docs/milestone-3-plan.md D5): each provider's
//! cooldown and the ledger of every call, failed ones too. Device-local and never synced; neither
//! `rebuild` nor a raw.db restore touches it, so a cooldown or a month's spend survives both.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::now_ms;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS provider_state(
  provider TEXT PRIMARY KEY,
  down_until INTEGER NOT NULL DEFAULT 0,  -- unix ms; 0 for none
  fails INTEGER NOT NULL DEFAULT 0,       -- failures in a row that set no cooldown (the breaker)
  backoff INTEGER NOT NULL DEFAULT 0      -- 429s in a row that named no reset
);
-- What left the machine and what it cost: `bytes_out` is the recorded text sent, `detail` a vetted
-- status, error code or retry value, never a provider's error body (issue #91).
CREATE TABLE IF NOT EXISTS provider_calls(
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  provider TEXT NOT NULL,
  role TEXT NOT NULL,                     -- curator, judge, digest
  span TEXT,                              -- what the call was for (a window, a session)
  outcome TEXT NOT NULL,                  -- ok, invalid, error, wait, budget
  ms INTEGER NOT NULL,
  detail TEXT,
  bytes_out INTEGER NOT NULL DEFAULT 0,
  est_tokens INTEGER,                     -- the uncalibrated estimate of what was sent
  prompt_tokens INTEGER,
  completion_tokens INTEGER,
  cached_tokens INTEGER,
  reasoning_tokens INTEGER
);
CREATE INDEX IF NOT EXISTS provider_calls_day ON provider_calls(provider, ts);
";

pub fn open(home: &Path) -> Result<Connection> {
    let path = home.join("providers.db");
    crate::db::private(home, 0o700);
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    crate::db::wal(&conn, "NORMAL")?;
    conn.execute_batch(SCHEMA).context("providers schema")?;
    for file in ["providers.db", "providers.db-wal", "providers.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    Ok(conn)
}

/// Tokens one provider call used, as the provider reported them (None where it did not say).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub prompt: Option<i64>,
    pub completion: Option<i64>,
    pub cached: Option<i64>,
    pub reasoning: Option<i64>,
}

/// One row of `provider_calls`.
pub struct Call<'a> {
    pub provider: &'a str,
    pub role: &'a str,
    pub span: &'a str,
    pub outcome: &'a str,
    pub ms: i64,
    pub detail: Option<&'a str>,
    pub bytes_out: usize,
    pub usage: Usage,
}

pub fn record(conn: &Connection, c: &Call) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_calls(ts, provider, role, span, outcome, ms, detail, bytes_out,
           prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            now_ms(),
            c.provider,
            c.role,
            c.span,
            c.outcome,
            c.ms,
            c.detail,
            c.bytes_out as i64,
            c.usage.prompt,
            c.usage.completion,
            c.usage.cached,
            c.usage.reasoning
        ],
    )?;
    Ok(())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub down_until: i64,
    pub fails: u32,
    pub backoff: u32,
}

pub fn state(conn: &Connection, provider: &str) -> Result<State> {
    Ok(conn
        .query_row(
            "SELECT down_until, fails, backoff FROM provider_state WHERE provider=?1",
            [provider],
            |r| {
                Ok(State {
                    down_until: r.get(0)?,
                    fails: r.get(1)?,
                    backoff: r.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

pub fn set_state(conn: &Connection, provider: &str, s: State) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_state(provider, down_until, fails, backoff) VALUES(?1,?2,?3,?4)
         ON CONFLICT(provider) DO UPDATE SET down_until=excluded.down_until,
           fails=excluded.fails, backoff=excluded.backoff",
        params![provider, s.down_until, s.fails, s.backoff],
    )?;
    Ok(())
}

/// Requests sent to `provider` since the last UTC midnight (the per-provider daily budget window).
/// A 429 that was waited out still counts: the budget bounds our requests, not our successes.
pub fn calls_today(conn: &Connection, provider: &str) -> Result<u32> {
    let day_ms: i64 = 86_400_000;
    let midnight = now_ms() / day_ms * day_ms;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM provider_calls WHERE provider=?1 AND ts>=?2
           AND outcome IN ('ok','error','invalid','wait')",
        params![provider, midnight],
        |r| r.get(0),
    )?)
}

/// The newest `n` calls, one line each, for doctor.
pub fn last_calls(conn: &Connection, n: u32) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT provider, role, outcome, ms, COALESCE(detail, '') FROM provider_calls
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([n], |r| {
            Ok(format!(
                "{} {} {} {}ms {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}
