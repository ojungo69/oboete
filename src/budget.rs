//! What a provider may take before a call (docs/milestone-3-plan.md D7, Task 4): its daily calls
//! and tokens, a paid entry's share of the month's USD cap, and a request's size against the
//! provider's own ceiling, in estimated tokens. A refused call uploads nothing and costs nothing.

use anyhow::Result;
use rusqlite::Connection;

use crate::config::Provider;
use crate::providers_db;

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
}

/// A ceiling check keeps this share of the ceiling free: the estimate is not exact.
const CEILING_SHARE: f64 = 0.95;

/// Whether `p` may take a request of `tokens` (calibrated) now. `ceiling_hit` is a ceiling a
/// provider answered 413 to earlier in this chain run: every entry with that ceiling is skipped.
pub fn admit(
    db: &Connection,
    p: &Provider,
    providers: &[Provider],
    tokens: f64,
    paid_usd_per_month: f64,
    ceiling_hit: Option<u32>,
) -> Result<Option<Refusal>> {
    let name = p.name();
    let limits = p.limits();
    let used = providers_db::calls_today(db, name)?;
    if used >= p.daily_budget() {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!("{used}/{} calls today", p.daily_budget()),
        }));
    }
    if let Some(max) = limits.max_request_tokens {
        if ceiling_hit == Some(max) {
            return Ok(Some(Refusal {
                outcome: "too_big",
                detail: format!("an entry with the same {max}-token ceiling refused it"),
            }));
        }
        if tokens > f64::from(max) * CEILING_SHARE {
            return Ok(Some(Refusal {
                outcome: "too_big",
                detail: format!("about {tokens:.0} tokens, over its {max}"),
            }));
        }
    }
    let rate = providers_db::rate(db, name)?;
    let now = crate::db::now_ms();
    if rate.requests == Some(0) && rate.requests_reset_at.is_some_and(|t| t > now) {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: "no requests left until its reset".into(),
        }));
    }
    if let (Some(left), Some(at)) = (rate.tokens, rate.tokens_reset_at)
        && at > now
        && (left as f64) < tokens
    {
        return Ok(Some(Refusal {
            outcome: "budget",
            detail: format!(
                "{left} tokens left until its reset in {} s",
                (at - now) / 1000
            ),
        }));
    }
    if let Some(daily) = limits.daily_tokens {
        let today = providers_db::tokens_today(db, name)?;
        if today as f64 + tokens > daily as f64 {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!("{today}/{daily} tokens today"),
            }));
        }
    }
    if limits.is_paid() {
        let spent = spent_this_month(db, providers)?;
        let this = limits.usd(tokens, f64::from(limits.max_output_tokens));
        if spent + this > paid_usd_per_month {
            return Ok(Some(Refusal {
                outcome: "budget",
                detail: format!(
                    "USD {spent:.2} of {paid_usd_per_month:.2} spent this month; this call up to {this:.3}"
                ),
            }));
        }
    }
    Ok(None)
}

/// USD spent this calendar month (UTC) on every paid entry, at their current prices. A sent call
/// whose usage never came back counts at its estimate and its largest answer.
pub fn spent_this_month(db: &Connection, providers: &[Provider]) -> Result<f64> {
    let mut usd = 0.0;
    for p in providers.iter().filter(|p| p.limits().is_paid()) {
        let limits = p.limits();
        let (prompt, completion) = providers_db::tokens_this_month(db, p.name())?;
        usd += limits.usd(prompt as f64, completion as f64);
        let (est, calls) = providers_db::unmetered_this_month(db, p.name())?;
        usd += limits.usd(
            est as f64,
            (calls * i64::from(limits.max_output_tokens)) as f64,
        );
    }
    Ok(usd)
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
            daily_budget: 10,
            timeout_s: 1,
            retry_429: false,
            extra: Default::default(),
            headers: Default::default(),
            limits,
        }
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
        let chain = std::slice::from_ref(&groq);
        assert!(
            admit(&db, &groq, chain, 7000.0, 5.0, None)
                .unwrap()
                .is_none()
        );
        let r = admit(&db, &groq, chain, 7700.0, 5.0, None)
            .unwrap()
            .unwrap();
        assert_eq!(r.outcome, "too_big"); // over 95% of the ceiling
        let r = admit(&db, &groq, chain, 100.0, 5.0, Some(8000))
            .unwrap()
            .unwrap();
        assert_eq!(r.outcome, "too_big");
        assert!(
            admit(&db, &groq, chain, 100.0, 5.0, Some(4000))
                .unwrap()
                .is_none()
        );
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
        let chain = std::slice::from_ref(&p);
        assert!(admit(&db, &p, chain, 1500.0, 5.0, None).unwrap().is_none());
        let r = admit(&db, &p, chain, 2500.0, 5.0, None).unwrap().unwrap();
        assert_eq!(
            (r.outcome, r.detail.as_str()),
            ("budget", "8000/10000 tokens today")
        );
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
        call(&db, "a", None, 1_000_000, 300_000);
        call(&db, "b", None, 630_000, 0);
        call(&db, "free", None, 50_000_000, 5_000_000);
        assert!((spent_this_month(&db, &chain).unwrap() - 4.63).abs() < 1e-9);
        // 10,000 in (0.01) and up to 4,000 out (0.04): 4.68, inside 5.
        assert!(
            admit(&db, &chain[0], &chain, 10_000.0, 5.0, None)
                .unwrap()
                .is_none()
        );
        // Just below the cap, the same call could cross it by its answer alone.
        call(&db, "b", None, 0, 33_000);
        let r = admit(&db, &chain[1], &chain, 10_000.0, 5.0, None)
            .unwrap()
            .unwrap();
        assert_eq!(r.outcome, "budget");
        assert!(
            admit(&db, &chain[2], &chain, 10_000.0, 5.0, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_sent_call_that_reported_no_usage_counts_at_its_largest_cost() {
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
        // A timeout after sending: 1,000 in (0.001) and up to 4,000 out (0.04). An HTTP error
        // response, a 429 here, was not billed.
        for detail in ["a timed out after 90s", "http 429: rate_limit"] {
            record(
                &db,
                &Call {
                    provider: "a",
                    role: "curator",
                    span: "s",
                    outcome: "error",
                    ms: 1,
                    detail: Some(detail),
                    bytes_out: 1,
                    est_tokens: Some(1_000),
                    usage: Usage::default(),
                },
            )
            .unwrap();
        }
        let spent = spent_this_month(&db, &[paid]).unwrap();
        assert!((spent - 0.041).abs() < 1e-9, "{spent}");
    }
}
