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
const CEILING_SHARE: f64 = 0.95;

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
    Ok(match read {
        Some((Some(limit), at, _)) => format!(
            "{budget} calls a day, a fifth of the {limit} free-model requests a day its key has (read {})",
            crate::db::utc(at)
        ),
        Some((None, at, _)) => format!(
            "{budget} calls a day: the read of its key's own limit at {} gave none",
            crate::db::utc(at)
        ),
        None => format!("{budget} calls a day until its key's own limit is read"),
    })
}

/// Whether `p` may take a request of `tokens` (calibrated) now. `ceiling_hit` is a ceiling a
/// provider answered 413 to earlier in this chain run: every entry with that ceiling is skipped.
pub fn admit(
    db: &Connection,
    p: &Provider,
    tokens: f64,
    paid_usd_per_month: f64,
    ceiling_hit: &[u32],
) -> Result<Option<Refusal>> {
    let name = p.name();
    let limits = p.limits();
    let now = crate::db::now_ms();
    // A rolling day, as `limits.daily_tokens` below: until the oldest call counted leaves it.
    let (used, oldest) = providers_db::calls_in_a_day(db, name)?;
    // ponytail: a key's budget (#238) counts the entry's calls, not its key's. Two entries on one
    // OpenRouter key take a fifth each, and a replaced key's calls count for a day. Record the
    // key_id with each call and count by it if an entry ever shares its key; the default has one.
    let budget = daily(db, p)?;
    if used >= budget {
        let mut until = providers_db::out_of_the_day(oldest.unwrap_or(now));
        // A key whose limit its last read did not give: the next read may raise the budget.
        if p.budget_from_key()
            && let Some((None, at, _)) = key_read(db, p)?
        {
            until = until.min(at + KEY_READ_RETRY_MS);
        }
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!("{used}/{budget} calls in 24 hours"),
            skip: Skip::Budget(until),
        }));
    }
    // The answer counts against the same limits as the prompt (Groq's TPM is input and output
    // together): the declared output, or with none declared, the largest answer seen.
    let output = match p.declared_output() {
        0 => UNDECLARED_OUTPUT,
        n => n,
    };
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
    if rate.requests == Some(0)
        && let Some(at) = rate.requests_reset_at.filter(|&t| t > now)
    {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: "no requests left until its reset".into(),
            skip: Skip::Wait(at),
        }));
    }
    if let (Some(left), Some(at)) = (rate.tokens, rate.tokens_reset_at)
        && at > now
        && (left as f64) < reserved
    {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!(
                "{left} tokens left until its reset in {} s",
                (at - now) / 1000
            ),
            skip: Skip::Wait(at),
        }));
    }
    // Counted over the last 24 hours: Groq's day is a rolling window (docs/milestone-1.md), and a
    // budget kept per UTC day could take twice its share around midnight.
    if let Some(daily) = limits.daily_tokens {
        let since = now - providers_db::DAY_MS;
        let (reported, oldest) = providers_db::tokens_since(db, name, since)?;
        let used = reported as f64 + unmetered(db, p, since)?;
        if used + reserved > daily as f64 {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!("{used:.0}/{daily} tokens in 24 hours"),
                // When the oldest call counted leaves the 24 hours.
                skip: Skip::Budget(providers_db::out_of_the_day(oldest.unwrap_or(now))),
            }));
        }
    }
    if limits.is_paid() {
        let spent = providers_db::usd_this_month(db)?;
        // The output the request asks for at most, as `provider::call` sends it.
        let this = limits.usd(tokens, f64::from(p.declared_output()));
        if spent + this > paid_usd_per_month {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!(
                    "USD {spent:.2} of {paid_usd_per_month:.2} spent this month; this call up to {this:.3}"
                ),
                skip: Skip::Budget(providers_db::next_month()),
            }));
        }
    }
    Ok(None)
}

/// What one call to a paid entry `p` cost, to be stored with it: the usage it reported, and for a
/// part it did not report, its largest (the calibrated estimate for the prompt, the declared
/// output for the answer). None for an entry that is not paid, or a call that was not billed.
pub fn cost(
    db: &Connection,
    p: &Provider,
    est: u32,
    usage: Usage,
    billed: bool,
) -> Result<Option<f64>> {
    let limits = p.limits();
    if !limits.is_paid() || !billed {
        return Ok(None);
    }
    let input = match usage.prompt {
        Some(n) => n as f64,
        None => f64::from(est) * factor(db, p.name())?,
    };
    let output = match usage.completion {
        Some(n) => n as f64,
        None => f64::from(largest_output(p)),
    };
    Ok(Some(limits.usd(input, output)))
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

/// The tokens `p`'s sent calls since `start` may have used and did not report: a missing prompt
/// count at its calibrated estimate, a missing completion count at its largest output.
fn unmetered(db: &Connection, p: &Provider, start: i64) -> Result<f64> {
    let (est, calls) = providers_db::unmetered(db, p.name(), start)?;
    Ok(est as f64 * factor(db, p.name())? + (calls * i64::from(largest_output(p))) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Limits;
    use crate::providers_db::{Call, Usage, open, record};

    fn entry(name: &str, limits: Limits) -> Provider {
        Provider::Openai {
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
        providers_db::set_key_limit(&db, "o", Some(1000), 1, k).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 200);
        assert_eq!(
            said(Some(&db)),
            format!(
                "200 calls a day, a fifth of the 1000 free-model requests a day its key has (read {})",
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
        providers_db::set_key_limit(&db, "o", None, 2, k).unwrap();
        assert_eq!(daily(&db, &p).unwrap(), 10);
        assert_eq!(
            said(Some(&db)),
            format!(
                "10 calls a day: the read of its key's own limit at {} gave none",
                crate::db::utc(2)
            )
        );
        providers_db::set_key_limit(&db, "o", Some(5), 3, k).unwrap();
        call(&db, "o", None, 10, 10);
        let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.detail, "1/1 calls in 24 hours");
        let Skip::Budget(day) = r.skip else {
            panic!("{:?}", r.skip)
        };
        // After a failed read, the refusal holds until the read is tried again, not for the day.
        let at = crate::db::now_ms();
        providers_db::set_key_limit(&db, "o", None, at, k).unwrap();
        (0..9).for_each(|_| call(&db, "o", None, 10, 10));
        let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(r.detail, "10/10 calls in 24 hours");
        let Skip::Budget(until) = r.skip else {
            panic!("{:?}", r.skip)
        };
        assert_eq!(until, at + KEY_READ_RETRY_MS);
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
