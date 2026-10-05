//! What a provider may take before a call (docs/milestone-3-plan.md D7, Task 4): its daily calls
//! and tokens, a paid entry's share of the month's USD cap, and a request's size against the
//! provider's own ceiling, in estimated tokens. A refused call uploads nothing and costs nothing.

use crate::provider::Skip;
use anyhow::Result;
use rusqlite::Connection;

use crate::config::Provider;
use crate::providers_db::{self, Usage};

/// Tokens a text is estimated to take before a provider's own factor: 0.8 per CJK character and
/// 0.28 per other character, fitted on the size sweep of 2026-09-27 (docs/spike/curator-sizes.md:
/// nim, OpenRouter and OpenCode Go, 114 answers, no fixed part). Rounded up.
pub fn estimate(text: &str) -> u32 {
    let (cjk, other) = text.chars().fold((0u64, 0u64), |(c, o), ch| {
        if is_cjk(ch) { (c + 1, o) } else { (c, o + 1) }
    });
    u32::try_from((cjk * 80 + other * 28).div_ceil(100)).unwrap_or(u32::MAX)
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30ff}'     // CJK punctuation, hiragana, katakana
        | '\u{3400}'..='\u{9fff}'   // CJK ideographs
        | '\u{ac00}'..='\u{d7af}'   // hangul
        | '\u{f900}'..='\u{faff}'   // compatibility ideographs
        | '\u{ff00}'..='\u{ffef}') // full-width forms
}

/// How far `provider` counts from the estimate: the median of `prompt_tokens / est_tokens` over
/// its last 50 answers that reported both, 1.0 until there are 5.
pub fn factor(db: &Connection, provider: &str) -> Result<f64> {
    let mut ratios = providers_db::token_ratios(db, provider, 50)?;
    if ratios.len() < 5 {
        return Ok(1.0);
    }
    ratios.sort_by(f64::total_cmp);
    Ok(ratios[ratios.len() / 2])
}

/// Why a provider is not called for this request: the `provider_calls` outcome and its detail.
#[derive(Debug)]
pub struct Refusal {
    pub outcome: &'static str,
    pub detail: String,
    /// Until when it holds (for the curation phase, D10 and D11).
    pub skip: crate::provider::Skip,
}

/// The output reserved on an entry that declares none: the largest of the 42 Groq
/// answers in the owner's ledger was 1,240 tokens (2026-09-27), and the largest prompt and answer
/// together, 7,961, was taken under the 8,000 ceiling.
const UNDECLARED_OUTPUT: u32 = 1_250;

/// A ceiling check keeps this share of the ceiling free: the estimate is not exact.
pub(crate) const CEILING_SHARE: f64 = 0.95;

/// Fresh reservations are checked promptly. A retained older row progressively backs off,
/// up to the ordinary ten-minute outage wait, without inferring that its sender is dead.
const RESERVATION_WAIT_MS: i64 = 1_000;
const MAX_RESERVATION_WAIT_MS: i64 = 10 * 60_000;

/// One curation request's reserved allowance. Dropping it is deliberately not a refund: the
/// process may have sent the request before losing its answer.
#[derive(Debug)]
pub(crate) struct Reservation {
    id: i64,
    provider: String,
    role: String,
    span: String,
    input: f64,
    output: f64,
}

impl Reservation {
    /// Actual reported usage, with the admission-time bounds for any missing part.
    pub(crate) fn cost(&self, p: &Provider, usage: Usage, billed: bool) -> Option<f64> {
        (billed && p.limits().is_paid()).then(|| {
            p.limits().usd(
                usage.prompt.map_or(self.input, |n| n as f64),
                usage.completion.map_or(self.output, |n| n as f64),
            )
        })
    }

    /// Only for a positively unsent request that previously left no attempt (for example an
    /// egress gate error). Ordinary provider/preflight errors settle and keep their attempt.
    pub(crate) fn cancel(self, db: &Connection) -> Result<()> {
        let changed = db.execute(
            "DELETE FROM provider_calls WHERE id=?1 AND provider=?2 AND role=?3 AND span=?4
             AND outcome='reserved'",
            rusqlite::params![self.id, self.provider, self.role, self.span],
        )?;
        anyhow::ensure!(changed == 1, "reservation already settled or missing");
        Ok(())
    }

    /// Settle exactly this attempt and its rate/state in one short transaction. State is read
    /// here, after external work, so another sender's owner hold cannot be overwritten.
    pub(crate) fn settle(
        self,
        db: &Connection,
        call: &providers_db::Call<'_>,
        rate: Option<providers_db::RateLeft>,
        next: impl FnOnce(providers_db::State) -> providers_db::State,
    ) -> Result<providers_db::State> {
        anyhow::ensure!(
            call.provider == self.provider && call.role == self.role && call.span == self.span,
            "reservation does not match this attempt"
        );
        anyhow::ensure!(
            call.outcome != "reserved",
            "reservation needs a final outcome"
        );
        let tx =
            rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE provider_calls SET outcome=?2, ms=?3, detail=?4, bytes_out=?5,
             est_tokens=?6, prompt_tokens=?7, completion_tokens=?8, cached_tokens=?9,
             reasoning_tokens=?10, usd=?11
             WHERE id=?1 AND outcome='reserved' AND provider=?12 AND role=?13 AND span=?14",
            rusqlite::params![
                self.id,
                call.outcome,
                call.ms,
                call.detail,
                call.bytes_out as i64,
                call.est_tokens,
                call.usage.prompt,
                call.usage.completion,
                call.usage.cached,
                call.usage.reasoning,
                call.usd,
                self.provider,
                self.role,
                self.span
            ],
        )?;
        anyhow::ensure!(changed == 1, "reservation already settled or missing");
        providers_db::freeze_unmetered(&tx, self.id, [self.input, self.output])?;
        let current = providers_db::state(&tx, &self.provider)?;
        let mut updated = next(current);
        // An answer cannot implicitly resume an owner-held entry or shorten a concurrent rest.
        if current.down_until > crate::db::now_ms() {
            updated.down_until = updated.down_until.max(current.down_until);
        }
        if current.down_until == providers_db::OWNER_HOLD {
            updated = current;
        }
        if updated != current {
            providers_db::set_state(&tx, &self.provider, updated)?;
        }
        if let Some(rate) = rate {
            let current = providers_db::rate(&tx, &self.provider)?;
            let (requests, requests_reset_at) = live_rate(
                (current.requests, current.requests_reset_at),
                (rate.requests, rate.requests_reset_at),
            );
            let (tokens, tokens_reset_at) = live_rate(
                (current.tokens, current.tokens_reset_at),
                (rate.tokens, rate.tokens_reset_at),
            );
            providers_db::set_rate(
                &tx,
                &self.provider,
                providers_db::RateLeft {
                    requests,
                    requests_reset_at,
                    tokens,
                    tokens_reset_at,
                },
            )?;
        }
        tx.commit()?;
        Ok(updated)
    }
}

/// Headers are captured before a response body finishes: an older snapshot can settle last.
/// A live interval therefore keeps the smaller allowance and later reset. After its reset,
/// the provider's next snapshot may replenish it normally.
fn live_rate(
    current: (Option<i64>, Option<i64>),
    incoming: (Option<i64>, Option<i64>),
) -> (Option<i64>, Option<i64>) {
    if let (Some(left), Some(until)) = current
        && until > crate::db::now_ms()
    {
        (
            Some(incoming.0.map_or(left, |n| n.min(left))),
            Some(until.max(incoming.1.unwrap_or(until))),
        )
    } else {
        incoming
    }
}

/// Atomically share curation budgets between the worker and an explicit settings probe. No
/// transaction survives this function, so isolation, allowance discovery and dispatch can run
/// afterwards without holding the ledger's write lock.
pub(crate) fn reserve(
    db: &Connection,
    p: &Provider,
    role: &str,
    span: &str,
    est: u32,
    paid_usd_per_month: f64,
    ceiling_hit: &[u32],
) -> Result<std::result::Result<Reservation, Refusal>> {
    reserve_with_history(db, (p, p), role, span, est, paid_usd_per_month, ceiling_hit)
}

/// Reserve this request while evaluating legacy history with the normal selected entry.
/// Pending/frozen rows keep their own bounds; only unframed legacy rows need this fallback.
pub(crate) fn reserve_with_history(
    db: &Connection,
    providers: (&Provider, &Provider),
    role: &str,
    span: &str,
    est: u32,
    paid_usd_per_month: f64,
    ceiling_hit: &[u32],
) -> Result<std::result::Result<Reservation, Refusal>> {
    let (p, history) = providers;
    anyhow::ensure!(
        p.name() == history.name(),
        "budget history does not match provider"
    );
    anyhow::ensure!(
        !matches!(role, "embed" | "query"),
        "embedding has its own reservation"
    );
    if !p.enabled() {
        return Ok(Err(Refusal {
            outcome: "gate",
            detail: "disabled in settings".into(),
            skip: Skip::Owner,
        }));
    }
    let tx = rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let state = providers_db::state(&tx, p.name())?;
    if state.down_until == providers_db::OWNER_HOLD {
        return Ok(Err(Refusal {
            outcome: "gate",
            detail: format!(
                "stopped until the owner acts (`oboete resume {}`)",
                p.name()
            ),
            skip: Skip::Owner,
        }));
    }
    if state.down_until > crate::db::now_ms() {
        return Ok(Err(Refusal {
            outcome: "gate",
            detail: "cooling down after an earlier failure".into(),
            skip: Skip::Wait(state.down_until),
        }));
    }
    let input = f64::from(est) * factor(&tx, p.name())?;
    if let Some(refusal) =
        admit_with_history(&tx, (p, history), input, paid_usd_per_month, ceiling_hit)?
    {
        return Ok(Err(refusal));
    }
    let output = f64::from(output_bound(p));
    let detail = serde_json::to_string(&[input, output])?;
    providers_db::record(
        &tx,
        &providers_db::Call {
            provider: p.name(),
            role,
            span,
            outcome: "reserved",
            ms: 0,
            detail: Some(&detail),
            bytes_out: 0,
            est_tokens: None,
            usage: Usage::default(),
            usd: p.limits().is_paid().then(|| p.limits().usd(input, output)),
        },
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Ok(Reservation {
        id,
        provider: p.name().into(),
        role: role.into(),
        span: span.into(),
        input,
        output,
    }))
}

pub(crate) fn output_bound(p: &Provider) -> u32 {
    match p.declared_output() {
        0 if p.limits().is_paid() => largest_output(p),
        0 => UNDECLARED_OUTPUT,
        n => n,
    }
}

/// A read of a key's own limit (#238) holds a day; one that failed is tried again after an hour.
pub(crate) const KEY_READ_HOLDS_MS: i64 = providers_db::DAY_MS;
pub(crate) const KEY_READ_RETRY_MS: i64 = 3_600_000;

/// `p`'s calls a day. An entry whose budget is its key's (`Provider::budget_from_key`) takes a
/// fifth of the limit its key's last read gave, and its default until a read gives one (#238).
pub fn daily(db: &Connection, p: &Provider) -> Result<u32> {
    if !p.budget_from_key() {
        return Ok(p.daily_budget());
    }
    Ok(daily_after(p, &key_read(db, p)?))
}

/// The last read of `p`'s key's limit, when it read the key `p` holds now: one of a replaced key,
/// or of none, is not this key's (doctor reads without the chain's refresh).
fn key_read(db: &Connection, p: &Provider) -> Result<Option<(Option<u32>, i64, String)>> {
    let now = crate::provider::key_of(p);
    Ok(providers_db::key_limit(db, p.name())?.filter(|(_, _, of)| *of == now))
}

/// When the key read `read` stops holding and the next is due (#238): a day after one that gave a
/// limit, an hour after one that gave none.
pub(crate) fn read_due(read: &(Option<u32>, i64, String)) -> i64 {
    read.1
        + if read.0.is_some() {
            KEY_READ_HOLDS_MS
        } else {
            KEY_READ_RETRY_MS
        }
}

/// `daily` for an entry whose key's last read is `read`.
fn daily_after(p: &Provider, read: &Option<(Option<u32>, i64, String)>) -> u32 {
    match read {
        Some((Some(limit), _, _)) => limit / 5,
        _ => p.daily_budget(),
    }
}

/// doctor's words for an entry whose budget is its key's (#238): the budget in use, and where it
/// came from. `db` is None before providers.db exists.
pub fn key_budget(db: Option<&Connection>, p: &Provider) -> Result<String> {
    let read = match db {
        Some(db) => key_read(db, p)?,
        None => None,
    };
    let budget = daily_after(p, &read);
    let mut said = match &read {
        Some((Some(limit), at, _)) => format!(
            "{budget} calls a day, a fifth of the {limit} free-model requests a day its key has (read {})",
            crate::db::utc(*at)
        ),
        Some((None, at, _)) => format!(
            "{budget} calls a day: the read of its key's own limit at {} gave none",
            crate::db::utc(*at)
        ),
        None => format!("{budget} calls a day until its key's own limit is read"),
    };
    // doctor runs no refresh: a read past due is what the next run replaces (Codex on 474d60c).
    if read
        .as_ref()
        .is_some_and(|r| read_due(r) <= crate::db::now_ms())
    {
        said.push_str("; the next curation run reads it again");
    }
    Ok(said)
}

/// Check a request with the normal selected entry's fallback for unmetered legacy rows.
/// A bounded probe changes its own output allowance, not the policy for previous calls.
/// `tokens` is calibrated; `ceiling_hit` holds ceilings that refused this chain's request.
pub(crate) fn admit_with_history(
    db: &Connection,
    providers: (&Provider, &Provider),
    tokens: f64,
    paid_usd_per_month: f64,
    ceiling_hit: &[u32],
) -> Result<Option<Refusal>> {
    let (p, history) = providers;
    anyhow::ensure!(
        p.name() == history.name(),
        "budget history does not match provider"
    );
    let name = p.name();
    let limits = p.limits();
    let now = crate::db::now_ms();
    // A rolling day, as `limits.daily_tokens` below: until the oldest call counted leaves it.
    let (used, oldest) = providers_db::calls_in_a_day(db, name)?;
    let pending = providers_db::reserved_since(db, name, now - providers_db::DAY_MS)?;
    // ponytail: a key's budget (#238) counts the entry's calls, not its key's. Two entries on one
    // OpenRouter key take a fifth each, and a replaced key's calls count for a day. Record the
    // key_id with each call and count by it if an entry ever shares its key; the default has one.
    let read = if p.budget_from_key() {
        key_read(db, p)?
    } else {
        None
    };
    let budget = daily_after(p, &read);
    if used >= budget {
        let mut until = providers_db::out_of_the_day(oldest.unwrap_or(now));
        // The next read of a key's limit may raise the budget: the refusal lasts until it is due,
        // so the chain, which a refused window waits out, runs the read then.
        if let Some(read) = &read {
            until = until.min(read_due(read));
        }
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!("{used}/{budget} calls in 24 hours"),
            skip: if used.saturating_sub(pending.calls) < budget {
                Skip::Wait(reservation_retry_at(db, Some(name), now)?)
            } else {
                Skip::Budget(until)
            },
        }));
    }
    // The answer counts against the same limits as the prompt (Groq's TPM is input and output
    // together): the declared output, or with none declared, the largest answer seen.
    let output = output_bound(p);
    let reserved = tokens + f64::from(output);
    if let Some(max) = limits.max_request_tokens {
        if ceiling_hit.contains(&max) {
            return Ok(Some(Refusal {
                outcome: "too_big",
                detail: format!("an entry with the same {max}-token ceiling refused it"),
                skip: Skip::TooBig,
            }));
        }
        // The estimate keeps its margin; the output is a bound, compared with the ceiling itself.
        if tokens > f64::from(max) * CEILING_SHARE || reserved > f64::from(max) {
            return Ok(Some(Refusal {
                outcome: "too_big",
                detail: format!("about {reserved:.0} tokens, over its {max}"),
                skip: Skip::TooBig,
            }));
        }
    }
    let rate = providers_db::rate(db, name)?;
    if let Some(refusal) = rate_refusal(db, name, rate, &pending, reserved, now)? {
        return Ok(Some(refusal));
    }
    // Counted over the last 24 hours: Groq's day is a rolling window (docs/milestone-1.md), and a
    // budget kept per UTC day could take twice its share around midnight.
    if let Some(daily) = limits.daily_tokens {
        let since = now - providers_db::DAY_MS;
        let (reported, oldest) = providers_db::tokens_since(db, name, since)?;
        let settled = reported as f64 + unmetered(db, history, since)?;
        let used = settled + pending.tokens;
        if used + reserved > daily as f64 {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!("{used:.0}/{daily} tokens in 24 hours"),
                // When the oldest call counted leaves the 24 hours.
                skip: if settled + reserved <= daily as f64 {
                    Skip::Wait(reservation_retry_at(db, Some(name), now)?)
                } else {
                    Skip::Budget(providers_db::out_of_the_day(oldest.unwrap_or(now)))
                },
            }));
        }
    }
    if limits.is_paid() {
        let spent = providers_db::usd_this_month(db)?;
        // The output the request asks for at most, as `provider::call` sends it.
        let this = limits.usd(tokens, f64::from(output));
        if spent + this > paid_usd_per_month {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!(
                    "USD {spent:.2} of {paid_usd_per_month:.2} spent this month; this call up to {this:.3}"
                ),
                skip: if spent - providers_db::reserved_usd_this_month(db)? + this
                    <= paid_usd_per_month
                {
                    Skip::Wait(reservation_retry_at(db, None, now)?)
                } else {
                    Skip::Budget(providers_db::next_month())
                },
            }));
        }
    }
    Ok(None)
}

/// A provider's reported rate window also covers requests whose reservations are still pending.
fn rate_refusal(
    db: &Connection,
    name: &str,
    rate: providers_db::RateLeft,
    pending: &providers_db::Reserved,
    reserved: f64,
    now: i64,
) -> Result<Option<Refusal>> {
    if rate
        .requests
        .is_some_and(|left| left <= i64::from(pending.calls))
        && let Some(at) = rate.requests_reset_at.filter(|&t| t > now)
    {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: "no requests left until its reset".into(),
            skip: Skip::Wait(if rate.requests == Some(0) {
                at
            } else {
                at.min(reservation_retry_at(db, Some(name), now)?)
            }),
        }));
    }
    if let (Some(left), Some(at)) = (rate.tokens, rate.tokens_reset_at)
        && at > now
        && (left as f64) < reserved + pending.tokens
    {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!(
                "{left} tokens left until its reset in {} s",
                (at - now) / 1000
            ),
            skip: Skip::Wait(if (left as f64) < reserved {
                at
            } else {
                at.min(reservation_retry_at(db, Some(name), now)?)
            }),
        }));
    }
    Ok(None)
}

/// Reservation age is a scheduling signal, never evidence that it was unsent or refundable.
/// Using the newest contributing row keeps a fresh sender responsive, survives restarts, and
/// avoids a one-second loop for the full accounting window when a sender never settles.
fn reservation_retry_at(db: &Connection, provider: Option<&str>, now: i64) -> Result<i64> {
    let latest: Option<i64> = match provider {
        Some(provider) => db.query_row(
            "SELECT MAX(ts) FROM provider_calls WHERE provider=?1 AND ts>=?2
             AND outcome='reserved' AND role NOT IN ('embed','query')",
            rusqlite::params![provider, now - providers_db::DAY_MS],
            |row| row.get(0),
        )?,
        None => db.query_row(
            "SELECT MAX(ts) FROM provider_calls WHERE outcome='reserved' AND usd>0
             AND role NOT IN ('embed','query')
             AND ts>=CAST(strftime('%s',?1/1000,'unixepoch','start of month') AS INTEGER)*1000",
            [now],
            |row| row.get(0),
        )?,
    };
    let age = now.saturating_sub(latest.unwrap_or(now));
    Ok(now + age.clamp(RESERVATION_WAIT_MS, MAX_RESERVATION_WAIT_MS))
}

/// The most `calls` requests of `tokens` estimated tokens in all may cost when every paid entry of
/// `providers` bills each of them (one that times out or drops its answer may still bill it, and
/// the chain goes on to the next): each entry's calibrated input, and its largest answer each
/// time. `None` when no entry is paid (`oboete recurate`'s estimate, Task 11).
pub fn most_usd(
    db: &Connection,
    providers: &[Provider],
    tokens: u32,
    calls: usize,
) -> Result<Option<f64>> {
    let mut most: Option<f64> = None;
    for p in providers.iter().filter(|p| p.limits().is_paid()) {
        let input = f64::from(tokens) * factor(db, p.name())?;
        let output = calls as f64 * f64::from(largest_output(p));
        let usd = p.limits().usd(input, output);
        most = Some(most.unwrap_or(0.0) + usd);
    }
    Ok(most)
}

/// The answer a request may get at most: its declared output, or the entry's output cap.
fn largest_output(p: &Provider) -> u32 {
    match p.declared_output() {
        0 => p.limits().max_output_tokens,
        n => n,
    }
}

/// The tokens `p`'s sent calls since `start` may have used and did not report. Settled
/// reservations keep their own input/output bounds; only legacy rows use today's fallback.
fn unmetered(db: &Connection, p: &Provider, start: i64) -> Result<f64> {
    providers_db::unmetered(
        db,
        p.name(),
        start,
        factor(db, p.name())?,
        largest_output(p),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Limits;
    use crate::providers_db::{Call, Usage, open, record};

    fn admit(
        db: &Connection,
        p: &Provider,
        tokens: f64,
        cap: f64,
        ceilings: &[u32],
    ) -> Result<Option<Refusal>> {
        admit_with_history(db, (p, p), tokens, cap, ceilings)
    }

    fn entry(name: &str, limits: Limits) -> Provider {
        Provider::Openai {
            enabled: true,
            name: name.into(),
            base_url: "http://127.0.0.1:9".into(),
            key_file: None,
            model: "m".into(),
            daily_budget: Some(10),
            timeout_s: 1,
            retry_429: false,
            extra: Default::default(),
            headers: Default::default(),
            limits,
            subscription: false,
        }
    }

    fn cost(
        db: &Connection,
        p: &Provider,
        est: u32,
        usage: Usage,
        billed: bool,
    ) -> Result<Option<f64>> {
        let reservation = reserve(db, p, "curator", "estimate", est, f64::MAX, &[])?
            .map_err(|r| anyhow::anyhow!(r.detail))?;
        let usd = reservation.cost(p, usage, billed);
        reservation.cancel(db)?;
        Ok(usd)
    }

    /// A call the budget refused: recorded, nothing sent, no usage.
    fn refusal(db: &Connection, provider: &str) {
        record(
            db,
            &Call {
                provider,
                role: "curator",
                span: "s",
                outcome: "budget",
                ms: 0,
                detail: Some("budget"),
                bytes_out: 0,
                est_tokens: None,
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
    }

    fn call(db: &Connection, provider: &str, est: Option<u32>, prompt: i64, completion: i64) {
        record(
            db,
            &Call {
                provider,
                role: "curator",
                span: "s",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: est,
                usage: Usage {
                    prompt: Some(prompt),
                    completion: Some(completion),
                    ..Default::default()
                },
                usd: None,
            },
        )
        .unwrap();
    }

    /// `oboete recurate`'s estimate: every window billed by every paid entry, its largest answer
    /// each time; none when nothing is paid.
    #[test]
    fn the_most_a_recuration_costs_is_every_paid_entry_billed() {
        let db = open(tempfile::tempdir().unwrap().path()).unwrap();
        let free = entry("free", Limits::default());
        let priced = |name: &str, usd: f64| {
            let limits = Limits {
                usd_per_mtok_in: usd,
                usd_per_mtok_out: 2.0 * usd,
                max_output_tokens: 1_000,
                ..Default::default()
            };
            entry(name, limits)
        };
        let all = [free.clone(), priced("cheap", 1.0), priced("dear", 3.0)];
        // 10,000 tokens in, two answers of 1,000 at twice the input price, on each paid entry.
        let most = most_usd(&db, &all, 10_000, 2).unwrap().unwrap();
        let each = |usd: f64| (10_000.0 * usd + 2_000.0 * 2.0 * usd) / 1e6;
        assert!((most - (each(1.0) + each(3.0))).abs() < 1e-12);
        assert_eq!(most_usd(&db, &[free], 10_000, 2).unwrap(), None);
    }

    /// A call to paid entry `p` with its cost stored, as the chain records one.
    fn paid_call(db: &Connection, p: &Provider, prompt: i64, completion: i64) {
        let usage = Usage {
            prompt: Some(prompt),
            completion: Some(completion),
            ..Default::default()
        };
        record(
            db,
            &Call {
                provider: p.name(),
                role: "curator",
                span: "s",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: None,
                usage,
                usd: cost(db, p, 0, usage, true).unwrap(),
            },
        )
        .unwrap();
    }

    #[test]
    fn reservation_retry_uses_only_rows_contributing_to_each_accounting_window() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        // A fixed UTC clock at 2026-09-05 makes day/month boundary fixtures deterministic.
        let now = 1_788_566_400_000_i64;
        let add = |name: &str, role: &str, usd: Option<f64>, at: i64| {
            record(
                &db,
                &Call {
                    provider: name,
                    role,
                    span: name,
                    outcome: "reserved",
                    ms: 0,
                    detail: Some("[100,4000]"),
                    bytes_out: 0,
                    est_tokens: None,
                    usage: Usage::default(),
                    usd,
                },
            )
            .unwrap();
            db.execute(
                "UPDATE provider_calls SET ts=?2 WHERE id=?1",
                rusqlite::params![db.last_insert_rowid(), at],
            )
            .unwrap();
        };
        add(
            "old-paid",
            "curator",
            Some(2.0),
            now - 2 * providers_db::DAY_MS,
        );
        add(
            "ancient-paid",
            "curator",
            Some(9.0),
            now - 35 * providers_db::DAY_MS,
        );
        add("free", "curator", None, now);
        add("zero", "curator", Some(0.0), now);
        add("query", "query", Some(9.0), now);
        add("embed", "embed", Some(9.0), now);
        assert_eq!(
            reservation_retry_at(&db, Some("old-paid"), now).unwrap(),
            now + 1_000
        );
        assert_eq!(reservation_retry_at(&db, None, now).unwrap(), now + 600_000);
        add("daily", "curator", Some(1.0), now - 3_600_000);
        assert_eq!(
            reservation_retry_at(&db, Some("daily"), now).unwrap(),
            now + 600_000
        );
        // Only another positive paid reservation in this month restarts the short recheck.
        add("fresh-paid", "curator", Some(0.5), now - 500);
        assert_eq!(reservation_retry_at(&db, None, now).unwrap(), now + 1_000);
        assert_eq!(
            reservation_retry_at(&db, Some("daily"), now).unwrap(),
            now + 600_000
        );
    }

    #[test]
    fn an_aged_active_sender_can_settle_and_immediately_release_a_small_probe() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let normal = entry(
            "p",
            Limits {
                daily_tokens: Some(4_200),
                max_output_tokens: 4_000,
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 1.0,
                ..Default::default()
            },
        );
        let held = reserve(&db, &normal, "curator", "held", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        db.execute(
            "UPDATE provider_calls SET ts=?1 WHERE span='held'",
            [crate::db::now_ms() - 3_600_000],
        )
        .unwrap();
        let prepared = crate::provider::probe_provider(&normal).unwrap();
        let est = crate::provider::probe_estimate(&prepared);
        let before = crate::db::now_ms();
        let refusal =
            reserve_with_history(&db, (&prepared, &normal), "probe", "test", est, 5.0, &[])
                .unwrap()
                .unwrap_err();
        assert!(
            matches!(refusal.skip, Skip::Wait(at) if at>=before+600_000 && at<=crate::db::now_ms()+600_000)
        );
        let usage = Usage {
            prompt: Some(10),
            completion: Some(10),
            ..Default::default()
        };
        let usd = held.cost(&normal, usage, true);
        held.settle(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "held",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 100,
                est_tokens: Some(100),
                usage,
                usd,
            },
            None,
            |state| state,
        )
        .unwrap();
        let allowed =
            reserve_with_history(&db, (&prepared, &normal), "probe", "test", est, 5.0, &[])
                .unwrap()
                .unwrap();
        assert!(
            (allowed.cost(&prepared, Usage::default(), true).unwrap() - 0.000231).abs() < 1e-12
        );
        assert_eq!(
            providers_db::state(&db, "p").unwrap(),
            providers_db::State::default()
        );
        assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 20);
        allowed.cancel(&db).unwrap();
    }

    #[test]
    fn reservation_pressure_backs_off_after_restart_without_refunding_it() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let mut p = entry("p", Limits::default());
        if let Provider::Openai { daily_budget, .. } = &mut p {
            *daily_budget = Some(1);
        }
        let held = reserve(&db, &p, "curator", "held", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let retry = |db: &Connection| {
            let before = crate::db::now_ms();
            let refusal = admit(db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
            let Skip::Wait(until) = refusal.skip else {
                panic!("temporary pressure became a permanent hold")
            };
            (before, until)
        };
        let (before, fresh) = retry(&db);
        assert!((1_000..2_000).contains(&(fresh - before)));
        db.execute(
            "UPDATE provider_calls SET ts=?1 WHERE span='held'",
            [crate::db::now_ms() - 3_600_000],
        )
        .unwrap();
        drop(db);
        let reopened = open(home.path()).unwrap();
        let (before, older) = retry(&reopened);
        assert!(
            (600_000..601_000).contains(&(older - before)),
            "aged reservation retry delay: {}",
            older - before
        );
        assert_eq!(providers_db::calls_in_a_day(&reopened, "p").unwrap().0, 1);
        assert_eq!(
            providers_db::state(&reopened, "p").unwrap(),
            providers_db::State::default()
        );
        // Explicit proof that a sender never sent may still cancel; age alone did not refund.
        held.cancel(&reopened).unwrap();
        assert!(admit(&reopened, &p, 1.0, 5.0, &[]).unwrap().is_none());
    }

    #[test]
    fn settled_missing_components_survive_calibration_and_same_name_parameter_changes() {
        for (usage, reported, used, usd) in [
            (
                Usage {
                    prompt: Some(70),
                    ..Default::default()
                },
                4_070,
                8_070,
                0.00407,
            ),
            (
                Usage {
                    completion: Some(20),
                    ..Default::default()
                },
                4_020,
                4_220,
                0.00022,
            ),
            (
                Usage {
                    prompt: Some(70),
                    completion: Some(20),
                    ..Default::default()
                },
                4_090,
                4_090,
                0.00009,
            ),
            (Usage::default(), 4_000, 8_200, 0.0042),
        ] {
            let home = tempfile::tempdir().unwrap();
            let db = open(home.path()).unwrap();
            let mut normal = entry(
                "p",
                Limits {
                    daily_tokens: Some(20_000),
                    max_output_tokens: 4_000,
                    usd_per_mtok_in: 1.0,
                    usd_per_mtok_out: 1.0,
                    ..Default::default()
                },
            );
            if let Provider::Openai { daily_budget, .. } = &mut normal {
                *daily_budget = Some(100);
            }
            // 1,000 actual tokens teach factor 2 before admission: the frozen input is 200.
            for _ in 0..5 {
                call(&db, "p", Some(100), 200, 0);
            }
            assert_eq!(factor(&db, "p").unwrap(), 2.0);
            let reservation = reserve(&db, &normal, "curator", "original", 100, 5.0, &[])
                .unwrap()
                .unwrap();
            let price = reservation.cost(&normal, usage, true);
            reservation
                .settle(
                    &db,
                    &Call {
                        provider: "p",
                        role: "curator",
                        span: "original",
                        outcome: "ok",
                        ms: 1,
                        detail: Some("vetted completion"),
                        bytes_out: 100,
                        est_tokens: Some(100),
                        usage,
                        usd: price,
                    },
                    None,
                    |_| Default::default(),
                )
                .unwrap();
            // A later 3,000 actual tokens change factor to 3. Changing model/prices/output for
            // the same name cannot re-estimate either missing component of the earlier call.
            for _ in 0..10 {
                call(&db, "p", Some(100), 300, 0);
            }
            assert_eq!(factor(&db, "p").unwrap(), 3.0);
            let mut current = crate::provider::probe_provider(&normal).unwrap();
            if let Provider::Openai { model, limits, .. } = &mut current {
                *model = "another-model".into();
                limits.usd_per_mtok_in = 9.0;
                limits.usd_per_mtok_out = 9.0;
                limits.daily_tokens = Some(used + 127);
            }
            let refusal = admit(&db, &current, 0.0, 5.0, &[])
                .unwrap()
                .expect("128 output tokens do not fit");
            assert_eq!(
                refusal.detail,
                format!("{used}/{} tokens in 24 hours", used + 127)
            );
            assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, reported);
            assert!((providers_db::usd_this_month(&db).unwrap() - usd).abs() < 1e-12);
            assert!(
                providers_db::last_calls(&db, 16)
                    .unwrap()
                    .iter()
                    .any(|line| line == "p curator ok 1ms vetted completion")
            );
            assert!(
                providers_db::last_calls(&db, 16)
                    .unwrap()
                    .iter()
                    .all(|line| !line.contains("oboete-budget-v1"))
            );
        }
    }

    #[test]
    fn only_the_final_owned_bounds_count_and_invalid_stored_bounds_fail_closed() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let p = entry(
            "p",
            Limits {
                daily_tokens: Some(4_100),
                max_output_tokens: 4_000,
                usd_per_mtok_in: 1.0,
                ..Default::default()
            },
        );
        let reservation = reserve(&db, &p, "curator", "original", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            providers_db::last_calls(&db, 1).unwrap(),
            ["p curator reserved 0ms allowance reserved"]
        );
        let reason = "vetted failure\u{1e}oboete-budget-v1:[0,0]";
        reservation
            .settle(
                &db,
                &Call {
                    provider: "p",
                    role: "curator",
                    span: "original",
                    outcome: "error",
                    ms: 1,
                    detail: Some(reason),
                    bytes_out: 100,
                    est_tokens: Some(100),
                    usage: Usage::default(),
                    usd: None,
                },
                None,
                |state| state,
            )
            .unwrap();
        let small = crate::provider::probe_provider(&p).unwrap();
        assert_eq!(
            admit(&db, &small, 0.0, 5.0, &[]).unwrap().unwrap().detail,
            "4100/4100 tokens in 24 hours"
        );
        assert_eq!(
            providers_db::last_calls(&db, 1).unwrap(),
            [format!("p curator error 1ms {reason}")]
        );
        // Corrupt fixture metadata cannot silently become zero usage or today's smaller cap.
        for malformed in [
            "[-1,4000]",
            "[100,-1]",
            "[100,null]",
            "[1e999,4000]",
            "[100]",
            "[100,4000,7]",
            "[100,4000] trailing",
            "\"private-canary\"",
        ] {
            let detail =
                format!("\u{1e}oboete-call-v1:vetted failure\u{1e}oboete-budget-v1:{malformed}");
            db.execute(
                "UPDATE provider_calls SET detail=?1 WHERE span='original'",
                [detail],
            )
            .unwrap();
            let error = admit(&db, &small, 0.0, 5.0, &[])
                .expect_err("invalid bounds must refuse admission");
            assert_eq!(error.to_string(), "invalid stored token bounds");
            assert_eq!(
                providers_db::last_calls(&db, 1).unwrap(),
                ["p curator error 1ms vetted failure"]
            );
        }
    }

    #[test]
    fn legacy_unmetered_rows_keep_the_existing_fallback_without_invented_history() {
        for detail in [
            "legacy timeout",
            "[9000,9000]",
            "claude error\u{1e}oboete-budget-v1:[0,0]",
        ] {
            let home = tempfile::tempdir().unwrap();
            let db = open(home.path()).unwrap();
            record(
                &db,
                &Call {
                    provider: "p",
                    role: "curator",
                    span: "legacy",
                    outcome: "error",
                    ms: 1,
                    detail: Some(detail),
                    bytes_out: 100,
                    est_tokens: Some(100),
                    usage: Usage::default(),
                    usd: None,
                },
            )
            .unwrap();
            let p = entry(
                "p",
                Limits {
                    daily_tokens: Some(300),
                    max_output_tokens: 128,
                    usd_per_mtok_in: 1.0,
                    ..Default::default()
                },
            );
            let refusal = admit(&db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
            assert_eq!(refusal.detail, "228/300 tokens in 24 hours");
            assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 0);
            assert_eq!(
                providers_db::last_calls(&db, 1).unwrap(),
                [format!("p curator error 1ms {detail}")]
            );
        }
    }

    #[test]
    fn a_small_probe_cannot_shrink_a_previous_unmetered_curation_call() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let normal = entry(
            "p",
            Limits {
                daily_tokens: Some(4_100),
                max_output_tokens: 4_000,
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 1.0,
                ..Default::default()
            },
        );
        let call = reserve(&db, &normal, "curator", "normal", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let usd = call.cost(&normal, Usage::default(), true);
        call.settle(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "normal",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 100,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd,
            },
            None,
            |_| Default::default(),
        )
        .unwrap();
        let probe = crate::provider::probe_provider(&normal).unwrap();
        let admission = reserve(
            &db,
            &probe,
            "probe",
            "settings",
            crate::provider::probe_estimate(&probe),
            5.0,
            &[],
        )
        .unwrap();
        let refusal =
            admission.expect_err("the prior call still takes its full 100 + 4000 allowance");
        assert_eq!(refusal.detail, "4100/4100 tokens in 24 hours");
        assert!(matches!(refusal.skip, Skip::Budget(_)));
        assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 0);
        assert!(providers_db::token_ratios(&db, "p", 50).unwrap().is_empty());
        assert_eq!(
            providers_db::last_calls(&db, 1).unwrap(),
            ["p curator ok 1ms "]
        );
    }

    #[test]
    fn settlement_keeps_the_admission_month() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let p = entry(
            "p",
            Limits {
                max_output_tokens: 100,
                usd_per_mtok_in: 4.5,
                usd_per_mtok_out: 4.5,
                ..Default::default()
            },
        );
        let earlier = reserve(&db, &p, "curator", "earlier", 100, 0.001, &[])
            .unwrap()
            .unwrap();
        // Age a fixture's admission across the calendar boundary; settlement must not move
        // spending into a pool where other work has already reserved the remaining allowance.
        db.execute("UPDATE provider_calls SET ts=0 WHERE span='earlier'", [])
            .unwrap();
        let _this_month = reserve(&db, &p, "probe", "current", 100, 0.001, &[])
            .unwrap()
            .unwrap();
        let usd = earlier.cost(&p, Usage::default(), true);
        earlier
            .settle(
                &db,
                &Call {
                    provider: "p",
                    role: "curator",
                    span: "earlier",
                    outcome: "ok",
                    ms: 1,
                    detail: None,
                    bytes_out: 1,
                    est_tokens: Some(100),
                    usage: Usage::default(),
                    usd,
                },
                None,
                |_| Default::default(),
            )
            .unwrap();
        assert!((providers_db::usd_this_month(&db).unwrap() - 0.0009).abs() < 1e-12);
    }

    #[test]
    fn a_late_response_cannot_replenish_a_live_rate_window() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let peer = open(home.path()).unwrap();
        let p = entry("p", Limits::default());
        let first = reserve(&db, &p, "curator", "s", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let second = reserve(&peer, &p, "probe", "s", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let until = crate::db::now_ms() + 60_000;
        let actual = providers_db::RateLeft {
            requests: Some(0),
            requests_reset_at: Some(until),
            tokens: Some(0),
            tokens_reset_at: Some(until),
        };
        let stale = providers_db::RateLeft {
            requests: Some(5),
            tokens: Some(10_000),
            ..actual
        };
        for (reservation, role, rate) in [(second, "probe", actual), (first, "curator", stale)] {
            reservation
                .settle(
                    &db,
                    &Call {
                        provider: "p",
                        role,
                        span: "s",
                        outcome: "ok",
                        ms: 1,
                        detail: None,
                        bytes_out: 1,
                        est_tokens: Some(100),
                        usage: Usage::default(),
                        usd: None,
                    },
                    Some(rate),
                    |_| Default::default(),
                )
                .unwrap();
        }
        let refused = reserve(&peer, &p, "probe", "next", 100, 5.0, &[]).unwrap();
        assert!(
            matches!(refused, Err(Refusal { skip: Skip::Wait(t), .. }) if t == until),
            "{refused:?}"
        );
        assert_eq!(providers_db::rate(&peer, "p").unwrap(), actual);
    }

    #[test]
    fn reservations_share_the_reported_rate_allowance() {
        for rate in [
            providers_db::RateLeft {
                requests: Some(1),
                requests_reset_at: Some(crate::db::now_ms() + 60_000),
                ..Default::default()
            },
            providers_db::RateLeft {
                tokens: Some(1_500),
                tokens_reset_at: Some(crate::db::now_ms() + 60_000),
                ..Default::default()
            },
        ] {
            let home = tempfile::tempdir().unwrap();
            let db = open(home.path()).unwrap();
            let peer = open(home.path()).unwrap();
            let p = entry("p", Limits::default());
            providers_db::set_rate(&db, "p", rate).unwrap();
            let _inflight = reserve(&db, &p, "curator", "first", 100, 5.0, &[])
                .unwrap()
                .unwrap();
            let refusal = reserve(&peer, &p, "probe", "second", 100, 5.0, &[]).unwrap();
            assert!(
                matches!(
                    refusal,
                    Err(Refusal {
                        skip: Skip::Wait(_),
                        ..
                    })
                ),
                "{refusal:?}"
            );
            assert_eq!(providers_db::tokens_since(&peer, "p", 0).unwrap().0, 0);
            assert_eq!(factor(&peer, "p").unwrap(), 1.0);
        }
    }

    #[test]
    fn concurrent_reservations_share_daily_calls_tokens_and_the_curation_month() {
        for limit in ["calls", "tokens", "month"] {
            let home = tempfile::tempdir().unwrap();
            let db = open(home.path()).unwrap();
            let peer = open(home.path()).unwrap();
            let mut p = entry(
                "p",
                Limits {
                    daily_tokens: (limit == "tokens").then_some(350),
                    max_output_tokens: 100,
                    usd_per_mtok_in: 1.0,
                    usd_per_mtok_out: 1.0,
                    ..Default::default()
                },
            );
            if limit == "calls"
                && let Provider::Openai { daily_budget, .. } = &mut p
            {
                *daily_budget = Some(1);
            }
            let mut other = p.clone();
            if limit == "month"
                && let Provider::Openai { name, .. } = &mut other
            {
                *name = "other".into();
            }
            let cap = if limit == "month" { 0.0003 } else { 5.0 };
            let barrier = std::sync::Barrier::new(2);
            let (first, second) = std::thread::scope(|scope| {
                let first = scope.spawn(|| {
                    let db = db;
                    barrier.wait();
                    reserve(&db, &p, "curator", "worker", 100, cap, &[]).unwrap()
                });
                let second = scope.spawn(|| {
                    let peer = peer;
                    barrier.wait();
                    reserve(&peer, &other, "probe", "settings", 100, cap, &[]).unwrap()
                });
                (first.join().unwrap(), second.join().unwrap())
            });
            let results = [first, second];
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{limit}");
            assert!(
                results.iter().any(|r| matches!(
                    r,
                    Err(Refusal {
                        skip: Skip::Wait(_),
                        ..
                    })
                )),
                "{limit}"
            );
            // Let the token go without settling, as when a caller crashes. Reopening does not
            // forgive it, and hypothetical tokens cannot alter calibration or reported usage.
            drop(results);
            let reopened = open(home.path()).unwrap();
            assert!((providers_db::usd_this_month(&reopened).unwrap() - 0.0002).abs() < 1e-12);
            assert_eq!(providers_db::embed_usd_this_month(&reopened).unwrap(), 0.0);
            assert_eq!(providers_db::tokens_since(&reopened, "p", 0).unwrap().0, 0);
            assert!(
                providers_db::token_ratios(&reopened, "p", 50)
                    .unwrap()
                    .is_empty()
            );
            let refusal = reserve(&reopened, &p, "curator", "after-crash", 100, cap, &[]).unwrap();
            assert!(
                matches!(
                    refusal,
                    Err(Refusal {
                        skip: Skip::Wait(_),
                        ..
                    })
                ),
                "{limit}: {refusal:?}"
            );
        }
    }

    #[test]
    fn settlement_releases_unbilled_cost_and_keeps_actual_or_missing_usage_conservative() {
        for (sent, billed, usage, expected_usd, expected_tokens) in [
            (false, false, Usage::default(), 0.0, 0),
            (true, false, Usage::default(), 0.0, 0),
            (true, true, Usage::default(), 0.0002, 0),
            (
                true,
                true,
                Usage {
                    prompt: Some(20),
                    completion: Some(10),
                    ..Default::default()
                },
                0.00003,
                30,
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let db = open(home.path()).unwrap();
            let peer = open(home.path()).unwrap();
            let p = entry(
                "p",
                Limits {
                    max_output_tokens: 100,
                    usd_per_mtok_in: 1.0,
                    usd_per_mtok_out: 1.0,
                    ..Default::default()
                },
            );
            let reservation = reserve(&db, &p, "probe", "s", 100, 0.0003, &[])
                .unwrap()
                .unwrap();
            let usd = reservation.cost(&p, usage, billed);
            let hold = providers_db::State {
                down_until: providers_db::OWNER_HOLD,
                fails: 2,
                backoff: 3,
            };
            providers_db::set_state(&peer, "p", hold).unwrap();
            let state = reservation
                .settle(
                    &db,
                    &Call {
                        provider: "p",
                        role: "probe",
                        span: "s",
                        outcome: "error",
                        ms: 1,
                        detail: Some(if billed {
                            "http request: timeout: global"
                        } else {
                            "http 400"
                        }),
                        bytes_out: usize::from(sent),
                        est_tokens: Some(100),
                        usage,
                        usd,
                    },
                    None,
                    |_| Default::default(),
                )
                .unwrap();
            assert_eq!(state, hold);
            assert_eq!(providers_db::state(&peer, "p").unwrap(), hold);
            assert_eq!(providers_db::calls_in_a_day(&peer, "p").unwrap().0, 1);
            assert!((providers_db::usd_this_month(&peer).unwrap() - expected_usd).abs() < 1e-12);
            assert_eq!(
                providers_db::tokens_since(&peer, "p", 0).unwrap().0,
                expected_tokens
            );
            assert_eq!(
                providers_db::reserved_since(&peer, "p", 0).unwrap().calls,
                0
            );
            assert!(
                reserve(&peer, &p, "probe", "held", 100, 5.0, &[])
                    .unwrap()
                    .is_err()
            );
        }
    }

    #[test]
    fn the_estimate_counts_cjk_and_other_characters_apart() {
        assert_eq!(estimate(""), 0);
        assert_eq!(estimate("日本語"), 3); // 2.4, rounded up
        assert_eq!(estimate("abcd"), 2); // 1.12
        // 12,000 Japanese-heavy characters come out near what Groq counted for them (6,500).
        let ja = "日付の列が dd.mm.yyyy 形式の行が落ちている。".repeat(400);
        assert!((6_000..8_000).contains(&estimate(&ja)), "{}", estimate(&ja));
    }

    #[test]
    fn the_factor_follows_recorded_usage() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        for _ in 0..4 {
            call(&db, "p", Some(100), 150, 1);
        }
        assert_eq!(factor(&db, "p").unwrap(), 1.0); // fewer than 5
        call(&db, "p", Some(100), 120, 1);
        call(&db, "p", None, 999, 1); // no estimate: not a sample
        assert_eq!(factor(&db, "p").unwrap(), 1.5);
    }

    #[test]
    fn a_request_over_the_ceiling_or_after_a_peer_refused_it_is_not_admitted() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let groq = entry(
            "groq",
            Limits {
                max_request_tokens: Some(8000),
                ..Default::default()
            },
        );
        assert!(admit(&db, &groq, 6700.0, 5.0, &[]).unwrap().is_none());
        // With no output declared, its largest answer is reserved: 7,000 and 1,250 are over 8,000.
        let r = admit(&db, &groq, 7000.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.outcome, "too_big");
        let r = admit(&db, &groq, 7700.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.outcome, "too_big"); // over 95% of the ceiling
        let r = admit(&db, &groq, 100.0, 5.0, &[4000, 8000])
            .unwrap()
            .unwrap();
        assert_eq!(r.outcome, "too_big");
        assert!(admit(&db, &groq, 100.0, 5.0, &[4000]).unwrap().is_none());
        // A declared output is reserved too: 7,000 in and 4,000 out do not fit in 8,000.
        let mut declared = groq.clone();
        if let Provider::Openai { extra, .. } = &mut declared {
            extra.insert("max_completion_tokens".into(), 4000.into());
        }
        let r = admit(&db, &declared, 7000.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.outcome, "too_big");
    }

    #[test]
    fn daily_tokens_are_a_budget() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let p = entry(
            "p",
            Limits {
                daily_tokens: Some(10_000),
                ..Default::default()
            },
        );
        call(&db, "p", None, 7000, 1000);
        // With no output declared, 1,250 are reserved: 700 in fits in the 2,000 left, 800 does not.
        assert!(admit(&db, &p, 700.0, 5.0, &[]).unwrap().is_none());
        let r = admit(&db, &p, 800.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(
            (r.outcome, r.detail.as_str()),
            ("budget", "8000/10000 tokens in 24 hours")
        );
        // The declared output is reserved: 1,000 in and up to 4,000 out do not fit in 2,000.
        let mut declared = p.clone();
        if let Provider::Openai { extra, .. } = &mut declared {
            extra.insert("max_tokens".into(), 4000.into());
        }
        let r = admit(&db, &declared, 1000.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.outcome, "budget");
        // A sent call with no usage back counts at its estimate and its largest output: 1,000
        // in and the entry's 4,000 out take the day to 13,000.
        record(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "s",
                outcome: "error",
                ms: 1,
                detail: Some("http request: timeout: global"),
                bytes_out: 1,
                est_tokens: Some(1_000),
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let r = admit(&db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.detail, "13000/10000 tokens in 24 hours");
    }

    /// Groq's day is a rolling window: a call 23 hours ago still counts, one 25 hours ago does
    /// not, and a refused call waits until the oldest counted one leaves the 24 hours.
    #[test]
    fn daily_tokens_are_counted_over_the_last_24_hours() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let p = entry(
            "p",
            Limits {
                daily_tokens: Some(10_000),
                ..Default::default()
            },
        );
        let (now, hour) = (crate::db::now_ms(), 3_600_000);
        let aged = |ms: i64| {
            db.execute(
                "UPDATE provider_calls SET ts = ?1 WHERE id = (SELECT MAX(id) FROM provider_calls)",
                [now - ms],
            )
            .unwrap();
        };
        for ago in [25, 23] {
            call(&db, "p", None, 5000, 1000);
            aged(ago * hour);
        }
        // A refusal and a 429 older than the counted call used no token: their age frees none.
        refusal(&db, "p");
        aged(23 * hour + hour / 2);
        record(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "s",
                outcome: "wait",
                ms: 1,
                detail: Some("http 429: slow down"),
                bytes_out: 1,
                est_tokens: Some(900),
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        aged(23 * hour + hour / 4);
        // 6,000 in the 24 hours: 2,700 in and 1,250 reserved fit in the 4,000 left, 2,800 do not.
        assert!(admit(&db, &p, 2700.0, 5.0, &[]).unwrap().is_none());
        let r = admit(&db, &p, 2800.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.detail, "6000/10000 tokens in 24 hours");
        let Skip::Budget(until) = r.skip else {
            panic!("{:?}", r.skip)
        };
        assert_eq!(until, now - 23 * hour + providers_db::DAY_MS + 1);
    }

    /// The call budget counts a rolling day too, and waits until its oldest call leaves it.
    /// #238: an OpenRouter free entry with no budget of the owner's takes a fifth of the limit its
    /// key's last read gave, and 10 until a read gives one; admit counts calls against that, and
    /// doctor says which it is. The owner's own budget holds whatever the key says, and a read of
    /// a key the entry no longer holds is none.
    #[test]
    fn a_key_budget_is_a_fifth_of_what_its_last_read_gave() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let key_file = home.path().join("KEY.md");
        std::fs::write(&key_file, "# a test key\nkey-a\n").unwrap();
        let free = format!(
            "kind = \"openai\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m:free\"\n\
             key_file = {key_file:?}\n"
        );
        let p: Provider = toml::from_str(&format!("name = \"o\"\n{free}")).unwrap();
        let k = crate::provider::key_of(&p);
        let k = k.as_str();
        let said = |db: Option<&Connection>| key_budget(db, &p).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 10);
        assert_eq!(
            said(None),
            "10 calls a day until its key's own limit is read"
        );
        assert_eq!(said(Some(&db)), said(None));
        let read_at = crate::db::now_ms();
        providers_db::set_key_limit(&db, "o", Some(1000), read_at, k).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 200);
        assert_eq!(
            said(Some(&db)),
            format!(
                "200 calls a day, a fifth of the 1000 free-model requests a day its key has (read {})",
                crate::db::utc(read_at)
            )
        );
        // A read past due: doctor runs no refresh, and says the next run takes it again.
        providers_db::set_key_limit(&db, "o", Some(1000), 1, k).unwrap();
        assert_eq!(
            said(Some(&db)),
            format!(
                "200 calls a day, a fifth of the 1000 free-model requests a day its key has (read {}); \
                 the next curation run reads it again",
                crate::db::utc(1)
            )
        );
        // doctor reads without the chain's refresh: a read of the key the file held before is
        // not this key's.
        std::fs::write(&key_file, "# a test key\nkey-b\n").unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 10);
        assert_eq!(
            said(Some(&db)),
            "10 calls a day until its key's own limit is read"
        );
        std::fs::write(&key_file, "# a test key\nkey-a\n").unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 200);
        let own: Provider =
            toml::from_str(&format!("name = \"o\"\n{free}daily_budget = 30\n")).unwrap();
        assert_eq!(daily(&db, &own).unwrap(), 30);
        providers_db::set_key_limit(&db, "o", None, read_at, k).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 10);
        assert_eq!(
            said(Some(&db)),
            format!(
                "10 calls a day: the read of its key's own limit at {} gave none",
                crate::db::utc(read_at)
            )
        );
        providers_db::set_key_limit(&db, "o", Some(0), 3, k).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 0);
        assert!(admit(&db, &p, 10.0, 5.0, &[]).unwrap().is_some());
        // A refusal lasts until the next read of the key's limit is due, if that comes before the
        // oldest call leaves the day: the read may raise the budget (Codex on 53b7277). A read
        // that gave a limit is due after a day, one that gave none after an hour.
        let now = crate::db::now_ms();
        let hour = 3_600_000;
        call(&db, "o", None, 10, 10);
        let refused = |read_at: i64, limit: Option<u32>| {
            providers_db::set_key_limit(&db, "o", limit, read_at, k).unwrap();
            let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
            let Skip::Budget(until) = r.skip else {
                panic!("{:?}", r.skip)
            };
            (r.detail, until)
        };
        let (detail, day) = refused(now, Some(5));
        assert_eq!(detail, "1/1 calls in 24 hours");
        assert!(day > now + 23 * hour, "{day}");
        let at = now - 20 * hour;
        assert_eq!(refused(at, Some(5)).1, at + KEY_READ_HOLDS_MS);
        (0..9).for_each(|_| call(&db, "o", None, 10, 10));
        let (detail, until) = refused(now, None);
        assert_eq!(detail, "10/10 calls in 24 hours");
        assert_eq!(until, now + KEY_READ_RETRY_MS);
        assert!(until < day);
    }

    #[test]
    fn daily_calls_are_counted_over_the_last_24_hours() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let mut p = entry("p", Limits::default());
        if let Provider::Openai { daily_budget, .. } = &mut p {
            *daily_budget = Some(2);
        }
        let (now, hour) = (crate::db::now_ms(), 3_600_000);
        for ago in [25, 23] {
            call(&db, "p", None, 10, 10);
            db.execute(
                "UPDATE provider_calls SET ts = ?1 WHERE id = (SELECT MAX(id) FROM provider_calls)",
                [now - ago * hour],
            )
            .unwrap();
        }
        assert!(admit(&db, &p, 10.0, 5.0, &[]).unwrap().is_none());
        call(&db, "p", None, 10, 10);
        let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.detail, "2/2 calls in 24 hours");
        let Skip::Budget(until) = r.skip else {
            panic!("{:?}", r.skip)
        };
        assert_eq!(until, now - 23 * hour + providers_db::DAY_MS + 1);
    }

    #[test]
    fn a_call_whose_largest_output_would_cross_the_cap_is_not_admitted() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let paid = |name: &str| {
            entry(
                name,
                Limits {
                    usd_per_mtok_in: 1.0,
                    usd_per_mtok_out: 10.0,
                    max_output_tokens: 4000,
                    ..Default::default()
                },
            )
        };
        let chain = [paid("a"), paid("b"), entry("free", Limits::default())];
        // USD 4.63 spent across both paid entries; the free one does not count.
        paid_call(&db, &chain[0], 1_000_000, 300_000);
        paid_call(&db, &chain[1], 630_000, 0);
        paid_call(&db, &chain[2], 50_000_000, 5_000_000);
        let spent = providers_db::usd_this_month(&db).unwrap();
        assert!((spent - 4.63).abs() < 1e-9, "{spent}");
        // 10,000 in (0.01) and up to 4,000 out (0.04): 4.68, inside 5.
        assert!(admit(&db, &chain[0], 10_000.0, 5.0, &[]).unwrap().is_none());
        // Just below the cap, the same call could cross it by its answer alone.
        paid_call(&db, &chain[1], 0, 33_000);
        let r = admit(&db, &chain[1], 10_000.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.outcome, "budget");
        assert!(admit(&db, &chain[2], 10_000.0, 5.0, &[]).unwrap().is_none());
        // An entry that asks for 100 output tokens is priced at 100 (0.011 in all): it fits.
        let mut small = chain[1].clone();
        if let Provider::Openai { extra, .. } = &mut small {
            extra.insert("max_completion_tokens".into(), 100.into());
        }
        assert!(admit(&db, &small, 10_000.0, 5.0, &[]).unwrap().is_none());
    }

    #[test]
    fn a_sent_call_that_reported_no_usage_is_charged_its_largest_cost_and_keeps_it() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let paid = entry(
            "a",
            Limits {
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 10.0,
                max_output_tokens: 4000,
                ..Default::default()
            },
        );
        let none = Usage::default();
        let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);
        // A timeout after sending: 1,000 in (0.001) and up to 4,000 out (0.04).
        let timeout = cost(&db, &paid, 1_000, none, true).unwrap();
        assert!(close(timeout, 0.041), "{timeout:?}");
        // An answer with an HTTP error status was not billed.
        assert_eq!(cost(&db, &paid, 1_000, none, false).unwrap(), None);
        // A prompt count only: 500 in (0.0005) and the most out (0.04).
        let partial = Usage {
            prompt: Some(500),
            ..Default::default()
        };
        assert!(close(
            cost(&db, &paid, 1_000, partial, true).unwrap(),
            0.0405
        ));
        // A free entry has no cost.
        let free = entry("f", Limits::default());
        assert_eq!(cost(&db, &free, 1_000, none, true).unwrap(), None);
        // The timeout's cost is stored with it and stays when the entry is removed, repriced or
        // calibrated later: five samples at twice the estimate raise a new call's input to 2,000,
        // not the stored one's.
        record(
            &db,
            &Call {
                provider: "a",
                role: "curator",
                span: "s",
                outcome: "error",
                ms: 1,
                detail: Some("http request: timeout: global"),
                bytes_out: 1,
                est_tokens: Some(1_000),
                usage: none,
                usd: timeout,
            },
        )
        .unwrap();
        for _ in 0..5 {
            call(&db, "a", Some(100), 200, 0);
        }
        assert!(close(cost(&db, &paid, 1_000, none, true).unwrap(), 0.042));
        assert!(close(
            Some(providers_db::usd_this_month(&db).unwrap()),
            0.041
        ));
    }
}
