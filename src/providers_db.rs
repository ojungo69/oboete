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
  outcome TEXT NOT NULL,                  -- ok, invalid, error, wait, budget, gate, too_big,
                                          -- and an answer curate::check refused: empty, prose,
                                          -- shape, over_cap, unanchored
  ms INTEGER NOT NULL,
  detail TEXT,
  bytes_out INTEGER NOT NULL DEFAULT 0,
  est_tokens INTEGER,                     -- the uncalibrated estimate of what was sent
  prompt_tokens INTEGER,
  completion_tokens INTEGER,
  cached_tokens INTEGER,
  reasoning_tokens INTEGER,
  usd REAL                                -- a paid entry's cost, fixed when the call is recorded
);
CREATE INDEX IF NOT EXISTS provider_calls_day ON provider_calls(provider, ts);
-- Whether a curator CLI provably cannot act (docs/milestone-3-plan.md Task 3): one row per CLI
-- version and probe profile, so a new codex or a changed profile is probed again.
CREATE TABLE IF NOT EXISTS isolation(
  cli TEXT NOT NULL,
  version TEXT NOT NULL,
  passed INTEGER NOT NULL,
  detail TEXT NOT NULL,                   -- a fixed reason, never the CLI's output
  ts INTEGER NOT NULL,
  PRIMARY KEY(cli, version)
);
-- The window each device's curation waits on (docs/milestone-3-plan.md D10, D11): one row per
-- device. It counts only while raw.db's next window is still this one, start and end: the
-- curation phase replaces a row that no longer is.
CREATE TABLE IF NOT EXISTS pending(
  device TEXT PRIMARY KEY,
  from_seq INTEGER NOT NULL,
  from_offset INTEGER,
  to_seq INTEGER NOT NULL,
  to_offset INTEGER,
  reason TEXT NOT NULL,                   -- why the last attempt gave no answer
  hold TEXT NOT NULL,                     -- time, budget or owner: what it waits for
  attempts INTEGER NOT NULL DEFAULT 0,    -- attempts that count toward D11's three
  next_attempt_at INTEGER NOT NULL,       -- unix ms: not tried again before then
  since INTEGER NOT NULL,                 -- when the window first waited
  prompt TEXT NOT NULL                    -- the SHA-256 of the request the attempts were on, with who was asked
);
-- A curator request currently being answered: metadata only, one generation per device. A
-- crashed process may leave a row; pid/start identify its owner, and the next request replaces it.
CREATE TABLE IF NOT EXISTS curation_inflight(
  device TEXT PRIMARY KEY,
  request_id TEXT NOT NULL,
  pid INTEGER NOT NULL,
  started_at INTEGER NOT NULL,            -- unix ms
  from_seq INTEGER NOT NULL,
  from_offset INTEGER,
  to_seq INTEGER NOT NULL,
  to_offset INTEGER,
  prompt_sha256 TEXT NOT NULL
);
-- What an entry's own key may request a day, where its budget is a fifth of that (#238): the
-- `:free` model requests OpenRouter's GET /api/v1/key gives. NULL when the read failed or its
-- answer had no limit.
CREATE TABLE IF NOT EXISTS key_limits(
  provider TEXT PRIMARY KEY,
  free_daily INTEGER,
  read_at INTEGER NOT NULL,               -- unix ms of the last read, failed or not
  key_sha TEXT NOT NULL                   -- which key it read: SHA-256's first 16 hex digits, '' for none
);
-- A session's digest that every provider failed (milestone 3 Task 9), as `pending` is for a
-- window: it counts only for the same request, and after D11's three attempts it is given up
-- until the request changes (the session's newer records, its claims, who is asked).
CREATE TABLE IF NOT EXISTS digest_pending(
  device TEXT NOT NULL,
  agent TEXT NOT NULL,
  session TEXT NOT NULL,
  repo TEXT NOT NULL,
  prompt TEXT NOT NULL,
  reason TEXT NOT NULL,
  hold TEXT NOT NULL,
  attempts INTEGER NOT NULL,
  next_attempt_at INTEGER NOT NULL,
  PRIMARY KEY(device, agent, session, repo)
);
";

/// The viewer's ledger reads never create a database or run a schema upgrade. An absent
/// ledger is different from one that cannot be read: only the former means no spend yet.
pub fn read_only(home: &Path) -> Result<Option<Connection>> {
    let path = home.join("providers.db");
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read providers ledger"),
    }
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("read providers ledger")?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(Some(conn))
}

pub fn open(home: &Path) -> Result<Connection> {
    let deadline = std::time::Instant::now() + crate::db::OPEN_WRITE_WAIT;
    let path = home.join("providers.db");
    crate::db::private(home, 0o700);
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(test)]
    crate::crash::arm(&conn);
    crate::db::wal_until(&conn, "NORMAL", deadline)?;
    crate::db::ensure_schema_until(&conn, SCHEMA, deadline).context("providers schema")?;
    // Columns added after the table's first version (milestone 3, Task 4).
    for column in [
        "tokens_left",
        "tokens_reset_at",
        "requests_left",
        "requests_reset_at",
    ] {
        crate::db::ensure_column_until(&mut conn, "provider_state", column, "INTEGER", deadline)?;
    }
    crate::db::ensure_column_until(&mut conn, "provider_calls", "usd", "REAL", deadline)?;
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
    /// A paid entry's cost at its price then (`budget::cost`); None for any other entry.
    pub usd: Option<f64>,
}

pub fn record(conn: &Connection, c: &Call) -> Result<()> {
    conn.execute(
        "INSERT INTO provider_calls(ts, provider, role, span, outcome, ms, detail, bytes_out,
           est_tokens, prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens, usd)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
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
            c.usage.reasoning,
            c.usd
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
/// any state. A window that waited on the owner is tried at the worker's next run, not an hour
/// later.
pub fn resume(conn: &Connection, provider: &str) -> Result<bool> {
    conn.execute(
        "UPDATE pending SET next_attempt_at = 0 WHERE hold = 'owner'",
        [],
    )?;
    conn.execute(
        "UPDATE digest_pending SET next_attempt_at = 0 WHERE hold = 'owner'",
        [],
    )?;
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

/// Requests sent to `provider` in the last 24 hours, and when the oldest of them was sent: the
/// per-provider daily budget counts a rolling day, as Groq counts its own (docs/milestone-1.md), so
/// it holds in any 24 hours, UTC days included. A 429 that was waited out still counts: the budget
/// bounds our requests, not our successes.
pub fn calls_in_a_day(conn: &Connection, provider: &str) -> Result<(u32, Option<i64>)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), MIN(ts) FROM provider_calls WHERE provider=?1 AND ts>=?2
           AND outcome IN ('ok','error','invalid','wait','empty','prose','shape','over_cap','unanchored',
                           'sent')",
        params![provider, now_ms() - DAY_MS],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// The last read of `provider`'s key limit (#238): the limit it gave, None when it failed, when
/// it was, and which key it read (`key_limits.key_sha`).
pub fn key_limit(conn: &Connection, provider: &str) -> Result<Option<(Option<u32>, i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT free_daily, read_at, key_sha FROM key_limits WHERE provider=?1",
            [provider],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

pub fn set_key_limit(
    conn: &Connection,
    provider: &str,
    free_daily: Option<u32>,
    at: i64,
    key_sha: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO key_limits(provider, free_daily, read_at, key_sha) VALUES(?1,?2,?3,?4)
         ON CONFLICT(provider) DO UPDATE SET free_daily=excluded.free_daily,
           read_at=excluded.read_at, key_sha=excluded.key_sha",
        params![provider, free_daily, at, key_sha],
    )?;
    Ok(())
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

pub const DAY_MS: i64 = 86_400_000;

/// When a call made at `ts` has left the rolling day that `calls_in_a_day` and `tokens_since`
/// count (`ts >= now - DAY_MS`): one ms after it is exactly a day old.
pub fn out_of_the_day(ts: i64) -> i64 {
    ts + DAY_MS + 1
}

/// A call that was sent and not refused with an HTTP error status (`http 429: …`): it may have used
/// tokens it did not report (a timeout, a dropped connection, an answer without a full usage block).
const SENT_NOT_REFUSED: &str = "bytes_out > 0
    AND outcome IN ('ok','invalid','error','empty','prose','shape','over_cap','unanchored')
    AND COALESCE(detail, '') NOT GLOB 'http [0-9][0-9][0-9]*'";

/// Tokens (prompt plus completion) `provider` reported since `since`, and when its oldest call
/// since then that the token budget counts was made: one that reported tokens or was sent and not
/// refused (our refusal or an HTTP error used none, so its age frees nothing).
pub fn tokens_since(conn: &Connection, provider: &str, since: i64) -> Result<(i64, Option<i64>)> {
    Ok(conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(COALESCE(prompt_tokens, 0) + COALESCE(completion_tokens, 0)), 0),
                    MIN(CASE WHEN COALESCE(prompt_tokens, 0) + COALESCE(completion_tokens, 0) > 0
                              OR ({SENT_NOT_REFUSED}) THEN ts END)
             FROM provider_calls WHERE provider=?1 AND ts>=?2"
        ),
        params![provider, since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// What every paid entry cost since the first of this month (UTC), at the prices of each call's
/// time: an entry since removed from the chain or repriced still counts. Embedding calls have a
/// cap of their own (`embed_usd_this_month`), so neither spend stops the other.
pub fn usd_this_month(conn: &Connection) -> Result<f64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(usd), 0) FROM provider_calls
         WHERE ts>=?1 AND role NOT IN ('embed', 'query')",
        [chrono_free_month_start(now_ms())],
        |r| r.get(0),
    )?)
}

/// What embedding calls, of documents and of queries, are estimated to have cost since the first
/// of this month (UTC): `[embedding] monthly_usd`'s count (milestone 4 D8).
pub fn embed_usd_this_month(conn: &Connection) -> Result<f64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(usd), 0) FROM provider_calls
         WHERE ts>=?1 AND role IN ('embed', 'query')",
        [chrono_free_month_start(now_ms())],
        |r| r.get(0),
    )?)
}

/// What became of a request `embed_phase::reserve` counted (milestone 4 D8): its outcome, time
/// and detail, its estimated cost kept only when it may have been run (`billed`).
pub fn settle(
    conn: &Connection,
    id: i64,
    outcome: &str,
    ms: i64,
    detail: &str,
    billed: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE provider_calls SET outcome = ?2, ms = ?3, detail = ?4,
           usd = CASE WHEN ?5 THEN usd END
         WHERE id = ?1",
        params![id, outcome, ms, detail, billed],
    )?;
    Ok(())
}

/// A request `embed_phase::reserve` counted that never left: no longer counted.
pub fn unreserve(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM provider_calls WHERE id = ?1 AND outcome = 'sent'",
        [id],
    )?;
    Ok(())
}

/// When the monthly spend starts again: the first of the next UTC month.
pub fn next_month() -> i64 {
    chrono_free_month_start(chrono_free_month_start(now_ms()) + 32 * DAY_MS)
}

/// What `provider`'s calls since `start` that were sent and not refused may have used beyond the
/// usage they reported: the estimate of each call with no prompt count, and the number of calls
/// with no completion count.
pub fn unmetered(conn: &Connection, provider: &str, start: i64) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(CASE WHEN prompt_tokens IS NULL THEN est_tokens END), 0),
                    COALESCE(SUM(completion_tokens IS NULL), 0)
             FROM provider_calls WHERE provider=?1 AND ts>=?2 AND {SENT_NOT_REFUSED}"
        ),
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

/// A window the curation phase waits on (D10, D11).
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub device: String,
    pub from_seq: i64,
    pub from_offset: Option<i64>,
    pub to_seq: i64,
    pub to_offset: Option<i64>,
    pub reason: String,
    pub hold: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub since: i64,
    /// The SHA-256 of the prompt the attempts were on.
    pub prompt: String,
}

const PENDING_COLUMNS: &str = "device, from_seq, from_offset, to_seq, to_offset, reason, hold,
     attempts, next_attempt_at, since, prompt";

fn pending_row(r: &rusqlite::Row) -> rusqlite::Result<Pending> {
    Ok(Pending {
        device: r.get(0)?,
        from_seq: r.get(1)?,
        from_offset: r.get(2)?,
        to_seq: r.get(3)?,
        to_offset: r.get(4)?,
        reason: r.get(5)?,
        hold: r.get(6)?,
        attempts: r.get(7)?,
        next_attempt_at: r.get(8)?,
        since: r.get(9)?,
        prompt: r.get(10)?,
    })
}

/// Every device's pending window, for doctor.
pub fn pending(conn: &Connection) -> Result<Vec<Pending>> {
    let mut st = conn.prepare(&format!(
        "SELECT {PENDING_COLUMNS} FROM pending ORDER BY device"
    ))?;
    Ok(st
        .query_map([], pending_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn pending_of(conn: &Connection, device: &str) -> Result<Option<Pending>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PENDING_COLUMNS} FROM pending WHERE device = ?1"),
            [device],
            pending_row,
        )
        .optional()?)
}

pub fn set_pending(conn: &Connection, p: &Pending) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO pending({PENDING_COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"
        ),
        params![
            p.device,
            p.from_seq,
            p.from_offset,
            p.to_seq,
            p.to_offset,
            p.reason,
            p.hold,
            p.attempts,
            p.next_attempt_at,
            p.since,
            p.prompt
        ],
    )?;
    Ok(())
}

pub fn clear_pending(conn: &Connection, device: &str) -> Result<()> {
    conn.execute("DELETE FROM pending WHERE device = ?1", [device])?;
    Ok(())
}

/// The current request's lifetime, including an unwinding curator. Completion is conditional
/// on its generation, so a late answer cannot clear another request's metadata.
#[must_use]
pub struct CurationRequest<'a> {
    conn: &'a Connection,
    device: &'a str,
    request_id: String,
}

impl Drop for CurationRequest<'_> {
    fn drop(&mut self) {
        if self
            .conn
            .execute(
                "DELETE FROM curation_inflight WHERE device=?1 AND request_id=?2",
                params![self.device, self.request_id],
            )
            .is_err()
        {
            // The call already happened: metadata failure must not discard its answer or cause
            // it to be charged again. No payload or database error text enters the diagnostic.
            eprintln!("oboete: curation request metadata could not be cleared; it may be stale");
        }
    }
}

/// Publish before calling the curator, without changing the call ledger or pending attempts.
/// Readers may SELECT `curation_inflight` through a read-only connection. Seq bounds are
/// inclusive, offsets are bytes within the first/last record; count raw records by device and
/// bounds, never by subtracting seqs. Only a SHA-256 is kept, never the request text. A row left
/// after a crash is not proof that its process still runs; the next request replaces it.
pub fn start_curation<'a>(
    conn: &'a Connection,
    device: &'a str,
    from: (i64, Option<i64>),
    to: (i64, Option<i64>),
    prompt_sha256: &str,
) -> Result<CurationRequest<'a>> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("random request id: {e}"))?;
    let request_id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute(
        "INSERT INTO curation_inflight(device, request_id, pid, started_at, from_seq,
           from_offset, to_seq, to_offset, prompt_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(device) DO UPDATE SET request_id=excluded.request_id, pid=excluded.pid,
           started_at=excluded.started_at, from_seq=excluded.from_seq,
           from_offset=excluded.from_offset, to_seq=excluded.to_seq,
           to_offset=excluded.to_offset, prompt_sha256=excluded.prompt_sha256",
        params![
            device,
            request_id,
            std::process::id(),
            now_ms(),
            from.0,
            from.1,
            to.0,
            to.1,
            prompt_sha256
        ],
    )?;
    Ok(CurationRequest {
        conn,
        device,
        request_id,
    })
}

/// A session's digest waiting after every provider failed (`digest_pending`).
#[derive(Debug, Clone, PartialEq)]
pub struct DigestPending {
    pub device: String,
    pub agent: String,
    pub session: String,
    pub repo: String,
    /// The SHA-256 of the request the attempts were on.
    pub prompt: String,
    pub reason: String,
    pub hold: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
}

pub fn digest_pending_of(
    conn: &Connection,
    device: &str,
    agent: &str,
    session: &str,
    repo: &str,
) -> Result<Option<DigestPending>> {
    Ok(conn
        .query_row(
            "SELECT prompt, reason, hold, attempts, next_attempt_at FROM digest_pending
             WHERE device = ?1 AND agent = ?2 AND session = ?3 AND repo = ?4",
            params![device, agent, session, repo],
            |r| {
                Ok(DigestPending {
                    device: device.into(),
                    agent: agent.into(),
                    session: session.into(),
                    repo: repo.into(),
                    prompt: r.get(0)?,
                    reason: r.get(1)?,
                    hold: r.get(2)?,
                    attempts: r.get(3)?,
                    next_attempt_at: r.get(4)?,
                })
            },
        )
        .optional()?)
}

pub fn set_digest_pending(conn: &Connection, p: &DigestPending) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO digest_pending(device, agent, session, repo, prompt, reason, hold,
           attempts, next_attempt_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            p.device,
            p.agent,
            p.session,
            p.repo,
            p.prompt,
            p.reason,
            p.hold,
            p.attempts,
            p.next_attempt_at
        ],
    )?;
    Ok(())
}

pub fn clear_digest_pending(
    conn: &Connection,
    device: &str,
    agent: &str,
    session: &str,
    repo: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM digest_pending WHERE device = ?1 AND agent = ?2 AND session = ?3
           AND repo = ?4",
        params![device, agent, session, repo],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inflight_curation_keeps_only_metadata_and_its_current_generation() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let reader = Connection::open_with_flags(
            home.path().join("providers.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let hash = "a".repeat(64);
        let started = now_ms();
        let old = start_curation(&db, "device-a", (7, Some(12)), (9, Some(34)), &hash).unwrap();
        let row: (String, i64, i64, i64, i64, i64, i64, String) = reader
            .query_row(
                "SELECT request_id, pid, started_at, from_seq, from_offset, to_seq, to_offset,
                   prompt_sha256 FROM curation_inflight WHERE device='device-a'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0.len(), 32);
        assert!(row.0.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(row.1, i64::from(std::process::id()));
        assert!((started..=now_ms()).contains(&row.2));
        assert_eq!((row.3, row.4, row.5, row.6), (7, 12, 9, 34));
        assert_eq!(row.7, hash);
        let columns: Vec<String> = reader
            .prepare("PRAGMA table_info(curation_inflight)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "device",
                "request_id",
                "pid",
                "started_at",
                "from_seq",
                "from_offset",
                "to_seq",
                "to_offset",
                "prompt_sha256"
            ]
        );
        let another_writer = open(home.path()).unwrap();
        let newer = start_curation(
            &another_writer,
            "device-a",
            (9, Some(34)),
            (11, None),
            &hash,
        )
        .unwrap();
        let other = start_curation(&db, "device-b", (7, None), (9, None), &hash).unwrap();
        drop(old);
        let kept: (String, i64, Option<i64>) = reader
            .query_row(
                "SELECT request_id, from_seq, to_offset FROM curation_inflight WHERE device='device-a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_ne!(kept.0, row.0);
        assert_eq!((kept.1, kept.2), (9, None));
        drop(newer);
        let devices: Vec<String> = reader
            .prepare("SELECT device FROM curation_inflight")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(devices, ["device-b"]);
        drop(other);
        let left: i64 = reader
            .query_row("SELECT count(*) FROM curation_inflight", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert!(last_calls(&db, 10).unwrap().is_empty());
    }

    #[test]
    fn schema_upgrade_is_atomic_and_keeps_the_cooldown_and_call_ledger() {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        set_state(
            &writer,
            "kept",
            State {
                down_until: OWNER_HOLD,
                fails: 2,
                backoff: 1,
            },
        )
        .unwrap();
        record(
            &writer,
            &Call {
                provider: "kept",
                role: "curator",
                span: "schema-test",
                outcome: "ok",
                ms: 7,
                detail: None,
                bytes_out: 1,
                est_tokens: None,
                usage: Usage::default(),
                usd: Some(0.25),
            },
        )
        .unwrap();
        let missing =
            "DROP TABLE key_limits; DROP INDEX provider_calls_day; DROP TABLE curation_inflight;";
        writer.execute_batch(missing).unwrap();
        crate::crash::off();
        let reopened = open(home.path()).unwrap();
        assert_eq!(crate::crash::count(), 1);
        reopened.execute_batch(missing).unwrap();
        crate::crash::at(1);
        let failed = open(home.path());
        crate::crash::off();
        assert!(failed.is_err());
        let remaining: i64 = writer.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('key_limits', 'provider_calls_day', 'curation_inflight')",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(remaining, 0);
        let recovered = open(home.path()).unwrap();
        assert_eq!(state(&recovered, "kept").unwrap().down_until, OWNER_HOLD);
        let kept: (i64, f64) = recovered
            .query_row(
                "SELECT count(*), SUM(usd) FROM provider_calls WHERE provider='kept'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (1, 0.25));
    }

    #[test]
    fn a_non_lock_schema_error_returns_immediately_without_partial_tables() {
        let home = tempfile::tempdir().unwrap();
        let writer = Connection::open(home.path().join("providers.db")).unwrap();
        crate::db::wal(&writer, "NORMAL").unwrap();
        writer
            .execute_batch(
                "CREATE TABLE provider_calls_day(kept TEXT);
                              INSERT INTO provider_calls_day VALUES ('unchanged');",
            )
            .unwrap();
        let started = std::time::Instant::now();
        let error = open(home.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("already a table named provider_calls_day"),
            "{error:#}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let tables: i64 = writer
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1);
        assert_eq!(
            writer
                .query_row("SELECT kept FROM provider_calls_day", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "unchanged"
        );
    }

    /// `oboete resume` makes a window that waited on the owner due now; one that waits on time
    /// keeps its time.
    #[test]
    fn resume_makes_a_window_waiting_on_the_owner_due() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        let row = |device: &str, hold: &str| Pending {
            device: device.into(),
            from_seq: 1,
            from_offset: None,
            to_seq: 2,
            to_offset: None,
            reason: "r".into(),
            hold: hold.into(),
            attempts: 0,
            next_attempt_at: 5_000_000_000_000,
            since: 1,
            prompt: "p".into(),
        };
        set_pending(&conn, &row("a", "owner")).unwrap();
        set_pending(&conn, &row("b", "time")).unwrap();
        resume(&conn, "claude").unwrap();
        let due: Vec<i64> = pending(&conn)
            .unwrap()
            .iter()
            .map(|p| p.next_attempt_at)
            .collect();
        assert_eq!(due, [0, 5_000_000_000_000]);
        // A digest that waits on the owner too.
        let digest = DigestPending {
            device: "a".into(),
            agent: "claude".into(),
            session: "s".into(),
            repo: "r".into(),
            prompt: "p".into(),
            reason: "r".into(),
            hold: "owner".into(),
            attempts: 0,
            next_attempt_at: 5_000_000_000_000,
        };
        set_digest_pending(&conn, &digest).unwrap();
        resume(&conn, "claude").unwrap();
        let got = digest_pending_of(&conn, "a", "claude", "s", "r").unwrap();
        assert_eq!(got.unwrap().next_attempt_at, 0);
    }

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
