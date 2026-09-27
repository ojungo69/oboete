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
  backoff INTEGER NOT NULL DEFAULT 0,     -- 429s in a row that named no reset
  -- What the provider's last answer said is left (Groq's x-ratelimit-* headers), resets in ms.
  tokens_left INTEGER, tokens_reset_at INTEGER, requests_left INTEGER, requests_reset_at INTEGER
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
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    crate::db::wal(&conn, "NORMAL")?;
    conn.execute_batch(SCHEMA).context("providers schema")?;
    // Columns added after the table's first version (milestone 3, Task 4).
    for column in [
        "tokens_left",
        "tokens_reset_at",
        "requests_left",
        "requests_reset_at",
    ] {
        crate::db::ensure_column(&mut conn, "provider_state", column, "INTEGER")?;
    }
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
    /// The uncalibrated estimate of what was sent (`budget::estimate`).
    pub est_tokens: Option<u32>,
    pub usage: Usage,
}

pub fn record(conn: &Connection, c: &Call) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_calls(ts, provider, role, span, outcome, ms, detail, bytes_out,
           est_tokens, prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            now_ms(),
            c.provider,
            c.role,
            c.span,
            c.outcome,
            c.ms,
            c.detail,
            c.bytes_out as i64,
            c.est_tokens,
            c.usage.prompt,
            c.usage.completion,
            c.usage.cached,
            c.usage.reasoning
        ],
    )?;
    Ok(())
}

/// `down_until` of a provider stopped until the owner acts (claude's `credits_required`, spec
/// 3.1): no time ends it, only `oboete resume`.
pub const OWNER_HOLD: i64 = i64::MAX;

/// Providers the chain skips now, with the time they are used again (`OWNER_HOLD`: when the owner
/// acts), for doctor.
pub fn stopped(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT provider, down_until FROM provider_state WHERE down_until > ?1 ORDER BY provider",
    )?;
    let rows = stmt
        .query_map([now_ms()], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Clear `provider`'s cooldown and breaker: the owner says it can be used again. Whether it had
/// any state.
pub fn resume(conn: &Connection, provider: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM provider_state WHERE provider=?1", [provider])? > 0)
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

/// What a provider said is left of its rate limits, and when each resets (Unix ms).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RateLeft {
    pub tokens: Option<i64>,
    pub tokens_reset_at: Option<i64>,
    pub requests: Option<i64>,
    pub requests_reset_at: Option<i64>,
}

pub fn rate(conn: &Connection, provider: &str) -> Result<RateLeft> {
    Ok(conn
        .query_row(
            "SELECT tokens_left, tokens_reset_at, requests_left, requests_reset_at
             FROM provider_state WHERE provider=?1",
            [provider],
            |r| {
                Ok(RateLeft {
                    tokens: r.get(0)?,
                    tokens_reset_at: r.get(1)?,
                    requests: r.get(2)?,
                    requests_reset_at: r.get(3)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

pub fn set_rate(conn: &Connection, provider: &str, r: RateLeft) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_state(provider, tokens_left, tokens_reset_at, requests_left,
           requests_reset_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(provider) DO UPDATE SET tokens_left=excluded.tokens_left,
           tokens_reset_at=excluded.tokens_reset_at, requests_left=excluded.requests_left,
           requests_reset_at=excluded.requests_reset_at",
        params![
            provider,
            r.tokens,
            r.tokens_reset_at,
            r.requests,
            r.requests_reset_at
        ],
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

const DAY_MS: i64 = 86_400_000;

/// Tokens (prompt plus completion) `provider` reported since the last UTC midnight.
pub fn tokens_today(conn: &Connection, provider: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(COALESCE(prompt_tokens, 0) + COALESCE(completion_tokens, 0)), 0)
         FROM provider_calls WHERE provider=?1 AND ts>=?2",
        params![provider, now_ms() / DAY_MS * DAY_MS],
        |r| r.get(0),
    )?)
}

/// Prompt and completion tokens `provider` reported since the first of this month (UTC).
pub fn tokens_this_month(conn: &Connection, provider: &str) -> Result<(i64, i64)> {
    let start = chrono_free_month_start(now_ms());
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0)
         FROM provider_calls WHERE provider=?1 AND ts>=?2",
        params![provider, start],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Since the last UTC midnight and since the first of this month (UTC), for `unmetered`.
pub fn today() -> i64 {
    now_ms() / DAY_MS * DAY_MS
}
pub fn this_month() -> i64 {
    chrono_free_month_start(now_ms())
}

/// What `provider`'s sent calls since `start` may have used beyond the usage they reported: the
/// estimate of each call with no prompt count, and the number of calls with no completion count
/// (a timeout, a dropped connection, an answer without a full usage block). A response with an
/// HTTP error status (`http 429: …`) used none.
pub fn unmetered(conn: &Connection, provider: &str, start: i64) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN prompt_tokens IS NULL THEN est_tokens END), 0),
                COALESCE(SUM(completion_tokens IS NULL), 0)
         FROM provider_calls
         WHERE provider=?1 AND ts>=?2 AND bytes_out > 0 AND outcome IN ('ok', 'invalid', 'error')
           AND COALESCE(detail, '') NOT GLOB 'http [0-9][0-9][0-9]*'",
        params![provider, start],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Unix ms of 00:00 UTC on the first day of `ms`'s month (civil-from-days, H. Hinnant).
fn chrono_free_month_start(ms: i64) -> i64 {
    let days = ms.div_euclid(DAY_MS);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day_of_month = doy - (153 * mp + 2) / 5; // 0-based
    (days - day_of_month) * DAY_MS
}

/// `prompt_tokens / est_tokens` of `provider`'s newest `n` calls that recorded both.
pub fn token_ratios(conn: &Connection, provider: &str, n: u32) -> Result<Vec<f64>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(prompt_tokens AS REAL) / est_tokens FROM provider_calls
         WHERE provider=?1 AND prompt_tokens > 0 AND est_tokens > 0 ORDER BY id DESC LIMIT ?2",
    )?;
    let ratios = stmt
        .query_map(params![provider, n], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ratios)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_month_starts_on_the_first_at_midnight_utc() {
        // 2026-09-27T02:52:53Z -> 2026-09-01T00:00:00Z; 2024-03-01 after a leap day.
        assert_eq!(
            chrono_free_month_start(1_790_477_573_000),
            1_788_220_800_000
        );
        assert_eq!(
            chrono_free_month_start(1_709_251_200_000),
            1_709_251_200_000
        );
        assert_eq!(
            chrono_free_month_start(1_709_251_199_999),
            1_706_745_600_000
        );
    }
}
