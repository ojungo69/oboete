//! The judge (docs/my-work-model.md J1): Typesafe's Jev, asked outside the curator chain at one
//! fixed endpoint. What it is shown goes through the outbound gate, a conversation of an excluded
//! repository is never sent, and each call is a row of the provider ledger, inside the month's
//! paid cap with every paid entry. A failed call is not made again: what it would have decided is
//! left to the model that decides it without a judge, as claude-mem leaves it.

use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::config::{self, Limits, Provider};
use crate::providers_db::{self, Usage};
use crate::raw::Raw;
use crate::redact::Rules;

/// Typesafe's endpoint (J1): no value of `[judge]` sends anything elsewhere.
pub(crate) const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// The judge's rows in the provider ledger, a name no chain entry has (`config::JUDGE_PREFIX`).
pub(crate) const NAME: &str = "judge:jev";
pub(crate) const ROLE: &str = "judge";
/// USD a million input tokens; the answer is free (J1).
const USD_PER_MTOK_IN: f64 = 0.042;
/// The answer's bound in the ledger's reservation; free, so it only bounds the count.
const OUTPUT_BOUND: u32 = 256;

#[cfg(test)]
thread_local! {
    /// A loopback URL a test calls in the endpoint's place.
    pub(crate) static AT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// One request: `state` and `questions`, keyed by ids and question names, never by recorded text
/// (keys are not gated), and the repositories `state` comes from.
pub(crate) struct Ask<'a> {
    pub span: &'a str,
    pub state: Value,
    pub questions: Value,
    pub repos: &'a [String],
}

#[derive(Debug, PartialEq)]
pub(crate) enum Asked {
    /// Each question's answer by its name, checked against its question.
    Answered(Map<String, Value>),
    /// Why nothing was answered: `off`, `key`, `excluded`, `masked` (a key the gate would change),
    /// a budget refusal's outcome, or a sent call's (`error`, `shape`).
    Skipped(&'static str),
}

/// The judge as an entry of the budget (`budget::reserve`): its daily calls and its price.
fn entry(j: &config::Judge) -> Provider {
    Provider::Openai {
        name: NAME.into(),
        enabled: j.enabled,
        base_url: ENDPOINT.into(),
        api: config::Api::Openai,
        key_file: Some(j.key_file.clone()),
        model: j.model.clone(),
        daily_budget: Some(j.daily_budget),
        timeout_s: j.timeout_s,
        retry_429: false,
        extra: Map::new(),
        headers: Default::default(),
        limits: Limits {
            usd_per_mtok_in: USD_PER_MTOK_IN,
            usd_per_mtok_out: 0.0,
            max_output_tokens: OUTPUT_BOUND,
            ..Limits::default()
        },
        subscription: false,
    }
}

/// Ask `j` once. `gate` is the caller's egress gate (the chain's `Gate`): asked after the
/// reservation and right before the request; its error cancels the reservation and is returned,
/// so the caller reads again what changed.
pub(crate) fn ask(
    db: &Connection,
    raw: &Raw,
    rules: &Rules,
    (j, paid_usd_per_month): (&config::Judge, f64),
    mut ask: Ask,
    gate: &crate::provider::Gate,
) -> Result<Asked> {
    if !j.enabled {
        return Ok(Asked::Skipped("off"));
    }
    let Ok(key) = config::read_key(&j.key_file) else {
        return Ok(Asked::Skipped("key"));
    };
    if excluded(raw, ask.repos)? {
        return Ok(Asked::Skipped("excluded"));
    }
    if !(gated(&mut ask.state, rules) && gated(&mut ask.questions, rules)) {
        return Ok(Asked::Skipped("masked"));
    }
    let body = json!({"model": j.model, "state": ask.state, "questions": ask.questions});
    let text = body.to_string();
    let est = crate::budget::estimate(&text);
    let p = entry(j);
    let reservation =
        match crate::budget::reserve(db, &p, ROLE, ask.span, est, paid_usd_per_month, &[])? {
            Ok(reservation) => reservation,
            Err(refusal) => {
                // As the chain: a state-only skip is no attempt.
                if refusal.outcome != "gate" {
                    providers_db::record(
                        db,
                        &providers_db::Call {
                            provider: NAME,
                            role: ROLE,
                            span: ask.span,
                            outcome: refusal.outcome,
                            ms: 0,
                            detail: Some(&refusal.detail),
                            bytes_out: 0,
                            est_tokens: Some(est),
                            usage: Usage::default(),
                            usd: None,
                        },
                    )?;
                }
                return Ok(Asked::Skipped(refusal.outcome));
            }
        };
    let admission = match gate() {
        Ok(admission) => admission,
        Err(e) => {
            reservation.cancel(db)?;
            return Err(e);
        }
    };
    // Again under the gate's admission: an exclusion written since the first look still holds. A
    // read that fails sends nothing either, and leaves no reservation (#763).
    let now = excluded(raw, ask.repos);
    if !matches!(now, Ok(false)) {
        drop(admission);
        reservation.cancel(db)?;
        return now.map(|_| Asked::Skipped("excluded"));
    }
    let start = Instant::now();
    let sent = post(j, &key, &body, admission);
    let ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);
    let (outcome, detail, answers, usage, billed) = match sent {
        Ok(out) => {
            let count = |v: &Value| v.as_i64().filter(|n| *n >= 0);
            let usage = Usage {
                prompt: count(&out["usage"]["input_tokens"]),
                completion: count(&out["usage"]["output_tokens"]),
                ..Usage::default()
            };
            // Typesafe's schema requires both counts: an answer without them is not its own. A
            // count that is there is still the cost (`Reservation::cost` bounds only the other).
            match answered(&ask.questions, &out) {
                _ if usage.prompt.is_none() || usage.completion.is_none() => {
                    ("shape", Some("usage".to_owned()), None, usage, true)
                }
                Some(answers) => ("ok", None, Some(answers), usage, true),
                None => ("shape", Some("answers".to_owned()), None, usage, true),
            }
        }
        Err((outcome, detail, billed)) => (outcome, Some(detail), None, Usage::default(), billed),
    };
    let detail = detail.as_deref();
    let usd = reservation.cost(&p, usage, billed);
    reservation.settle(
        db,
        &providers_db::Call {
            provider: NAME,
            role: ROLE,
            span: ask.span,
            outcome,
            ms,
            detail,
            bytes_out: text.len(),
            est_tokens: Some(est),
            usage,
            usd,
        },
        None,
        |s| s,
    )?;
    Ok(answers.map_or(Asked::Skipped(outcome), Asked::Answered))
}

/// Doctor's line: the judge's switch, its key file, and its calls in the last 24 hours.
pub(crate) fn doctor_line(home: &std::path::Path, judge: Option<&config::Judge>) -> String {
    let Some(j) = judge else {
        return "judge (Jev): off, no key set ([judge]; set one on the settings page)".to_owned();
    };
    if !j.enabled {
        return "judge (Jev): off ([judge] enabled = false)".to_owned();
    }
    if config::read_key(&j.key_file).is_err() {
        return format!(
            "judge (Jev): on, but {} has no key on line 2: nothing is sent",
            j.key_file.display()
        );
    }
    let calls = match providers_db::read_only(home) {
        Ok(Some(db)) => providers_db::calls_in_a_day(&db, NAME).map(|(calls, _)| calls),
        Ok(None) => Ok(0),
        Err(e) => Err(e),
    };
    match calls {
        Ok(calls) => format!(
            "judge (Jev): on, {calls} of {} calls in the last 24 hours",
            j.daily_budget
        ),
        Err(_) => {
            "judge (Jev): on; its calls unknown: the provider ledger does not read".to_owned()
        }
    }
}

/// Whether `j` is still the judge `[judge]` says now, on, with the same key file: a switch turned
/// off since the round read the settings sends nothing more (J1). One that does not read is not.
pub(crate) fn still_on(home: &std::path::Path, j: &config::Judge) -> bool {
    (config::load(home).ok().and_then(|c| c.judge))
        .is_some_and(|now| now.enabled && now.key_file == j.key_file)
}

/// Whether a repository of `repos` is kept from being sent, or never recorded, now (spec 5.5).
fn excluded(raw: &Raw, repos: &[String]) -> Result<bool> {
    let excluded = raw.exclusions_now()?;
    Ok(repos.iter().any(|r| excluded.contains(r)))
}

/// Every string of `v` through the outbound gate. Keys are ids and names, which a choice answers
/// with, so they cannot be masked: false when the gate would change one, and nothing is sent.
fn gated(v: &mut Value, rules: &Rules) -> bool {
    match v {
        Value::String(s) => {
            *s = crate::redact::outbound_with(s, rules);
            true
        }
        Value::Array(a) => a.iter_mut().all(|v| gated(v, rules)),
        Value::Object(o) => o
            .iter_mut()
            .all(|(k, v)| crate::redact::outbound_with(k, rules) == *k && gated(v, rules)),
        _ => true,
    }
}

/// The answers of `out` when each question has one of its own type: a choice among its criteria
/// with its own probability, probabilities of criteria only, and a `noul`, each within 0 to 1.
/// The judges ask only these two types.
fn answered(questions: &Value, out: &Value) -> Option<Map<String, Value>> {
    let answers = out["answers"].as_object()?;
    let unit = |v: &Value| v.as_f64().is_some_and(|x| (0.0..=1.0).contains(&x));
    for (name, q) in questions.as_object()? {
        let a = answers.get(name)?;
        let kind = q["type"].as_str()?;
        let fits = a["type"].as_str() == Some(kind)
            && match kind {
                "noul" => unit(&a["noul"]),
                "choice" => {
                    let criteria = q["criteria"].as_object()?;
                    let ps = a["probabilities"].as_object()?;
                    a["choice"]
                        .as_str()
                        .is_some_and(|c| criteria.contains_key(c) && ps.contains_key(c))
                        && ps.iter().all(|(k, v)| criteria.contains_key(k) && unit(v))
                }
                _ => false,
            };
        if !fits {
            return None;
        }
    }
    Some(answers.clone())
}

/// One POST with no redirect followed: the answer as JSON, or the outcome, a detail of its status
/// or kind (no error body is kept: it can echo what was sent) and whether it may have been billed.
fn post(
    j: &config::Judge,
    key: &str,
    body: &Value,
    admission: Option<crate::dispatch::Guard>,
) -> std::result::Result<Value, (&'static str, String, bool)> {
    #[cfg(test)]
    let url = AT
        .with_borrow(Clone::clone)
        .unwrap_or_else(|| ENDPOINT.into());
    #[cfg(not(test))]
    let url = ENDPOINT.to_owned();
    let timeout = Duration::from_secs(j.timeout_s);
    let req = crate::provider::admitted_agent(&url, timeout, 0, admission.as_ref())
        .post(&url)
        .header("Authorization", &format!("Bearer {key}"));
    let resp = match &admission {
        Some(admission) => crate::dispatch::json(req, body, admission),
        None => req.send_json(body),
    };
    let mut resp = resp.map_err(|e| match e {
        ureq::Error::Timeout(_) => ("error", "timeout".to_owned(), true),
        _ => ("error", "transport".to_owned(), true),
    })?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(("error", format!("http {status}"), false));
    }
    let max = crate::provider::MAX_RESPONSE_BYTES;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(resp.body_mut().as_reader(), max + 1),
        &mut raw,
    )
    .map_err(|_| ("error", "transport".to_owned(), true))?;
    if raw.len() as u64 > max {
        return Err(("shape", "too large".to_owned(), true));
    }
    serde_json::from_slice(&raw).map_err(|_| ("shape", "json".to_owned(), true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::tests::{request_body, serve_each};
    use std::sync::mpsc::Receiver;

    const KEY: &str = "TESTKEY_for_the_stub";

    struct Home {
        dir: tempfile::TempDir,
        raw: Raw,
        db: Connection,
        judge: config::Judge,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("TYPESAFE_KEY.md");
        std::fs::write(&key_file, format!("# a test key\n{KEY}\n")).unwrap();
        Home {
            raw: crate::raw::open(dir.path()).unwrap(),
            db: providers_db::open(dir.path()).unwrap(),
            judge: config::Judge {
                enabled: true,
                key_file,
                model: "jev-latest".into(),
                timeout_s: 5,
                daily_budget: 300,
            },
            dir,
        }
    }

    fn rules(toml: &str) -> Rules {
        Rules::new(&config::parse_capture(Some(toml)).unwrap().redaction).unwrap()
    }

    /// A conversation and a choice among two requests, `g7` and `g8`, or none.
    fn asked(repos: &[String]) -> Ask<'_> {
        Ask {
            span: "c1",
            state: json!({
                "conversation": {"worktree": "github.com/o/r", "asks": ["fix codename-blue's save"]},
                "requests": {"g7": {"title": "Fix the save"}, "g8": {"title": "Tidy the logs"}},
            }),
            questions: json!({"pick": {"type": "choice",
                "instructions": "Which request does `conversation` continue?",
                "criteria": {"g7": "`requests.g7`: Fix the save", "g8": "`requests.g8`: Tidy the logs",
                    "none": "None of these"}}}),
            repos,
        }
    }

    fn picked(choice: &str) -> Vec<u8> {
        json!({"model": "jev-1.14.0",
            "answers": {"pick": {"type": "choice", "choice": choice, "confidence": 0.7,
                "probabilities": {"g7": 0.8, "g8": 0.15, "none": 0.05}}},
            "usage": {"input_tokens": 1_000_000, "output_tokens": 12}})
        .to_string()
        .into_bytes()
    }

    fn run(h: &Home, rules: &Rules, cap: f64, ask_: Ask) -> Result<Asked> {
        ask(&h.db, &h.raw, rules, (&h.judge, cap), ask_, &|| Ok(None))
    }

    /// The stub at `answers`, called in the endpoint's place.
    fn stub(answers: Vec<(&'static str, Vec<u8>)>, headers: &'static str) -> Receiver<String> {
        let (url, rx) = serve_each(answers, headers);
        AT.set(Some(url));
        rx
    }

    fn requests(rx: &Receiver<String>) -> usize {
        std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(300)).ok()).count()
    }

    /// Every row of the ledger, each column as text.
    fn ledger(db: &Connection) -> Vec<String> {
        let mut st = db
            .prepare(
                "SELECT provider, role, span, outcome, COALESCE(detail, ''), prompt_tokens,
                        completion_tokens, usd FROM provider_calls ORDER BY id",
            )
            .unwrap();
        st.query_map([], |r| {
            Ok(format!(
                "{} {} {} {} {} {:?} {:?} {:?}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<f64>>(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// J1: one request to the endpoint with the key, every recorded string gated (the ids are keys,
    /// kept as they are), the answer checked, its input priced at USD 0.042 a million, and one
    /// settled row in the ledger.
    #[test]
    fn an_answer_is_gated_sent_once_priced_and_recorded() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let rules = rules(
            "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n",
        );
        let answered = run(&h, &rules, 5.0, asked(&[])).unwrap();
        let Asked::Answered(answers) = answered else {
            panic!("{answered:?}")
        };
        assert_eq!(answers["pick"]["choice"], "g7");
        let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.starts_with("POST "), "{request}");
        assert!(
            request.contains(&format!("authorization: Bearer {KEY}"))
                || request.contains(&format!("Authorization: Bearer {KEY}"))
        );
        let sent = request_body(&request);
        assert_eq!(sent["model"], "jev-latest");
        let text = sent.to_string();
        assert!(!text.contains("codename-blue"), "{text}");
        assert!(sent["state"]["requests"].get("g7").is_some(), "{text}");
        assert!(sent["questions"]["pick"]["criteria"].get("g7").is_some());
        assert_eq!(requests(&rx), 0);
        assert_eq!(
            ledger(&h.db),
            ["judge:jev judge c1 ok  Some(1000000) Some(12) Some(0.042)"]
        );
    }

    /// Spec 5.5: a repository kept from being sent, or never recorded, is not sent: no request,
    /// no row.
    #[test]
    fn an_excluded_repository_is_never_sent() {
        for capture in [false, true] {
            let mut h = home();
            let rx = stub(vec![("200 OK", picked("g7"))], "");
            if capture {
                h.raw
                    .exclude_capture(("repo", "github.com/o/r"), false)
                    .unwrap();
            } else {
                h.raw.exclude("github.com/o/r", false).unwrap();
            }
            let repos = ["github.com/o/r".to_owned()];
            let answered = run(&h, &rules(""), 5.0, asked(&repos)).unwrap();
            assert_eq!(answered, Asked::Skipped("excluded"), "capture {capture}");
            assert_eq!(requests(&rx), 0, "capture {capture}");
            assert!(ledger(&h.db).is_empty(), "capture {capture}");
        }
    }

    /// A key the gate would change (a rule that matches an id, a question's name or a criterion)
    /// is never sent: no request, no row.
    #[test]
    fn a_key_the_gate_would_mask_is_never_sent() {
        for rule in ["g7", "pick", "none"] {
            let h = home();
            let rx = stub(vec![("200 OK", picked("g7"))], "");
            let rules = rules(&format!(
                "[redaction]\nextra_rules = [{{ id = \"ids\", regex = \"^{rule}$\" }}]\n"
            ));
            assert_eq!(
                run(&h, &rules, 5.0, asked(&[])).unwrap(),
                Asked::Skipped("masked"),
                "{rule}"
            );
            assert_eq!(requests(&rx), 0, "{rule}");
            assert!(ledger(&h.db).is_empty(), "{rule}");
        }
    }

    /// An exclusion written after the first look, before the request, still holds: the second look
    /// is under the gate's admission. Nothing is sent and the reservation is gone.
    #[test]
    fn an_exclusion_written_while_the_call_waits_holds() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let gate = || {
            let mut raw = crate::raw::open(h.dir.path())?;
            raw.exclude("github.com/o/r", false)?;
            Ok(None)
        };
        let repos = ["github.com/o/r".to_owned()];
        let answered = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&repos),
            &gate,
        )
        .unwrap();
        assert_eq!(answered, Asked::Skipped("excluded"));
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// #763: a second look at the exclusions that fails returns its error, sends nothing and
    /// leaves no reservation.
    #[test]
    fn a_second_exclusion_read_that_fails_leaves_no_reservation() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        // A repository never recorded makes the read load the capture settings from config.toml.
        h.raw
            .exclude_capture(("repo", "github.com/o/other"), false)
            .unwrap();
        let config = h.dir.path().join("config.toml");
        let gate = || {
            std::fs::write(&config, "[capture\n")?;
            Ok(None)
        };
        let repos = ["github.com/o/r".to_owned()];
        let answered = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&repos),
            &gate,
        );
        assert!(answered.is_err(), "{answered:?}");
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// The daily calls and the month's paid cap refuse before anything is sent, each with a row.
    #[test]
    fn the_daily_calls_and_the_paid_cap_refuse_with_a_row_and_no_request() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        assert_eq!(
            run(&h, &rules(""), 0.0, asked(&[])).unwrap(),
            Asked::Skipped("budget")
        );
        h.judge.daily_budget = 0;
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("budget")
        );
        assert_eq!(requests(&rx), 0);
        let rows = ledger(&h.db);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter()
                .all(|r| r.starts_with("judge:jev judge c1 budget ")),
            "{rows:?}"
        );
    }

    /// No retry and no redirect: an error status, a 429 and a redirect are one request each, its
    /// body never kept; a redirect's target receives nothing.
    #[test]
    fn a_failed_call_is_one_request_with_no_body_kept() {
        let (target, seen) = serve_each(vec![("200 OK", picked("g7"))], "");
        let location: &'static str = Box::leak(format!("Location: {target}/\r\n").into_boxed_str());
        for (status, headers) in [
            ("500 Internal Server Error", ""),
            ("429 Too Many Requests", "Retry-After: 0\r\n"),
            ("307 Temporary Redirect", location),
        ] {
            let h = home();
            let body = br#"{"error": "echo: fix codename-blue's save"}"#.to_vec();
            let rx = stub(vec![(status, body), ("200 OK", picked("g7"))], headers);
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(answered, Asked::Skipped("error"), "{status}");
            assert_eq!(requests(&rx), 1, "{status}");
            let code = &status[..3];
            assert_eq!(
                ledger(&h.db),
                [format!(
                    "judge:jev judge c1 error http {code} None None None"
                )],
                "{status}"
            );
        }
        assert_eq!(requests(&seen), 0);
    }

    /// An answer that does not fit its question is `shape`, still billed at the reported input.
    #[test]
    fn an_answer_that_does_not_fit_is_shape() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g9"))], "");
        let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
        assert_eq!(answered, Asked::Skipped("shape"));
        assert_eq!(requests(&rx), 1);
        assert_eq!(
            ledger(&h.db),
            ["judge:jev judge c1 shape answers Some(1000000) Some(12) Some(0.042)"]
        );
    }

    /// Typesafe's schema requires both token counts: without either, or with one negative or not
    /// a whole number, the answer is `shape`, its input priced as reported, else at the
    /// reservation's estimate; and what it cost counts in the month's cap.
    #[test]
    fn an_answer_without_its_token_counts_is_shape() {
        let estimated = "None None Some(4.662e-6)";
        for (usage, kept) in [
            (json!(null), estimated),
            (
                json!({"input_tokens": 1_000_000}),
                "Some(1000000) None Some(0.042)",
            ),
            (json!({"output_tokens": 12}), "None Some(12) Some(4.662e-6)"),
            (
                json!({"input_tokens": -1, "output_tokens": 12}),
                "None Some(12) Some(4.662e-6)",
            ),
            (
                json!({"input_tokens": 10.5, "output_tokens": 12}),
                "None Some(12) Some(4.662e-6)",
            ),
        ] {
            let h = home();
            let mut answer: Value = serde_json::from_slice(&picked("g7")).unwrap();
            answer["usage"] = usage.clone();
            let rx = stub(vec![("200 OK", answer.to_string().into_bytes())], "");
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(answered, Asked::Skipped("shape"), "{usage}");
            assert_eq!(requests(&rx), 1);
            let rows = ledger(&h.db);
            assert_eq!(rows.len(), 1, "{rows:?}");
            // The reservation's bounds stay with a call of unknown usage (`freeze_unmetered`).
            let row = &rows[0];
            assert!(row.starts_with("judge:jev judge c1 shape "), "{row}");
            assert!(row.contains("usage") && row.ends_with(kept), "{row}");
            if kept.ends_with("Some(0.042)") {
                // USD 0.042 spent: a cap of 0.04 takes no further call.
                assert_eq!(
                    run(&h, &rules(""), 0.04, asked(&[])).unwrap(),
                    Asked::Skipped("budget")
                );
            }
        }
    }

    /// The egress gate is asked right before the request: its error is returned, nothing is sent
    /// and the reservation is gone.
    #[test]
    fn a_gate_error_sends_nothing_and_leaves_no_row() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let gate = || Err(anyhow::anyhow!("the list changed"));
        let e = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&[]),
            &gate,
        )
        .unwrap_err();
        assert_eq!(e.to_string(), "the list changed");
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// Off, or no key in its file: nothing is reserved or sent.
    #[test]
    fn off_or_without_a_key_nothing_goes() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        h.judge.enabled = false;
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("off")
        );
        h.judge.enabled = true;
        std::fs::write(&h.judge.key_file, "only a note\n").unwrap();
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("key")
        );
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
        drop(h.dir);
    }

    /// Doctor's line names each state without the key.
    #[test]
    fn doctor_names_the_judges_state() {
        let mut h = home();
        assert!(doctor_line(h.dir.path(), None).contains("off, no key set"));
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (Jev): on, 0 of 300 calls in the last 24 hours"
        );
        h.judge.enabled = false;
        assert!(
            doctor_line(h.dir.path(), Some(&h.judge)).contains("off ([judge] enabled = false)")
        );
        h.judge.enabled = true;
        std::fs::write(&h.judge.key_file, "a note only\n").unwrap();
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert!(line.contains("has no key on line 2"), "{line}");
        assert!(!line.contains(KEY));
    }

    #[test]
    fn an_answer_fits_its_question_or_is_refused() {
        let questions = asked(&[]).questions;
        let mut noul = json!({"same": {"type": "noul", "instructions": "Same work?",
            "criteria": {"true": "Same", "false": "Different"}}});
        let fits = |q: &Value, a: Value| answered(q, &json!({"answers": a})).is_some();
        let choice = |choice: &str, p: f64| {
            json!({"pick": {"type": "choice", "choice": choice,
                "probabilities": {"g7": p, "none": 0.1}}})
        };
        assert!(fits(&questions, choice("g7", 0.9)));
        assert!(fits(&questions, choice("none", 0.0)));
        assert!(!fits(&questions, choice("g9", 0.9)));
        assert!(!fits(&questions, choice("g7", 1.5)));
        assert!(
            !fits(&questions, choice("g8", 0.9)),
            "no probability of its own"
        );
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "choice", "choice": "g7", "probabilities": {}}})
        ));
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "choice", "choice": "g7"}})
        ));
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "noul", "noul": 0.5}})
        ));
        assert!(!fits(&questions, json!({})));
        assert!(fits(&noul, json!({"same": {"type": "noul", "noul": 0.96}})));
        assert!(!fits(
            &noul,
            json!({"same": {"type": "noul", "noul": -0.1}})
        ));
        noul["same"]["type"] = "score".into();
        assert!(!fits(
            &noul,
            json!({"same": {"type": "score", "score": 1.0}})
        ));
    }
}
