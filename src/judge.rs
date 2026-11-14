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
    /// Nothing was sent: d1 on this PC's check refused what listens at its address (#786), for
    /// this reason. Its ledger row is an `error` with the reason as its detail.
    Refused(&'static str),
}

impl Asked {
    /// What a judge records of a call that gave no answer: why, a refusal before sending being an
    /// `error`, as its ledger row is (#791).
    pub(crate) fn unanswered(&self) -> &'static str {
        match self {
            Asked::Answered(_) => "ok",
            Asked::Skipped(why) => why,
            Asked::Refused(_) => "error",
        }
    }
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
    let (sent, refused) = send(j, (&url, key.as_deref()), &body, admission);
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
            // No byte went before a refusal (#786).
            bytes_out: refused.map_or(text.len(), |_| 0),
            est_tokens: Some(est),
            usage,
            usd,
        },
        None,
        |s| s,
    )?;
    let unanswered = refused.map_or(Asked::Skipped(outcome), Asked::Refused);
    Ok(answers.map_or(unanswered, Asked::Answered))
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

/// `body` posted to `url`, but to d1 on this PC only when the program listening at its port may
/// be asked (#786): else no request is made. What `post` answers, and why the check refused when
/// it did.
fn send(
    j: &config::Judge,
    (url, key): (&str, Option<&str>),
    body: &Value,
    admission: Option<crate::dispatch::Guard>,
) -> (Posted, Option<&'static str>) {
    let port = || url.parse::<ureq::http::Uri>().ok()?.port_u16();
    let allowed = match j.destination {
        Destination::D1Local => port().map_or(Err(listener::UNREAD), listener::ours),
        _ => Ok(()),
    };
    match allowed {
        Ok(()) => (post(j, (url, key), body, admission), None),
        Err(why) => (Err(("error", why.to_owned(), false)), Some(why)),
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
mod tests;
