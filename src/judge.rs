//! The judge (docs/my-work-model.md J1, J7): a decision model asked outside the curator chain at
//! one of the addresses fixed here, Typesafe's Jev unless the owner chose another of the list. What
//! it is shown goes through the outbound gate, a conversation of an excluded repository is never
//! sent, and each call is a row of the provider ledger under its destination's name, inside the
//! month's paid cap with every paid entry. A failed call is not made again: what it would have
//! decided is left to the model that decides it without a judge, as claude-mem leaves it.

use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::config::{self, Destination, Limits, Provider};
use crate::providers_db::{self, Usage};
use crate::raw::Raw;
use crate::redact::Rules;

mod listener;

/// The addresses of the list (J7, decision 63); no value of `[judge]` sends anything elsewhere.
/// Typesafe's, Jev's (J1).
const JEV: &str = "https://api.typesafe.ai/v1/systemone";
/// Cloudflare's Workers AI, an account's: the account id and the model follow.
const CLOUDFLARE: &str = "https://api.cloudflare.com/client/v4/accounts/";
/// Liquid's.
const LIQUID: &str = "https://api.liquid.ai/decisions/v1/systemone";
/// d1 on this PC: Ollama's port, which llama.cpp's server can be started on too.
pub(crate) const LOCAL: &str = "127.0.0.1:11434";
/// Jev's rows in the provider ledger. Each destination's name starts `judge:`, as no chain entry's
/// does (`config::JUDGE_PREFIX`).
pub(crate) const NAME: &str = "judge:jev";
pub(crate) const ROLE: &str = "judge";
/// The answer's bound in the ledger's reservation; free, so it only bounds the count.
const OUTPUT_BOUND: u32 = 256;

/// What the code fixes of a destination (J7): its rows' name in the provider ledger, its name in
/// evidence, doctor and the page, the model its address takes (none for Jev, whose is `[judge]
/// model`), its USD a million input tokens (the answer is free), and its key, none for d1 on this
/// PC.
pub(crate) struct Fixed {
    pub name: &'static str,
    pub label: &'static str,
    model: Option<&'static str>,
    pub usd_per_mtok_in: f64,
    pub key: Option<Key>,
}

/// A service's key: the `[judge]` field the settings page writes its file into, and the reading of
/// that field, together so that the key written is the key sent.
#[derive(Clone, Copy)]
pub(crate) struct Key {
    pub field: &'static str,
    file: fn(&config::Judge) -> Option<&std::path::Path>,
}

pub(crate) fn fixed(d: Destination) -> Fixed {
    let typesafe = Key {
        field: "key_file",
        file: |j| j.key_file.as_deref(),
    };
    let cloudflare = Key {
        field: "cloudflare_key_file",
        file: |j| j.cloudflare_key_file.as_deref(),
    };
    let liquid = Key {
        field: "liquid_key_file",
        file: |j| j.liquid_key_file.as_deref(),
    };
    let (name, label, model, usd_per_mtok_in, key) = match d {
        Destination::Jev => (NAME, "Jev", None, 0.042, Some(typesafe)),
        Destination::Clef => ("judge:clef", "Clef", Some("clef"), 0.24, Some(cloudflare)),
        Destination::ClefFlash => (
            "judge:clef-flash",
            "Clef-flash",
            Some("clef-flash"),
            0.038,
            Some(cloudflare),
        ),
        Destination::D1 => ("judge:d1", "d1 (Liquid)", Some("d1.1"), 0.02, Some(liquid)),
        Destination::D1Local => ("judge:d1-local", "d1 on this PC", Some("d1"), 0.0, None),
    };
    Fixed {
        name,
        label,
        model,
        usd_per_mtok_in,
        key,
    }
}

/// The file of the key destination `d` takes, as `j` names it; none for d1 on this PC.
pub(crate) fn key_file_of(j: &config::Judge, d: Destination) -> Option<&std::path::Path> {
    fixed(d).key.and_then(|key| (key.file)(j))
}

/// The file of the key `j`'s destination takes.
pub(crate) fn key_file(j: &config::Judge) -> Option<&std::path::Path> {
    key_file_of(j, j.destination)
}

/// Whether `d` is one of Cloudflare's: its address names the account, and its answer comes in an
/// envelope.
pub(crate) fn cloudflare(d: Destination) -> bool {
    matches!(d, Destination::Clef | Destination::ClefFlash)
}

/// The model selector `j` sends.
fn model(j: &config::Judge) -> &str {
    fixed(j.destination).model.unwrap_or(&j.model)
}

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

/// The judge as an entry of the budget (`budget::reserve`): its daily calls and its price, under
/// its destination's name.
fn entry(j: &config::Judge) -> Provider {
    let fixed = fixed(j.destination);
    Provider::Openai {
        name: fixed.name.into(),
        enabled: j.enabled,
        base_url: url(j).unwrap_or_default(),
        api: config::Api::Openai,
        key_file: key_file(j).map(std::path::Path::to_path_buf),
        model: model(j).into(),
        daily_budget: Some(j.daily_budget),
        timeout_s: j.timeout_s,
        retry_429: false,
        extra: Map::new(),
        headers: Default::default(),
        limits: Limits {
            usd_per_mtok_in: fixed.usd_per_mtok_in,
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
    let Some((url, key)) = target(j) else {
        return Ok(Asked::Skipped("key"));
    };
    if excluded(raw, ask.repos)? {
        return Ok(Asked::Skipped("excluded"));
    }
    if !(gated(&mut ask.state, rules) && gated(&mut ask.questions, rules)) {
        return Ok(Asked::Skipped("masked"));
    }
    let body = json!({"model": model(j), "state": ask.state, "questions": ask.questions});
    let name = fixed(j.destination).name;
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
                            provider: name,
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
    let (sent, bytes_out) = send(j, (&url, key.as_deref()), (&body, &text), admission);
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
            provider: name,
            role: ROLE,
            span: ask.span,
            outcome,
            ms,
            detail,
            bytes_out,
            est_tokens: Some(est),
            usage,
            usd,
        },
        None,
        |s| s,
    )?;
    Ok(answers.map_or(Asked::Skipped(outcome), Asked::Answered))
}

/// Doctor's line: the judge's destination (d1 on this PC with its address) and switch, what it
/// lacks, and its calls in the last 24 hours.
pub(crate) fn doctor_line(home: &std::path::Path, judge: Option<&config::Judge>) -> String {
    let Some(j) = judge else {
        return "judge (Jev): off, no key set ([judge]; set one on the settings page)".to_owned();
    };
    let label = fixed(j.destination).label;
    let at = match j.destination {
        Destination::D1Local => format!("judge ({label}, {LOCAL})"),
        _ => format!("judge ({label})"),
    };
    if !j.enabled {
        return format!("{at}: off ([judge] enabled = false)");
    }
    if let Some(missing) = missing(j) {
        return format!("{at}: on, but {missing}: nothing is sent");
    }
    let name = fixed(j.destination).name;
    let calls = match providers_db::read_only(home) {
        Ok(Some(db)) => providers_db::calls_in_a_day(&db, name).map(|(calls, _)| calls),
        Ok(None) => Ok(0),
        Err(e) => Err(e),
    };
    match calls {
        Ok(calls) if j.daily_budget == config::no_daily_cap() => {
            format!("{at}: on, {calls} calls in the last 24 hours, no cap of calls a day")
        }
        Ok(calls) => format!(
            "{at}: on, {calls} of {} calls in the last 24 hours",
            j.daily_budget
        ),
        Err(_) => format!("{at}: on; its calls unknown: the provider ledger does not read"),
    }
}

/// What `j`'s destination lacks to be asked, said without its key or its account id: none when
/// it has what it needs.
fn missing(j: &config::Judge) -> Option<String> {
    // Only Cloudflare's address can be missing: it names the account.
    if url(j).is_none() {
        return Some("no Cloudflare account id is set".to_owned());
    }
    let field = fixed(j.destination).key?.field;
    match key_file(j) {
        None => Some(format!("no key is set ([judge] {field})")),
        Some(file) if config::read_key(file).is_err() => {
            Some(format!("{} has no key on line 2", file.display()))
        }
        Some(_) => None,
    }
}

/// Whether `j` is on with what its destination needs to be asked (J7).
pub(crate) fn ready(j: &config::Judge) -> bool {
    j.enabled && target(j).is_some()
}

/// The address `j` sends to, its destination's: Cloudflare's names the account, only when its id
/// is 32 lowercase hex digits (`config::account_id`), so the host and the path stay the list's.
pub(crate) fn url(j: &config::Judge) -> Option<String> {
    match j.destination {
        Destination::Jev => Some(JEV.to_owned()),
        Destination::Clef | Destination::ClefFlash => {
            let account = (j.cloudflare_account_id.as_deref()).filter(|a| config::account_id(a))?;
            Some(format!(
                "{CLOUDFLARE}{account}/ai/run/@cf/cloudflare/{}",
                model(j)
            ))
        }
        Destination::D1 => Some(LIQUID.to_owned()),
        Destination::D1Local => Some(format!("http://{LOCAL}/v1/systemone")),
    }
}

/// Where `j` sends and the key it sends with (none for d1 on this PC): none when its key, or
/// Cloudflare's account id, is missing or does not read, and nothing goes.
fn target(j: &config::Judge) -> Option<(String, Option<String>)> {
    let url = url(j)?;
    #[cfg(test)]
    let url = AT.with_borrow(Clone::clone).unwrap_or(url);
    let key = match fixed(j.destination).key {
        Some(_) => Some(config::read_key(key_file(j)?).ok()?),
        None => None,
    };
    Some((url, key))
}

/// Whether `j` is still the judge `[judge]` says now, on, sending to the same address with the
/// same key file: a switch turned off, or another destination chosen, since the round read the
/// settings sends nothing more (J1, J7). One that does not read is not.
pub(crate) fn still_on(home: &std::path::Path, j: &config::Judge) -> bool {
    let sends = |j: &config::Judge| (j.destination, url(j), key_file(j).map(|f| f.to_owned()));
    (config::load(home).ok().and_then(|c| c.judge))
        .is_some_and(|now| now.enabled && sends(&now) == sends(j))
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

/// What `post` answers: the answer, or the outcome, its detail and whether it may have been billed.
type Posted = std::result::Result<Value, (&'static str, String, bool)>;

/// `body` (as `text`) posted to `url`, but to d1 on this PC only when the program listening at its
/// port may be asked (#786): else no request is made. What `post` answers, and the bytes out:
/// `text`'s length once a request went, none before.
fn send(
    j: &config::Judge,
    (url, key): (&str, Option<&str>),
    (body, text): (&Value, &str),
    admission: Option<crate::dispatch::Guard>,
) -> (Posted, usize) {
    let port = || url.parse::<ureq::http::Uri>().ok()?.port_u16();
    let allowed = match j.destination {
        Destination::D1Local => port().map_or(Err(listener::UNREAD), listener::ours),
        _ => Ok(()),
    };
    match allowed {
        Ok(()) => (post(j, (url, key), body, admission), text.len()),
        Err(why) => (Err(("error", why.to_owned(), false)), 0),
    }
}

/// One POST to `url`, with `key` when its destination takes one, and no redirect followed: the
/// answer as JSON (Cloudflare's from inside its envelope), or the outcome, a detail of its status
/// or kind (no error body is kept: it can echo what was sent) and whether it may have been billed.
fn post(
    j: &config::Judge,
    (url, key): (&str, Option<&str>),
    body: &Value,
    admission: Option<crate::dispatch::Guard>,
) -> Posted {
    let timeout = Duration::from_secs(j.timeout_s);
    let mut req = crate::provider::admitted_agent(url, timeout, 0, admission.as_ref()).post(url);
    if let Some(key) = key {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
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
    answer_of(j.destination, &raw).map_err(|detail| ("shape", detail.to_owned(), true))
}

/// The answer in `raw`, the body `d`'s address answered with: Cloudflare's from inside its
/// envelope, when the envelope says it succeeded (its REST API's form, called 2026-10-31). Else
/// why not: `json`, or Cloudflare's `envelope`.
fn answer_of(d: Destination, raw: &[u8]) -> std::result::Result<Value, &'static str> {
    let mut out: Value = serde_json::from_slice(raw).map_err(|_| "json")?;
    if !cloudflare(d) {
        return Ok(out);
    }
    (out["success"] == true && out["result"].is_object())
        .then(|| out["result"].take())
        .ok_or("envelope")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::tests::{request_body, serve_each};
    use std::sync::mpsc::Receiver;

    const KEY: &str = "TESTKEY_for_the_stub";
    const CF_KEY: &str = "CFTOKEN_for_the_stub";
    const LIQUID_KEY: &str = "liquid_KEY_for_the_stub";
    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

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
                destination: config::Destination::Jev,
                key_file: Some(key_file),
                cloudflare_key_file: None,
                cloudflare_account_id: None,
                liquid_key_file: None,
                model: "jev-latest".into(),
                timeout_s: 5,
                daily_budget: 300,
            },
            dir,
        }
    }

    /// `h`'s judge sending to `d`, each service's own key in a file of its own.
    fn sending_to(h: &mut Home, d: config::Destination) {
        let file = |name: &str, key: &str| {
            let file = h.dir.path().join(name);
            std::fs::write(&file, format!("# a test key\n{key}\n")).unwrap();
            file
        };
        let (cloudflare, liquid) = (file("CF_KEY.md", CF_KEY), file("LIQUID_KEY.md", LIQUID_KEY));
        h.judge.cloudflare_key_file = Some(cloudflare);
        h.judge.cloudflare_account_id = Some(ACCOUNT.into());
        h.judge.liquid_key_file = Some(liquid);
        h.judge.destination = d;
    }

    /// `answer` in Cloudflare's envelope, as its REST API answered on 2026-10-31.
    fn enveloped(answer: &[u8]) -> Vec<u8> {
        let answer: Value = serde_json::from_slice(answer).unwrap();
        json!({"result": answer, "success": true, "errors": [], "messages": []})
            .to_string()
            .into_bytes()
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

    /// Decision 64: with no cap of calls a day and no monthly cap, the judge asks.
    #[test]
    fn with_its_caps_off_the_judge_asks() {
        let mut h = home();
        h.judge.daily_budget = config::no_daily_cap();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let answered = run(&h, &rules(""), config::no_monthly_cap(), asked(&[])).unwrap();
        assert!(matches!(answered, Asked::Answered(_)), "{answered:?}");
        assert_eq!(requests(&rx), 1);
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
        std::fs::write(h.judge.key_file.as_ref().unwrap(), "only a note\n").unwrap();
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("key")
        );
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
        drop(h.dir);
    }

    /// J7 (decision 63): each destination's address is fixed here. Cloudflare's names the account
    /// id in its path only when it is 32 lowercase hex digits, so no value of `[judge]` reaches
    /// another host or path, and without one the judge is not ready.
    #[test]
    fn each_destination_is_a_fixed_address() {
        use config::Destination::*;
        let mut h = home();
        let cloudflare = |model: &str| {
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{ACCOUNT}/ai/run/@cf/cloudflare/{model}"
            )
        };
        for (d, at) in [
            (Jev, "https://api.typesafe.ai/v1/systemone".to_owned()),
            (Clef, cloudflare("clef")),
            (ClefFlash, cloudflare("clef-flash")),
            (
                D1,
                "https://api.liquid.ai/decisions/v1/systemone".to_owned(),
            ),
            (D1Local, "http://127.0.0.1:11434/v1/systemone".to_owned()),
        ] {
            sending_to(&mut h, d);
            assert_eq!(url(&h.judge).as_deref(), Some(at.as_str()), "{d:?}");
            assert!(ready(&h.judge), "{d:?}");
        }
        sending_to(&mut h, ClefFlash);
        for bad in [
            None,
            Some("0123456789ABCDEF0123456789abcdef"),
            Some("0123456789abcdef0123456789abcde"),
            Some("0123456789abcdef0123456789abcdef/../../zones"),
        ] {
            h.judge.cloudflare_account_id = bad.map(Into::into);
            assert_eq!(url(&h.judge), None, "{bad:?}");
            assert!(!ready(&h.judge), "{bad:?}");
        }
    }

    /// J7: each destination is asked with its own model selector and its own service's key (none
    /// for d1 on this PC), its answer read where its API puts it (Cloudflare's inside its
    /// envelope), and its call kept in the ledger under its own name at its own price.
    #[test]
    fn each_destination_sends_its_model_and_key_and_keeps_its_own_row() {
        use config::Destination::*;
        let row =
            |name: &str, usd: &str| format!("{name} judge c1 ok  Some(1000000) Some(12) {usd}");
        for (d, model, key, kept) in [
            (
                Jev,
                "jev-latest",
                Some(KEY),
                row("judge:jev", "Some(0.042)"),
            ),
            (Clef, "clef", Some(CF_KEY), row("judge:clef", "Some(0.24)")),
            (
                ClefFlash,
                "clef-flash",
                Some(CF_KEY),
                row("judge:clef-flash", "Some(0.038)"),
            ),
            (D1, "d1.1", Some(LIQUID_KEY), row("judge:d1", "Some(0.02)")),
            (D1Local, "d1", None, row("judge:d1-local", "None")),
        ] {
            let mut h = home();
            sending_to(&mut h, d);
            let answer = match d {
                Clef | ClefFlash => enveloped(&picked("g7")),
                _ => picked("g7"),
            };
            let rx = stub(vec![("200 OK", answer)], "");
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert!(
                matches!(answered, Asked::Answered(_)),
                "{d:?}: {answered:?}"
            );
            let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(request_body(&request)["model"], model, "{d:?}");
            let lower = request.to_lowercase();
            match key {
                Some(key) => {
                    let header = format!("authorization: bearer {}", key.to_lowercase());
                    assert!(lower.contains(&header), "{d:?}");
                }
                None => assert!(!lower.contains("authorization"), "{d:?}"),
            }
            assert_eq!(ledger(&h.db), [kept], "{d:?}");
        }
    }

    /// J7: Cloudflare's answer is the `result` of its envelope when `success` is true; another
    /// envelope, or an answer outside one, is not its own (`shape`), and is paid for as sent.
    #[test]
    fn cloudflare_s_envelope_is_read_or_refused() {
        let answer: Value = serde_json::from_slice(&picked("g7")).unwrap();
        for body in [
            json!({"result": answer, "success": false, "errors": [{"code": 3040}], "messages": []}),
            json!({"success": true, "errors": [], "messages": []}),
            answer.clone(),
        ] {
            let mut h = home();
            sending_to(&mut h, config::Destination::ClefFlash);
            let _rx = stub(vec![("200 OK", body.to_string().into_bytes())], "");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("shape"), "{body}");
            let kept = ledger(&h.db);
            assert_eq!(kept.len(), 1, "{kept:?}");
            let row = &kept[0];
            assert!(row.starts_with("judge:clef-flash judge c1 shape "), "{row}");
            assert!(row.contains("envelope"), "{row}");
        }
        // A body that is not JSON is `json`, from Cloudflare or any other.
        for d in [config::Destination::ClefFlash, config::Destination::Jev] {
            let mut h = home();
            sending_to(&mut h, d);
            let _rx = stub(vec![("200 OK", b"<html>busy</html>".to_vec())], "");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("shape"), "{d:?}");
            let row = &ledger(&h.db)[0];
            assert!(row.contains("json") && !row.contains("envelope"), "{row}");
        }
    }

    /// J7: a destination without what it needs sends nothing and keeps no row: Cloudflare's without
    /// its token or its account id, Liquid's without its key. d1 on this PC needs neither.
    #[test]
    fn a_destination_without_its_key_sends_nothing() {
        use config::Destination::*;
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        for (d, unset) in [(Clef, "token"), (ClefFlash, "account"), (D1, "key")] {
            sending_to(&mut h, d);
            match unset {
                "token" => h.judge.cloudflare_key_file = None,
                "account" => h.judge.cloudflare_account_id = None,
                _ => h.judge.liquid_key_file = None,
            }
            assert!(!ready(&h.judge), "{d:?}");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("key"), "{d:?}");
        }
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
        sending_to(&mut h, D1Local);
        (
            h.judge.key_file,
            h.judge.cloudflare_key_file,
            h.judge.liquid_key_file,
        ) = (None, None, None);
        assert!(ready(&h.judge));
    }

    /// #786: d1 on this PC is asked only when a program that may be asked listens at its port. With
    /// none there, nothing is sent: the call is an `error` that says why, and no byte went out.
    #[cfg(target_os = "linux")]
    #[test]
    fn d1_on_this_pc_with_no_listener_sends_nothing() {
        let mut h = home();
        sending_to(&mut h, config::Destination::D1Local);
        // Port 0: no socket ever listens there, so no other test's stub can take it meanwhile.
        AT.set(Some("http://127.0.0.1:0/v1/systemone".to_owned()));
        let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
        assert_eq!(asked_, Asked::Skipped("error"));
        assert_eq!(
            ledger(&h.db),
            ["judge:d1-local judge c1 error no listener None None None"]
        );
        let out: i64 = (h.db)
            .query_row("SELECT bytes_out FROM provider_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(out, 0);
    }

    /// J7: the round's judge is not the one `[judge]` names once its destination, or the key file
    /// or account id that destination reads, changed since the round read the settings.
    #[test]
    fn a_destination_switched_since_the_round_read_it_is_not_still_on() {
        use config::Destination::*;
        let mut h = home();
        sending_to(&mut h, ClefFlash);
        let path = |f: &Option<std::path::PathBuf>| f.as_ref().unwrap().display().to_string();
        let (config, jev) = (h.dir.path().join("config.toml"), path(&h.judge.key_file));
        let table = |destination: &str, account: &str, cloudflare: &str| {
            let text = format!(
                "providers = []\n[judge]\ndestination = \"{destination}\"\nkey_file = '{jev}'\n\
                 cloudflare_key_file = '{cloudflare}'\ncloudflare_account_id = \"{account}\"\n"
            );
            std::fs::write(&config, text).unwrap();
        };
        let cloudflare = path(&h.judge.cloudflare_key_file);
        table("clef-flash", ACCOUNT, &cloudflare);
        assert!(still_on(h.dir.path(), &h.judge));
        table("clef", ACCOUNT, &cloudflare);
        assert!(!still_on(h.dir.path(), &h.judge));
        table("clef-flash", &"f".repeat(32), &cloudflare);
        assert!(!still_on(h.dir.path(), &h.judge));
        table("clef-flash", ACCOUNT, &path(&h.judge.liquid_key_file));
        assert!(!still_on(h.dir.path(), &h.judge));
        // Jev's key file is not Clef-flash's: Jev's alone changed, it is still the same judge.
        table("clef-flash", ACCOUNT, &cloudflare);
        h.judge.key_file = None;
        assert!(still_on(h.dir.path(), &h.judge));
        sending_to(&mut h, D1Local);
        table("d1-local", ACCOUNT, &cloudflare);
        assert!(still_on(h.dir.path(), &h.judge));
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
        // Decision 64: no cap is said, not counted against 4294967295.
        h.judge.daily_budget = config::no_daily_cap();
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (Jev): on, 0 calls in the last 24 hours, no cap of calls a day"
        );
        h.judge.enabled = false;
        assert!(
            doctor_line(h.dir.path(), Some(&h.judge)).contains("off ([judge] enabled = false)")
        );
        h.judge.enabled = true;
        std::fs::write(h.judge.key_file.as_ref().unwrap(), "a note only\n").unwrap();
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert!(line.contains("has no key on line 2"), "{line}");
        assert!(!line.contains(KEY));
        // J7: the destination is named, d1 on this PC with the address it is sent to; one without
        // what it needs says so, its account id not repeated.
        h.judge.daily_budget = 300;
        sending_to(&mut h, config::Destination::D1Local);
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (d1 on this PC, 127.0.0.1:11434): on, 0 of 300 calls in the last 24 hours"
        );
        sending_to(&mut h, config::Destination::ClefFlash);
        h.judge.cloudflare_account_id = None;
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert_eq!(
            line,
            "judge (Clef-flash): on, but no Cloudflare account id is set: nothing is sent"
        );
        h.judge.cloudflare_account_id = Some(ACCOUNT.into());
        std::fs::write(h.judge.cloudflare_key_file.as_ref().unwrap(), "a note\n").unwrap();
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert!(line.starts_with("judge (Clef-flash): on, but "), "{line}");
        assert!(
            line.contains("has no key on line 2") && !line.contains(ACCOUNT),
            "{line}"
        );
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
