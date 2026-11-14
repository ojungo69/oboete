//! What a provider may take before a call (docs/milestone-3-plan.md D7, Task 4): its daily calls
//! and tokens, a paid entry's share of the month's USD cap, and a request's size against the
//! provider's own ceiling, in estimated tokens. A refused call uploads nothing and costs nothing.

use crate::provider::{AttemptEvent, AttemptOutcome, Skip};
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
    /// Made for a codex entry that draws on credits (`provider::draws_on_credits`).
    credits: bool,
}

impl Reservation {
    /// The ledger row is unique while this reservation is pending, across chain instances.
    pub(crate) fn id(&self) -> i64 {
        self.id
    }

    /// Actual reported usage, with the admission-time bounds for any missing part. The tokens the
    /// provider's cache wrote and read are priced as `Provider::billed_input` prices them, never
    /// less than the bill; an input never reported is priced as admission priced it
    /// (`Provider::input_weight`; Codex on #409).
    pub(crate) fn cost(&self, p: &Provider, usage: Usage, billed: bool) -> Option<f64> {
        (billed && p.limits().is_paid()).then(|| {
            p.limits().usd(
                usage
                    .prompt
                    .map_or(self.input * p.input_weight(&self.role), |n| {
                        p.billed_input(&self.role, n, usage.cached, usage.written)
                    }),
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
        // An answer cannot implicitly resume an owner-held entry or shorten a concurrent rest,
        // which keeps its mark (`provider::rest_holds`). A rest codex's allowance read set
        // without credits gives way to a call that drew on them, which read the allowance itself
        // before it sent anything; any other call keeps it, as one may have been reserved before
        // the rest was written (Codex on #418).
        if !(current.uncredited && self.credits) {
            // A rest that holds every entry outranks one an entry with credits passes: an
            // uncredited rest kept here never takes the place of a credited one this call brings
            // as long (a safety hold among them; Codex on #418).
            let credited =
                |s: &providers_db::State| !s.uncredited && s.down_until >= current.down_until;
            if current.down_until == providers_db::OWNER_HOLD {
                let hold = credited(&updated);
                updated = current;
                updated.uncredited &= !hold;
            } else if current.down_until > crate::db::now_ms()
                && current.down_until >= updated.down_until
            {
                updated.uncredited = current.uncredited && !credited(&updated);
                updated.down_until = current.down_until;
            }
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
    /// A terminal observation follows this reservation's successful cancellation only.
    /// A failure or unwind leaves the observer's pending allowance unchanged.
    pub(crate) fn cancel_report(
        self,
        db: &Connection,
        observer: &mut impl FnMut(&AttemptEvent),
    ) -> Result<()> {
        let attempt = self.id;
        self.cancel(db)?;
        observer(&AttemptEvent::Cancelled { attempt });
        Ok(())
    }

    /// Keep native accounting authoritative and publish its own committed result.
    pub(crate) fn settle_report(
        self,
        db: &Connection,
        call: &providers_db::Call<'_>,
        rate: Option<providers_db::RateLeft>,
        next: impl FnOnce(providers_db::State) -> providers_db::State,
        sent: bool,
        observer: &mut impl FnMut(&AttemptEvent),
    ) -> Result<providers_db::State> {
        let attempt = self.id;
        let state = self.settle(db, call, rate, next)?;
        observer(&AttemptEvent::Settled {
            attempt,
            outcome: AttemptOutcome::from_ledger(call.outcome),
            sent,
            usd: call.usd,
            usage_unknown: sent && (call.usage.prompt.is_none() || call.usage.completion.is_none()),
        });
        Ok(state)
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
    let holds = crate::provider::rest_holds(&state, p);
    if holds && state.down_until == providers_db::OWNER_HOLD {
        return Ok(Err(Refusal {
            outcome: "gate",
            detail: format!(
                "stopped until the owner acts (`oboete resume {}`)",
                p.name()
            ),
            skip: Skip::Owner,
        }));
    }
    if holds && state.down_until > crate::db::now_ms() {
        return Ok(Err(Refusal {
            outcome: "gate",
            detail: "cooling down after an earlier failure".into(),
            skip: Skip::Wait(state.down_until),
        }));
    }
    let input = f64::from(est) * factor(&tx, p.name())?;
    if let Some(refusal) = admit_with_history(
        &tx,
        (p, history),
        role,
        input,
        paid_usd_per_month,
        ceiling_hit,
    )? {
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
            usd: p
                .limits()
                .is_paid()
                .then(|| p.limits().usd(input * p.input_weight(role), output)),
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
        credits: crate::provider::draws_on_credits(p),
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
    role: &str,
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
        // The output the request asks for at most, as `provider::call` sends it, and the input at
        // the most a token of it may be billed (`Provider::input_weight`).
        let this = limits.usd(tokens * p.input_weight(role), f64::from(output));
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
             AND ts>=?1",
            [providers_db::chrono_free_month_start(now)],
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
    db: Option<&Connection>,
    providers: &[Provider],
    tokens: u32,
    calls: usize,
) -> Result<Option<f64>> {
    most_usd_calibrated(db, providers, tokens, calls).map(|(usd, _)| usd)
}

/// The estimate and the exact per-entry factors used for its confirmation key.
pub(crate) fn most_usd_calibrated(
    db: Option<&Connection>,
    providers: &[Provider],
    tokens: u32,
    calls: usize,
) -> Result<(Option<f64>, Vec<f64>)> {
    let mut most: Option<f64> = None;
    let mut calibrations = Vec::new();
    for p in providers.iter().filter(|p| p.limits().is_paid()) {
        let calibration = match db {
            Some(db) => factor(db, p.name())?,
            None => 1.0,
        };
        calibrations.push(calibration);
        // Recuration's windows are the curator's.
        let input = f64::from(tokens) * calibration * p.input_weight("curator");
        let output = calls as f64 * f64::from(largest_output(p));
        let usd = p.limits().usd(input, output);
        most = Some(most.unwrap_or(0.0) + usd);
    }
    Ok((most, calibrations))
}

/// The answer a request may get at most: its declared output, or the entry's output cap.
pub(crate) fn largest_output(p: &Provider) -> u32 {
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
mod tests;
