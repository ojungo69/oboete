//! The viewer's settings page (#94): what it shows of config.toml, and its one write. The page
//! edits summary, spending, Gemini, injection, capture and chain settings; the rest of the file,
//! comments included, stays as it is (toml_edit). No key file's contents, header or `extra` value
//! goes into an answer, and an error's text never does either: it could quote a value from the
//! file (`config::toml_error`).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::{self, ChainOverlay, Provider, ToolOutput};

/// Why a save was refused: an HTTP status, a code the page puts in words, and the field it is
/// about ("chain.groq.timeout_s"), empty when it is about the whole request.
#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub field: String,
}

fn refused(status: u16, code: &'static str, field: impl Into<String>) -> Refusal {
    Refusal {
        status,
        code,
        field: field.into(),
    }
}

/// A file that does not read or parse: the page shows no form, and doctor gives the line.
fn invalid() -> Refusal {
    refused(422, "file_invalid", "")
}

const BUDGET: std::ops::RangeInclusive<u32> = 1..=100_000;
const TIMEOUT_S: std::ops::RangeInclusive<u64> = 5..=900;
const MODEL_CHARS: usize = 200;

/// config.toml's bytes, none when there is no file.
fn bytes(home: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(home.join("config.toml")) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// What a save names to replace the file: its bytes' SHA-256, "none" for no file.
fn version(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "none".to_owned(),
        |b| {
            Sha256::digest(b)
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect()
        },
    )
}

/// The file's text, "" for no file; none when it is not UTF-8.
fn utf8(bytes: Option<&[u8]>) -> Option<&str> {
    bytes.map_or(Some(""), |b| std::str::from_utf8(b).ok())
}

/// The file as every reader of it parses it, or none when one of them would refuse it: the
/// curators, capture and its redaction rules, injection, and backups (Codex on #270).
fn parsed(path: &Path, text: &str) -> Option<(config::Config, config::Capture, config::Inject)> {
    let capture = config::parse_capture(Some(text)).ok()?;
    crate::redact::Rules::new(&capture.redaction).ok()?;
    crate::backup::location(text).ok()?;
    Some((
        config::from_text(path, text).ok()?,
        capture.capture,
        config::parse_inject(text).ok()?,
    ))
}

/// The page's GET: the values as they apply, and each chain entry's own state.
pub fn show(home: &Path) -> Value {
    let path = home.join("config.toml");
    let Ok(bytes) = bytes(home) else {
        return json!({"version": "none", "error": "file_invalid"});
    };
    let version = version(bytes.as_deref());
    let read = utf8(bytes.as_deref()).and_then(|t| {
        let doc = t.parse::<toml_edit::DocumentMut>().ok()?;
        Some((parsed(&path, t)?, alone(&path, &doc)?))
    });
    let Some(((cfg, capture, inject), alone)) = read else {
        return json!({"version": version, "error": "file_invalid"});
    };
    let ledger = crate::providers_db::read_only(home);
    let db = ledger.as_ref().ok().and_then(Option::as_ref);
    let (spend, stopped) = match &ledger {
        Ok(Some(db)) => (
            crate::providers_db::usd_this_month(db).ok(),
            crate::providers_db::stopped(db).ok().map(|rows| {
                rows.into_iter()
                    .filter(|(_, until)| *until == crate::providers_db::OWNER_HOLD)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
            }),
        ),
        Ok(None) => (Some(0.0), Some(Vec::new())),
        Err(_) => (None, None),
    };
    // One row per name, as `[chain]` sets every entry of a name alike.
    let chain: Vec<Value> = names(&cfg.providers)
        .into_iter()
        .map(|name| {
            let same: Vec<&Provider> = (cfg.providers.iter())
                .filter(|p| p.name() == name)
                .collect();
            // Of several entries of one name, the strictest rule, each taken from the entry
            // without `[chain]`, as a save checks it (OpenCodeReview on #270).
            let rules: Vec<_> = (alone.providers.iter())
                .filter(|p| p.name() == name)
                .map(Provider::model_rule)
                .collect();
            let rule = [config::ModelRule::Fixed, config::ModelRule::Free]
                .into_iter()
                .find(|r| rules.contains(r))
                .unwrap_or(config::ModelRule::Any);
            entry(&same, &cfg.chain, rule, db)
        })
        .collect();
    json!({
        "version": version,
        "summary": {
            "curate": cfg.summary.curate,
            "language": cfg.summary.language,
            "window_tokens": cfg.summary.window_tokens,
            "idle_minutes": cfg.summary.idle_minutes,
        },
        "paid_usd_per_month": cfg.paid_usd_per_month,
        "gemini": cfg.gemini.map(gemini_place),
        "usd_this_month": spend,
        "stopped": stopped,
        "inject": {
            "session_start": inject.session_start,
            "session_start_chars": inject.session_start_chars,
            "per_prompt": inject.per_prompt,
            "per_prompt_chars": inject.per_prompt_chars,
            "correction": inject.correction,
            "correction_chars": inject.correction_chars,
        },
        "capture": {
            "store_prompts": capture.store_prompts,
            "tool_output": tool_output(capture.tool_output),
        },
        "chain": chain,
        // Where a key typed on the page can be written (#94 part 3; macOS and Windows: #281).
        "key_input": cfg!(target_os = "linux"),
        "warnings": cfg.warnings,
        // The page checks and words its fields by these, so they are stated once.
        "ranges": {
            "window_tokens": [0, u32::MAX],
            "idle_minutes": [0, u32::MAX],
            // No finite upper limit in config.toml; JSON cannot carry infinity.
            "paid_usd_per_month": [0, null],
            "session_start_chars": range(config::SESSION_START_CHARS),
            "per_prompt_chars": range(config::PER_PROMPT_CHARS),
            "correction_chars": range(config::CORRECTION_CHARS),
            "daily_budget": [BUDGET.start(), BUDGET.end()],
            "timeout_s": [TIMEOUT_S.start(), TIMEOUT_S.end()],
        },
    })
}

fn range(r: std::ops::RangeInclusive<usize>) -> [usize; 2] {
    [*r.start(), *r.end()]
}

fn tool_output(t: ToolOutput) -> &'static str {
    match t {
        ToolOutput::Full => "full",
        ToolOutput::HeadTail => "head-tail",
    }
}

fn gemini_place(place: config::GeminiPlace) -> &'static str {
    match place {
        config::GeminiPlace::BeforeSubscriptions => "before-subscriptions",
        config::GeminiPlace::AfterSubscriptions => "after-subscriptions",
    }
}

/// One chain entry for the page. `key` is the key file's state as doctor words it; its path is
/// shown so the user knows where the key goes, and its contents are never read into the answer
/// (a budget from the key reads it inside the process, to fingerprint it).
fn entry(
    same: &[&Provider],
    chain: &ChainOverlay,
    rule: config::ModelRule,
    db: Option<&Connection>,
) -> Value {
    let p = same[0];
    let name = p.name();
    let (kind, key, key_file, model, timeout_s) = match p {
        Provider::Openai {
            key_file,
            model,
            timeout_s,
            ..
        } => (
            "api",
            match key_file {
                Some(f) if f.exists() => "ok",
                Some(_) => "missing",
                None => "none",
            },
            key_file.as_ref().map(|f| f.display().to_string()),
            Some(model.as_str()),
            *timeout_s,
        ),
        Provider::Cli {
            cli,
            model,
            timeout_s,
            ..
        } => (
            "cli",
            if crate::setup::on_path(cli) {
                "on-path"
            } else {
                "not-on-path"
            },
            None,
            model.as_deref(),
            *timeout_s,
        ),
    };
    let budget_of = |q: &Provider| match db {
        Some(db) if q.budget_from_key() => crate::budget::daily(db, q).unwrap_or(q.daily_budget()),
        _ => q.daily_budget(),
    };
    let budget = budget_of(p);
    // Whether the row's entry takes the `[chain]` model: one `load()` left unset (a model the entry
    // cannot price) is shown as not applied, and a save that leaves it keeps it (#274).
    let applied = chain.model.get(name).map(String::as_str) == model;
    // The row shows the first entry's values: where entries of its name use different ones (with
    // `[chain]` applied), it says so (#274).
    let key_file_of = |p: &Provider| match p {
        Provider::Openai { key_file, .. } => key_file.clone(),
        Provider::Cli { .. } => None,
    };
    let model_of = |p: &Provider| match p {
        Provider::Openai { model, .. } => Some(model.clone()),
        Provider::Cli { model, .. } => model.clone(),
    };
    let timeout_of = |p: &Provider| match p {
        Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. } => *timeout_s,
    };
    let differs: Vec<&str> = [
        (
            "key_file",
            same.iter().any(|q| key_file_of(q) != key_file_of(p)),
        ),
        ("model", same.iter().any(|q| model_of(q) != model_of(p))),
        // The calls a day each uses: a key's read limit is its own (cubic on #287).
        ("daily_budget", same.iter().any(|q| budget_of(q) != budget)),
        (
            "timeout_s",
            same.iter().any(|q| timeout_of(q) != timeout_of(p)),
        ),
    ]
    .into_iter()
    .filter_map(|(field, d)| d.then_some(field))
    .collect();
    json!({
        "name": name,
        "entries": same.len(),
        "differs": differs,
        "kind": kind,
        "on": !chain.turns_off(name),
        "key": key,
        "key_file": key_file,
        "model": chain.model.get(name),
        "model_applied": applied,
        "effective_model": model,
        "model_rule": rule,
        "daily_budget": chain.daily_budget.get(name),
        "effective_daily_budget": (budget != config::no_daily_cap()).then_some(budget),
        "budget_from_key": p.budget_from_key(),
        "timeout_s": chain.timeout_s.get(name),
        "effective_timeout_s": timeout_s,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    version: String,
    summary: SummaryIn,
    paid_usd_per_month: f64,
    gemini: Option<config::GeminiPlace>,
    inject: InjectIn,
    capture: CaptureIn,
    /// Every chain entry once, in the order the page wants.
    chain: Vec<EntryIn>,
}

/// Only the summary fields the page edits; `shrink` stays as the file has it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryIn {
    curate: bool,
    language: String,
    window_tokens: u32,
    idle_minutes: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InjectIn {
    session_start: bool,
    session_start_chars: usize,
    per_prompt: bool,
    per_prompt_chars: usize,
    correction: bool,
    correction_chars: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureIn {
    store_prompts: bool,
    tool_output: ToolOutput,
}

/// A name's values; `None` follows the entries' own value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryIn {
    name: String,
    on: bool,
    daily_budget: Option<u32>,
    timeout_s: Option<u64>,
    model: Option<String>,
}

/// A key save's body (#94, part 3): the entry's name, the key, and the version of the config the
/// page showed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeySave {
    entry: String,
    key: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resume {
    provider: String,
}

/// The same operation as `oboete resume`: clear the stop and make owner-held work eligible
/// at the worker's next run. This request starts no worker and sends no provider call.
pub fn resume(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Resume =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if posted.provider.is_empty() {
        return Err(refused(422, "bad_entry", "provider"));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = || refused(503, "providers_unavailable", "");
    let mut db = crate::providers_db::open(home).map_err(|_| unavailable())?;
    // All the cooldown and retry changes succeed together or are rolled back together.
    let tx = db.transaction().map_err(|_| unavailable())?;
    let resumed = crate::providers_db::resume(&tx, &posted.provider).map_err(|_| unavailable())?;
    tx.commit().map_err(|_| unavailable())?;
    Ok(json!({"provider": posted.provider, "resumed": resumed}))
}

/// Writes a key typed on the page into the key file its entry names (`keyfile`), against the
/// config the page showed: a changed config is refused, and entries of one name are written only
/// when they name one file. The answer is the entry's key state: the key is in no answer.
pub fn save_key(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: KeySave =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if posted.entry.is_empty() || posted.entry.len() > 64 {
        return Err(refused(422, "bad_entry", "entry"));
    }
    let field = format!("chain.{}.key", posted.entry);
    if !crate::keyfile::valid(&posted.key) {
        return Err(refused(422, "bad_key", field));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = home.join("config.toml");
    let was = bytes(home).map_err(|_| invalid())?;
    if version(was.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    let text = utf8(was.as_deref()).ok_or_else(invalid)?;
    let (cfg, ..) = parsed(&path, text).ok_or_else(invalid)?;
    let mut files = (cfg.providers.iter())
        .filter(|p| p.name() == posted.entry)
        .map(|p| match p {
            Provider::Openai {
                key_file: Some(f), ..
            } => Some(f),
            _ => None,
        });
    let Some(first) = files.next() else {
        return Err(refused(404, "no_entry", field));
    };
    let Some(file) = first else {
        return Err(refused(422, "no_key_file", field));
    };
    if let Some(other) = files.find(|f| *f != Some(file)) {
        let code = if other.is_some() {
            "ambiguous"
        } else {
            "no_key_file"
        };
        return Err(refused(422, code, field));
    }
    let written = crate::keyfile::write(file, &posted.key, home)
        .map_err(|r| refused(r.status(), r.code(), field))?;
    Ok(json!({"entry": posted.entry, "key": "ok", "durable": written.durable}))
}

/// The page's save: checks the request against the file it read (`version`), writes the page's
/// keys into it and checks the result as every reader parses it before it replaces the file, and
/// answers the new `show`. A value that equals the entry's own is not written, so it keeps
/// following later changes to the defaults.
pub fn save(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Save = serde_json::from_slice(body).map_err(|e| match e.classify() {
        serde_json::error::Category::Data => refused(422, "type", ""),
        _ => refused(400, "bad_request", ""),
    })?;
    // Two tabs saving at once: one after the other, and the second finds the file changed.
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = home.join("config.toml");
    let was = bytes(home).map_err(|_| invalid())?;
    if version(was.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    let text = utf8(was.as_deref()).ok_or_else(invalid)?;
    let (now, capture, inject) = parsed(&path, text).ok_or_else(invalid)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid())?;
    let base = alone(&path, &doc).ok_or_else(invalid)?;
    let chain = checked(&posted, &base, &now)?;
    // The order changes when the chain's does, not when `order` would be spelled another way.
    let reordered = !(posted.chain.iter().map(|e| e.name.as_str())).eq(names(&now.providers));
    if posted.paid_usd_per_month != now.paid_usd_per_month {
        put_root(
            &mut doc,
            "paid_usd_per_month",
            Some(posted.paid_usd_per_month.into()),
        );
    }
    if posted.gemini != now.gemini {
        put_root(
            &mut doc,
            "gemini",
            posted.gemini.map(|p| gemini_place(p).into()),
        );
    }
    let s = &posted.summary;
    if s.curate != now.summary.curate {
        put(&mut doc, "summary", "curate", s.curate.into());
    }
    if s.language != now.summary.language {
        put(&mut doc, "summary", "language", s.language.as_str().into());
    }
    for (key, value, was) in [
        ("window_tokens", s.window_tokens, now.summary.window_tokens),
        ("idle_minutes", s.idle_minutes, now.summary.idle_minutes),
    ] {
        if value != was {
            put(&mut doc, "summary", key, i64::from(value).into());
        }
    }
    let i = &posted.inject;
    for (key, now, was) in [
        ("session_start", i.session_start, inject.session_start),
        ("per_prompt", i.per_prompt, inject.per_prompt),
        ("correction", i.correction, inject.correction),
    ] {
        if now != was {
            put(&mut doc, "inject", key, now.into());
        }
    }
    for (key, now, was) in [
        (
            "session_start_chars",
            i.session_start_chars,
            inject.session_start_chars,
        ),
        (
            "per_prompt_chars",
            i.per_prompt_chars,
            inject.per_prompt_chars,
        ),
        (
            "correction_chars",
            i.correction_chars,
            inject.correction_chars,
        ),
    ] {
        if now != was {
            put(&mut doc, "inject", key, (now as i64).into());
        }
    }
    let c = &posted.capture;
    if c.store_prompts != capture.store_prompts {
        put(&mut doc, "capture", "store_prompts", c.store_prompts.into());
    }
    if c.tool_output != capture.tool_output {
        put(
            &mut doc,
            "capture",
            "tool_output",
            tool_output(c.tool_output).into(),
        );
    }
    write_chain(&mut doc, &now.chain, reordered, chain);
    let candidate = doc.to_string();
    if candidate == text {
        return Ok(show(home));
    }
    parsed(&path, &candidate).ok_or_else(invalid)?;
    let staged =
        crate::setup::stage(&path, &candidate).map_err(|_| refused(500, "write_failed", ""))?;
    // A hand edit or another viewer between the read and here is not overwritten (the temp file
    // goes when `staged` drops).
    // ponytail: one landing between this check and the rename still is; a lock file shared with
    // hand edits' tools cannot exist, so add one for two viewers if that is ever seen.
    if version(bytes(home).map_err(|_| invalid())?.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    staged
        .commit()
        .map_err(|_| refused(500, "write_failed", ""))?;
    Ok(show(home))
}

/// Each entry as it is without `[chain]`: what a value equal to its own is compared with, and
/// what its model rule is taken from.
fn alone(path: &Path, doc: &toml_edit::DocumentMut) -> Option<config::Config> {
    let mut b = doc.clone();
    b.remove("chain");
    config::from_text(path, &b.to_string()).ok()
}

/// The entries' names, each once, in the order of its first entry.
fn names(providers: &[Provider]) -> Vec<&str> {
    let mut seen = std::collections::BTreeSet::new();
    (providers.iter())
        .map(Provider::name)
        .filter(|n| seen.insert(*n))
        .collect()
}

/// `[chain]` as the page asks for it, over the entries `base` has.
#[derive(Default)]
struct Chain {
    order: Vec<String>,
    off: Vec<String>,
    daily_budget: std::collections::BTreeMap<String, u32>,
    timeout_s: std::collections::BTreeMap<String, u64>,
    model: std::collections::BTreeMap<String, String>,
}

/// The posted values, refused where one is out of range, names a model the entry cannot price,
/// or the entries are not the chain's; each value equal to the entry's own is left out. `now` is
/// the file as it applies before the save.
fn checked(posted: &Save, base: &config::Config, now: &config::Config) -> Result<Chain, Refusal> {
    if !posted.paid_usd_per_month.is_finite() || posted.paid_usd_per_month < 0.0 {
        return Err(refused(422, "range", "paid_usd_per_month"));
    }
    let i = &posted.inject;
    for (field, value, range) in [
        (
            "inject.session_start_chars",
            i.session_start_chars,
            config::SESSION_START_CHARS,
        ),
        (
            "inject.per_prompt_chars",
            i.per_prompt_chars,
            config::PER_PROMPT_CHARS,
        ),
        (
            "inject.correction_chars",
            i.correction_chars,
            config::CORRECTION_CHARS,
        ),
    ] {
        if !range.contains(&value) {
            return Err(refused(422, "range", field));
        }
    }
    // Each name once, as the page shows it.
    let order: Vec<String> = posted.chain.iter().map(|e| e.name.clone()).collect();
    let own = names(&base.providers);
    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_unstable();
        v
    }
    if sorted(order.iter().map(String::as_str).collect()) != sorted(own.clone()) {
        return Err(refused(422, "names", "chain"));
    }
    // Turning off the last entry in use. A chain with none in use already (all off by hand, or no
    // entries) is not this save's doing, so the rest of the settings still save (cubic on #94).
    let in_use = |p: &Provider| !now.chain.turns_off(p.name());
    if posted.chain.iter().all(|e| !e.on) && now.providers.iter().any(in_use) {
        return Err(refused(422, "chain_empty", "chain"));
    }
    let mut chain = Chain {
        off: (posted.chain.iter())
            .filter(|e| !e.on)
            .map(|e| e.name.clone())
            .collect(),
        order: if order == own { Vec::new() } else { order },
        ..Chain::default()
    };
    for e in &posted.chain {
        let ps: Vec<&Provider> = (base.providers.iter())
            .filter(|p| p.name() == e.name)
            .collect();
        put_entry(e, &ps, &now.chain, &mut chain)?;
    }
    keep_unknown(&mut chain, &now.chain, &own);
    Ok(chain)
}

/// Puts one entry's calls a day, timeout and model into `chain`: a new value is checked, and one
/// equal to the entry's own is left out, so it keeps following later changes to the defaults. A
/// value the entry or the file has already is the user's, in range or not, so it does not block
/// the rest of a save (cubic on #94).
fn put_entry(
    e: &EntryIn,
    ps: &[&Provider],
    now: &ChainOverlay,
    chain: &mut Chain,
) -> Result<(), Refusal> {
    let field = |key: &str| format!("chain.{}.{key}", e.name);
    // `[chain]` sets every entry of one name alike: a value is the entries' own only when it is
    // each one's, and a model is set only when each one's rule allows it (Codex on #270).
    let each = |f: &dyn Fn(&Provider) -> bool| ps.iter().all(|p| f(p));
    let own_timeout = |p: &Provider| match p {
        Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. } => *timeout_s,
    };
    let own_model = |p: &Provider| match p {
        Provider::Openai { model, .. } => Some(model.clone()),
        Provider::Cli { model, .. } => model.clone(),
    };
    if let Some(n) = e.daily_budget {
        let had = each(&|p| p.daily_budget() == n) || now.daily_budget.get(&e.name) == Some(&n);
        if !had && !BUDGET.contains(&n) {
            return Err(refused(422, "range", field("daily_budget")));
        }
        // A budget from the key follows the key: any number typed is the user's.
        if !each(&|p| !p.budget_from_key() && p.daily_budget() == n) {
            chain.daily_budget.insert(e.name.clone(), n);
        }
    }
    if let Some(posted) = e.timeout_s {
        // A number past 2^53 reaches the page rounded (a JSON number is a double there): one that
        // rounds as the file's own does stands for it, kept as the file has it (#274).
        let rounds_as = |v: u64| v as f64 == posted as f64;
        let s = (now.timeout_s.get(&e.name).copied())
            .filter(|&v| rounds_as(v))
            .unwrap_or(posted);
        // Against the file's value itself: an entry's own that only rounds as it does is another
        // value, which leaving the file's out would switch to (cubic on #287).
        let own = each(&|p| own_timeout(p) == s);
        if !own && now.timeout_s.get(&e.name) != Some(&s) && !TIMEOUT_S.contains(&s) {
            return Err(refused(422, "range", field("timeout_s")));
        }
        if !own {
            chain.timeout_s.insert(e.name.clone(), s);
        }
    }
    // The file's own value stays as it is, applied where each entry's rule allows it.
    if let Some(m) = &e.model
        && now.model.get(&e.name) == Some(m)
    {
        chain.model.insert(e.name.clone(), m.clone());
    } else if let Some(m) = &e.model
        && !each(&|p| own_model(p).as_ref() == Some(m))
    {
        let allowed = |c: char| c.is_ascii_alphanumeric() || "._:/@+-".contains(c);
        let fits = !m.is_empty() && m.chars().count() <= MODEL_CHARS && m.chars().all(allowed);
        if !fits {
            return Err(refused(422, "model", field("model")));
        }
        if !each(&|p| p.model_rule().allows(m)) {
            return Err(refused(422, "paid_model", field("model")));
        }
        chain.model.insert(e.name.clone(), m.clone());
    }
    Ok(())
}

/// A name `[chain]` has and no entry has (doctor warns of it) is not the page's: it stays as it
/// is, and does not make a save rewrite its key (cubic on #94).
fn keep_unknown(chain: &mut Chain, now: &ChainOverlay, own: &[&str]) {
    fn keep<V: Clone>(
        to: &mut std::collections::BTreeMap<String, V>,
        now: &std::collections::BTreeMap<String, V>,
        unknown: &dyn Fn(&str) -> bool,
    ) {
        to.extend((now.iter().filter(|(n, _)| unknown(n))).map(|(n, v)| (n.clone(), v.clone())));
    }
    let unknown = |n: &str| !own.contains(&n);
    chain
        .order
        .extend(now.order.iter().filter(|n| unknown(n)).cloned());
    chain
        .off
        .extend(now.off.iter().filter(|n| unknown(n)).cloned());
    keep(&mut chain.daily_budget, &now.daily_budget, &unknown);
    keep(&mut chain.timeout_s, &now.timeout_s, &unknown);
    keep(&mut chain.model, &now.model, &unknown);
}

/// Changes a top-level value while keeping its comment; `None` removes an optional setting.
fn put_root(doc: &mut toml_edit::DocumentMut, key: &str, value: Option<toml_edit::Value>) {
    if let Some(mut value) = value {
        if let Some(old) = doc.get(key).and_then(toml_edit::Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        doc[key] = toml_edit::Item::Value(value);
    } else {
        // TOML has no null value for Gemini. Keep the removed line's comments at the end
        // of the document instead of dropping them with its key.
        let comments = doc
            .as_table()
            .get_key_value(key)
            .map(|(name, item)| {
                let prefix = name
                    .leaf_decor()
                    .prefix()
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let suffix = item
                    .as_value()
                    .and_then(|v| v.decor().suffix())
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                format!("{prefix}{suffix}")
            })
            .unwrap_or_default();
        doc.remove(key);
        if comments.contains('#') {
            doc.set_trailing(format!(
                "{}\n{comments}\n",
                doc.trailing().as_str().unwrap_or("")
            ));
        }
    }
}

/// Sets `table.key`, making the table (a `[table]`, not an inline one) when it is missing. A value
/// replaced keeps the spacing and comment around it (cubic on #270).
fn put(doc: &mut toml_edit::DocumentMut, table: &str, key: &str, mut value: toml_edit::Value) {
    if !doc.contains_key(table) {
        doc.insert(table, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    if let Some(old) = doc[table].get(key).and_then(toml_edit::Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    doc[table][key] = toml_edit::Item::Value(value);
}

/// Writes each `[chain]` key whose value changes from what the file has now (`now`; the order
/// when `reordered`), and removes one the page no longer sets; a key that does not change is not
/// touched, so its comments stay.
fn write_chain(doc: &mut toml_edit::DocumentMut, now: &ChainOverlay, reordered: bool, to: Chain) {
    fn set(v: &[String]) -> std::collections::BTreeSet<&String> {
        v.iter().collect()
    }
    fn map<V>(
        m: &std::collections::BTreeMap<String, V>,
        f: impl Fn(&V) -> toml_edit::Value,
    ) -> New {
        New::Map(m.iter().map(|(k, v)| (k.clone(), f(v))).collect())
    }
    let changes = [
        ("order", reordered, New::List(to.order.clone())),
        (
            "off",
            set(&to.off) != set(&now.off),
            New::List(to.off.clone()),
        ),
        (
            "daily_budget",
            to.daily_budget != now.daily_budget,
            map(&to.daily_budget, |v| i64::from(*v).into()),
        ),
        (
            "timeout_s",
            to.timeout_s != now.timeout_s,
            map(&to.timeout_s, |v| (*v as i64).into()),
        ),
        (
            "model",
            to.model != now.model,
            map(&to.model, |v| v.as_str().into()),
        ),
    ];
    for (key, changed, new) in changes {
        if changed {
            set_key(doc, key, new);
        }
    }
}

/// A `[chain]` key's new value: names, or values by name.
enum New {
    List(Vec<String>),
    Map(Vec<(String, toml_edit::Value)>),
}

/// Sets `[chain].key`, or removes it when it is empty. A map the file has already, as `{ ... }`
/// or as a table of its own, changes entry by entry, so an entry that does not change keeps its
/// comment and place (cubic on #270).
fn set_key(doc: &mut toml_edit::DocumentMut, key: &str, new: New) {
    let empty = match &new {
        New::List(v) => v.is_empty(),
        New::Map(m) => m.is_empty(),
    };
    if empty {
        if let Some(t) = (doc.get_mut("chain")).and_then(toml_edit::Item::as_table_like_mut) {
            t.remove(key);
        }
        return;
    }
    let had = (doc.get("chain").and_then(|c| c.get(key)))
        .and_then(toml_edit::Item::as_table_like)
        .is_some();
    match new {
        New::Map(m) if had => {
            let Some(t) = doc["chain"][key].as_table_like_mut() else {
                return;
            };
            let gone: Vec<String> = (t.iter().map(|(k, _)| k.to_owned()))
                .filter(|k| !m.iter().any(|(n, _)| n == k))
                .collect();
            for k in gone {
                t.remove(&k);
            }
            let same = |a: &toml_edit::Value, b: &toml_edit::Value| {
                (a.as_integer().is_some() && a.as_integer() == b.as_integer())
                    || (a.as_str().is_some() && a.as_str() == b.as_str())
            };
            for (n, v) in m {
                match t.get_mut(&n).and_then(toml_edit::Item::as_value_mut) {
                    Some(old) if same(old, &v) => {}
                    Some(old) => {
                        let decor = old.decor().clone();
                        *old = v;
                        *old.decor_mut() = decor;
                    }
                    None => {
                        t.insert(&n, toml_edit::Item::Value(v));
                    }
                }
            }
        }
        New::Map(m) => put(
            doc,
            "chain",
            key,
            toml_edit::Value::InlineTable(m.into_iter().collect()),
        ),
        New::List(v) => put(
            doc,
            "chain",
            key,
            toml_edit::Value::Array(v.iter().map(String::as_str).collect()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(text: Option<&str>) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        if let Some(t) = text {
            std::fs::write(home.path().join("config.toml"), t).unwrap();
        }
        home
    }

    fn file(home: &tempfile::TempDir) -> Option<String> {
        std::fs::read_to_string(home.path().join("config.toml")).ok()
    }

    /// The page's body for the values `show` gave, with `change` applied to it.
    fn posted(shown: &Value, change: impl FnOnce(&mut Value)) -> Vec<u8> {
        let chain: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
            .map(|e| {
                json!({"name": e["name"], "on": e["on"], "daily_budget": e["daily_budget"],
                    "timeout_s": e["timeout_s"], "model": e["model"]})
            })
            .collect();
        let mut v = json!({"version": shown["version"], "inject": shown["inject"],
            "summary": shown["summary"], "paid_usd_per_month": shown["paid_usd_per_month"],
            "gemini": shown["gemini"], "capture": shown["capture"], "chain": chain});
        change(&mut v);
        serde_json::to_vec(&v).unwrap()
    }

    fn save_to(home: &tempfile::TempDir, body: &[u8]) -> Result<Value, Refusal> {
        save(home.path(), &Mutex::new(()), body)
    }

    #[test]
    fn the_summary_shows_saved_values_and_the_parser_ranges() {
        let home = home_with(None);
        let shown = show(home.path());
        assert_eq!(
            shown["summary"],
            json!({"curate": false, "language": "Japanese", "window_tokens": 5000,
                "idle_minutes": 10})
        );
        assert_eq!(
            shown["ranges"]["window_tokens"],
            json!([0, 4_294_967_295_u32])
        );
        assert_eq!(
            shown["ranges"]["idle_minutes"],
            json!([0, 4_294_967_295_u32])
        );
        assert!(!home.path().join("config.toml").exists());
        assert!(!home.path().join("providers.db").exists());

        let home = home_with(Some(
            "[summary]\ncurate = true\nlanguage = \"\"\nwindow_tokens = 0\nidle_minutes = 4294967295\n",
        ));
        assert_eq!(
            show(home.path())["summary"],
            json!({"curate": true, "language": "", "window_tokens": 0,
                "idle_minutes": 4_294_967_295_u32})
        );
    }

    #[test]
    fn a_summary_save_is_lossless_and_stale_checked() {
        let text = "# invented settings\n[summary]\ncurate = false # owner switch\n\
            language = \"Japanese\" # language note\nwindow_tokens = 5000 # size note\n\
            idle_minutes = 10 # wait note\nshrink = true\nfuture = \"kept\"\n\
            [backup]\ndir = \"invented-backups\" # unrelated\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        let unchanged = save_to(&home, &posted(&shown, |_| {})).unwrap();
        assert_eq!(file(&home).as_deref(), Some(text));
        assert_eq!(unchanged["version"], shown["version"]);
        let body = posted(&shown, |v| {
            v["summary"] = json!({"curate": true, "language": "English\n日本語 <example>",
                "window_tokens": 0, "idle_minutes": 4_294_967_295_u32});
        });
        let saved = save_to(&home, &body).unwrap();
        assert_eq!(saved["summary"]["language"], "English\n日本語 <example>");
        let cfg = config::load(home.path()).unwrap();
        assert!(cfg.summary.curate && cfg.summary.shrink);
        assert_eq!(cfg.summary.window_tokens, 0);
        assert_eq!(cfg.summary.idle_minutes, u32::MAX);
        let after = file(&home).unwrap();
        for kept in [
            "# invented settings",
            "# owner switch",
            "# language note",
            "# size note",
            "# wait note",
            "shrink = true",
            "future = \"kept\"",
            "dir = \"invented-backups\" # unrelated",
        ] {
            assert!(after.contains(kept), "{kept}: {after}");
        }
        assert!(!home.path().join("providers.db").exists());
        let stale = save_to(&home, &body).unwrap_err();
        assert_eq!((stale.status, stale.code), (409, "stale"));
        assert_eq!(file(&home).as_deref(), Some(after.as_str()));
    }

    #[test]
    fn the_paid_cap_takes_finite_values_of_0_or_more_only() {
        let home = home_with(Some(
            "paid_usd_per_month = 5.0 # monthly cap\n[summary]\nfuture = 7\n",
        ));
        let mut shown = show(home.path());
        assert_eq!(shown["paid_usd_per_month"], 5.0);
        assert_eq!(shown["ranges"]["paid_usd_per_month"], json!([0, null]));
        for cap in [0.0, 0.125, 5.0, 1.0e300] {
            shown = save_to(
                &home,
                &posted(&shown, |v| v["paid_usd_per_month"] = json!(cap)),
            )
            .unwrap();
            assert_eq!(shown["paid_usd_per_month"], cap);
            assert_eq!(config::load(home.path()).unwrap().paid_usd_per_month, cap);
            let text = file(&home).unwrap();
            assert!(
                text.contains("# monthly cap") && text.contains("future = 7"),
                "{text}"
            );
        }
        let before = file(&home).unwrap();
        let invalid = [
            ("paid_usd_per_month", json!(-0.1)),
            ("paid_usd_per_month", json!("5")),
            ("paid_usd_per_month", Value::Null),
        ];
        for (field, value) in invalid {
            let refusal = save_to(&home, &posted(&shown, |v| v[field] = value)).unwrap_err();
            assert_eq!(refusal.status, 422);
            assert_eq!(file(&home).as_deref(), Some(before.as_str()));
        }
        for (field, value) in [
            ("curate", json!("true")),
            ("language", json!(42)),
            ("window_tokens", json!(-1)),
            ("window_tokens", json!(4_294_967_296_u64)),
            ("window_tokens", json!(1.5)),
            ("idle_minutes", json!(-1)),
            ("idle_minutes", json!(4_294_967_296_u64)),
            ("idle_minutes", json!(1.5)),
            ("shrink", json!(true)),
        ] {
            let refusal =
                save_to(&home, &posted(&shown, |v| v["summary"][field] = value)).unwrap_err();
            assert_eq!((refusal.status, refusal.code), (422, "type"), "{field}");
            assert_eq!(file(&home).as_deref(), Some(before.as_str()));
        }
        let overflow = String::from_utf8(posted(&shown, |v| v["paid_usd_per_month"] = json!(0)))
            .unwrap()
            .replace("\"paid_usd_per_month\":0", "\"paid_usd_per_month\":1e999");
        assert!(save_to(&home, overflow.as_bytes()).is_err());
        assert_eq!(file(&home).as_deref(), Some(before.as_str()));
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn the_spend_and_the_owner_stops_are_read_without_a_write() {
        let empty = home_with(None);
        let shown = show(empty.path());
        assert_eq!(shown["usd_this_month"], 0.0);
        assert_eq!(shown["stopped"], json!([]));
        assert!(!empty.path().join("providers.db").exists());

        let home = home_with(Some("providers = []\n"));
        let db = Connection::open(home.path().join("providers.db")).unwrap();
        // An older ledger needs no schema upgrade to read its spend and stops.
        db.execute_batch(
            "CREATE TABLE provider_calls(provider TEXT, ts INTEGER, role TEXT, usd REAL);
            CREATE TABLE provider_state(provider TEXT, down_until INTEGER);
            INSERT INTO provider_state VALUES ('owner-stopped', 9223372036854775807),
                ('time-stopped', 9223372036854775806), ('expired', 1);",
        )
        .unwrap();
        for (provider, ts, role, usd) in [
            ("removed-entry", crate::db::now_ms(), "curator", Some(0.25)),
            ("other-entry", crate::db::now_ms(), "digest", Some(0.5)),
            ("embedding", crate::db::now_ms(), "embed", Some(99.0)),
            ("embedding", crate::db::now_ms(), "query", Some(99.0)),
            ("old-entry", 1, "curator", Some(88.0)),
            ("free-entry", crate::db::now_ms(), "curator", None),
        ] {
            db.execute(
                "INSERT INTO provider_calls VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![provider, ts, role, usd],
            )
            .unwrap();
        }
        let before = std::fs::read(home.path().join("providers.db")).unwrap();
        let shown = show(home.path());
        assert_eq!(shown["usd_this_month"], 0.75);
        assert_eq!(shown["stopped"], json!(["owner-stopped"]));
        assert_eq!(
            std::fs::read(home.path().join("providers.db")).unwrap(),
            before
        );
        assert!(!home.path().join("providers.db-wal").exists());
        assert_eq!(file(&home).as_deref(), Some("providers = []\n"));

        let damaged = home_with(Some("providers = []\n"));
        std::fs::write(
            damaged.path().join("providers.db"),
            "invented damaged ledger",
        )
        .unwrap();
        let shown = show(damaged.path());
        assert!(shown.get("error").is_none());
        assert_eq!(shown["usd_this_month"], Value::Null);
        assert_eq!(shown["stopped"], Value::Null);
        assert_eq!(
            std::fs::read_to_string(damaged.path().join("providers.db")).unwrap(),
            "invented damaged ledger"
        );
    }

    #[test]
    fn geminis_place_follows_the_config_chain() {
        let home = home_with(Some("[summary]\nfuture = \"kept\" # unchanged\n"));
        let mut shown = show(home.path());
        assert!(
            shown.get("gemini").is_some(),
            "Gemini's saved choice must be shown"
        );
        assert_eq!(shown["gemini"], Value::Null);
        for place in [
            json!("before-subscriptions"),
            json!("after-subscriptions"),
            Value::Null,
        ] {
            shown = save_to(&home, &posted(&shown, |v| v["gemini"] = place.clone())).unwrap();
            assert_eq!(shown["gemini"], place);
            let cfg = config::load(home.path()).unwrap();
            let gemini = cfg.providers.iter().position(|p| p.name() == "gemini");
            match place.as_str() {
                Some("before-subscriptions") => {
                    let first_cli = cfg
                        .providers
                        .iter()
                        .position(|p| matches!(p, Provider::Cli { .. }))
                        .unwrap();
                    assert_eq!(gemini, Some(first_cli - 1));
                }
                Some("after-subscriptions") => assert_eq!(gemini, Some(cfg.providers.len() - 1)),
                _ => assert!(gemini.is_none()),
            }
            assert_eq!(
                shown["chain"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["name"] == "gemini"),
                place != Value::Null
            );
            assert!(
                file(&home)
                    .unwrap()
                    .contains("future = \"kept\" # unchanged")
            );
            assert!(!home.path().join("providers.db").exists());
        }
        let before = file(&home).unwrap();
        for invalid in [json!("none"), json!("elsewhere"), json!(true), json!(1)] {
            let refusal = save_to(&home, &posted(&shown, |v| v["gemini"] = invalid)).unwrap_err();
            assert!(matches!(
                (refusal.status, refusal.code),
                (422, "type") | (400, "bad_request")
            ));
            assert_eq!(file(&home).as_deref(), Some(before.as_str()));
        }

        // The parser keeps an explicit Gemini entry and lets [chain] order take precedence.
        let explicit = home_with(Some(
            "gemini = \"before-subscriptions\" # place\n\
            [chain]\norder = [\"subscription\", \"gemini\"]\n\
            [[providers]]\nkind = \"cli\"\nname = \"subscription\"\ncli = \"invented-cli\"\n\
            [[providers]]\nkind = \"openai\"\nname = \"gemini\"\n\
            base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"invented-model\"\n",
        ));
        let shown = show(explicit.path());
        let saved = save_to(
            &explicit,
            &posted(&shown, |v| v["gemini"] = json!("after-subscriptions")),
        )
        .unwrap();
        assert_eq!(saved["chain"][0]["name"], "subscription");
        assert_eq!(saved["chain"][1]["name"], "gemini");
        assert!(
            file(&explicit)
                .unwrap()
                .contains("gemini = \"after-subscriptions\" # place")
        );
        let saved = save_to(&explicit, &posted(&saved, |v| v["gemini"] = Value::Null)).unwrap();
        assert_eq!(saved["gemini"], Value::Null);
        assert_eq!(saved["chain"][1]["name"], "gemini");
    }

    #[test]
    fn taking_gemini_out_keeps_its_comments() {
        let home = home_with(Some(
            "# invented heading\ngemini = \"after-subscriptions\" # place note\n\
            paid_usd_per_month = 5.0 # cap note\n[summary]\nfuture = \"kept\" # future note\n",
        ));
        let shown = show(home.path());
        let saved = save_to(&home, &posted(&shown, |v| v["gemini"] = Value::Null)).unwrap();
        assert_eq!(saved["gemini"], Value::Null);
        let text = file(&home).unwrap();
        for comment in [
            "# invented heading",
            "# place note",
            "# cap note",
            "# future note",
        ] {
            assert!(text.contains(comment), "missing {comment}: {text}");
        }
        assert_eq!(config::load(home.path()).unwrap().gemini, None);
    }

    #[test]
    fn a_top_level_setting_changed_keeps_the_comments_above_it() {
        let home = home_with(Some(
            "# cap heading\n\"paid_usd_per_month\" = 5.0 # cap suffix\n\
            # Gemini heading\n\"gemini\" = \"before-subscriptions\" # Gemini suffix\n\
            [summary]\nfuture = \"kept\" # unrelated\n",
        ));
        let shown = show(home.path());
        let saved = save_to(
            &home,
            &posted(&shown, |v| {
                v["paid_usd_per_month"] = json!(0.125);
                v["gemini"] = json!("after-subscriptions");
            }),
        )
        .unwrap();
        assert_eq!(saved["paid_usd_per_month"], 0.125);
        assert_eq!(saved["gemini"], "after-subscriptions");
        let text = file(&home).unwrap();
        for kept in [
            "# cap heading",
            "# Gemini heading",
            "\"paid_usd_per_month\" = 0.125 # cap suffix",
            "\"gemini\" = \"after-subscriptions\" # Gemini suffix",
            "future = \"kept\" # unrelated",
        ] {
            assert!(text.contains(kept), "missing {kept}: {text}");
        }
    }

    fn at<'a>(shown: &'a Value, name: &str) -> &'a Value {
        (shown["chain"].as_array().unwrap().iter())
            .find(|e| e["name"] == name)
            .unwrap()
    }

    /// A config of API entries, each named with its key file.
    fn api_entries(entries: &[(&str, &Path)]) -> String {
        (entries.iter())
            .map(|(name, file)| {
                format!(
                    "[[providers]]\nkind = \"openai\"\nname = \"{name}\"\n\
                     base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\nkey_file = {:?}\n\n",
                    file.display().to_string()
                )
            })
            .collect()
    }

    fn key_body(entry: &str, key: &str, version: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"entry": entry, "key": key, "version": version})).unwrap()
    }

    /// #94 part 3: a key typed on the page reaches its entry's key file, against the config the
    /// page showed, and no answer holds it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_key_saved_from_the_page_reaches_its_file_and_no_answer() {
        let keys = tempfile::tempdir().unwrap();
        let file = keys.path().join("LOCAL_KEY.md");
        std::fs::write(&file, "Local\nold-key-123\n").unwrap();
        let text = api_entries(&[("local", &file), ("twin", &file), ("twin", &file)]);
        let home = home_with(Some(&text));
        let version = show(home.path())["version"].clone();
        let canary = format!("{}-{}", "canary", "7d1e0b52");
        let saved = save_key(
            home.path(),
            &Mutex::new(()),
            &key_body("local", &canary, &version),
        );
        assert_eq!(
            saved,
            Ok(json!({"entry": "local", "key": "ok", "durable": true}))
        );
        assert_eq!(crate::config::read_key(&file).unwrap(), canary);
        let shown = show(home.path());
        assert!(!shown.to_string().contains(&canary), "{shown}");
        // config.toml is not written, so the page's version still holds.
        assert_eq!(
            (self::file(&home), &shown["version"]),
            (Some(text.clone()), &version)
        );
        // Entries of one name that name one file take the key.
        let twin = save_key(
            home.path(),
            &Mutex::new(()),
            &key_body("twin", "twin-key-1", &version),
        );
        assert_eq!(twin.unwrap()["key"], "ok");
        assert_eq!(crate::config::read_key(&file).unwrap(), "twin-key-1");
        // A config changed since the page read it: refused, nothing written.
        std::fs::write(home.path().join("config.toml"), format!("{text}# edited\n")).unwrap();
        let stale = save_key(
            home.path(),
            &Mutex::new(()),
            &key_body("local", "late-key-12", &version),
        );
        let r = stale.unwrap_err();
        assert_eq!((r.status, r.code), (409, "stale"));
        assert_eq!(crate::config::read_key(&file).unwrap(), "twin-key-1");
    }

    /// #94 part 3: a key save names one key file of one API entry, and a request of another
    /// shape writes nothing.
    #[test]
    fn a_key_save_names_one_key_file_of_one_api_entry() {
        let keys = tempfile::tempdir().unwrap();
        let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
        let text = api_entries(&[("twin", &a), ("twin", &b)])
            + "[[providers]]\nkind = \"openai\"\nname = \"open\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\n\
               [[providers]]\nkind = \"cli\"\nname = \"sub\"\ncli = \"claude\"\n";
        let home = home_with(Some(&text));
        let version = show(home.path())["version"].clone();
        let refusal = |body: Vec<u8>| {
            let r = save_key(home.path(), &Mutex::new(()), &body).unwrap_err();
            (r.status, r.code)
        };
        let key = "key-abcdefgh12";
        assert_eq!(refusal(key_body("twin", key, &version)), (422, "ambiguous"));
        assert_eq!(
            refusal(key_body("open", key, &version)),
            (422, "no_key_file")
        );
        assert_eq!(
            refusal(key_body("sub", key, &version)),
            (422, "no_key_file")
        );
        assert_eq!(
            refusal(key_body("nobody", key, &version)),
            (404, "no_entry")
        );
        assert_eq!(
            refusal(key_body("twin", "short", &version)),
            (422, "bad_key")
        );
        assert_eq!(
            refusal(key_body("twin", "key-abc\nbase_url", &version)),
            (422, "bad_key")
        );
        assert_eq!(
            refusal(key_body(&"x".repeat(65), key, &version)),
            (422, "bad_entry")
        );
        let extra =
            json!({"entry": "twin", "key": key, "version": version, "key_file": "/tmp/X_KEY.md"});
        assert_eq!(
            refusal(serde_json::to_vec(&extra).unwrap()),
            (400, "bad_request")
        );
        assert_eq!(self::file(&home), Some(text));
        assert!(!a.exists() && !b.exists());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_key_save_waits_for_owner_only_files_off_linux() {
        let keys = tempfile::tempdir().unwrap();
        let file = keys.path().join("LOCAL_KEY.md");
        let home = home_with(Some(&api_entries(&[("local", &file)])));
        let version = show(home.path())["version"].clone();
        let r = save_key(
            home.path(),
            &Mutex::new(()),
            &key_body("local", "key-abcdefgh12", &version),
        );
        let r = r.unwrap_err();
        assert_eq!((r.status, r.code), (422, "unsupported"));
        assert!(!file.exists());
    }

    /// #94 acceptance test 2: no key reaches the page, and none is taken from it.
    #[test]
    fn no_key_is_shown_or_taken() {
        let home = tempfile::tempdir().unwrap();
        let sentinel = format!("{}-{}", "oboete-sentinel", "4b1d");
        let key = home.path().join("LOCAL_KEY.md");
        std::fs::write(&key, format!("# the key\n{sentinel}\n")).unwrap();
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\nbase_url = \"http://127.0.0.1:9/v1\"\n\
             model = \"m\"\nkey_file = {:?}\nheaders = {{ \"x-h\" = \"{sentinel}\" }}\n\
             extra = {{ note = \"{sentinel}\" }}\n",
            key.display().to_string()
        );
        std::fs::write(home.path().join("config.toml"), &text).unwrap();
        let shown = show(home.path());
        assert!(shown.get("error").is_none(), "{shown}");
        assert!(!shown.to_string().contains(&sentinel), "{shown}");
        assert_eq!(at(&shown, "local")["key"], "ok");
        for extra in [
            ("key", json!(sentinel)),
            ("base_url", json!("http://evil.example/v1")),
            ("key_file", json!("/tmp/k")),
            ("headers", json!({"x": "y"})),
        ] {
            let body = posted(&shown, |v| {
                v["chain"][0][extra.0] = extra.1.clone();
            });
            let refusal = save_to(&home, &body).unwrap_err();
            assert_eq!((refusal.status, refusal.code), (422, "type"), "{}", extra.0);
            assert_eq!(file(&home).as_deref(), Some(text.as_str()));
        }
        let saved = save_to(
            &home,
            &posted(&shown, |v| v["chain"][0]["timeout_s"] = json!(30)),
        )
        .unwrap();
        assert!(!saved.to_string().contains(&sentinel), "{saved}");
    }

    /// A save writes only the keys it changes, keeps the rest of the file (comments, other
    /// tables) as it was, and leaves a value equal to the entry's own out.
    #[test]
    fn a_save_writes_only_what_changes() {
        let text = "# my settings\n[summary]\ncurate = false # not yet\n\n[backup]\ndir = \"b\"\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        let unchanged = save_to(&home, &posted(&shown, |_| {})).unwrap();
        assert_eq!(file(&home).as_deref(), Some(text));
        assert_eq!(unchanged["version"], shown["version"]);
        let groq = at(&shown, "groq");
        let own_timeout = groq["effective_timeout_s"].clone();
        let names: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
            .map(|e| e["name"].clone())
            .collect();
        let saved = save_to(
            &home,
            &posted(&shown, |v| {
                v["inject"]["session_start_chars"] = json!(2000);
                v["capture"]["tool_output"] = json!("head-tail");
                let chain = v["chain"].as_array_mut().unwrap();
                chain.swap(0, 1);
                for e in chain.iter_mut() {
                    match e["name"].as_str().unwrap() {
                        "codex" => e["on"] = json!(false),
                        "groq" => {
                            e["timeout_s"] = own_timeout.clone();
                            e["daily_budget"] = json!(50);
                        }
                        "claude" => e["model"] = json!("sonnet"),
                        _ => {}
                    }
                }
            }),
        )
        .unwrap();
        let after = file(&home).unwrap();
        assert!(after.starts_with(text), "{after}");
        for line in [
            "session_start_chars = 2000",
            "tool_output = \"head-tail\"",
            "off = [\"codex\"]",
            "daily_budget = { groq = 50 }",
            "model = { claude = \"sonnet\" }",
        ] {
            assert!(after.contains(line), "{line}: {after}");
        }
        // The groq timeout equals its own: not written.
        assert!(!after.contains("timeout_s"), "{after}");
        let order = format!(
            "order = [{:?}, {:?}",
            names[1].as_str().unwrap(),
            names[0].as_str().unwrap()
        );
        assert!(after.contains(&order), "{order}: {after}");
        assert_eq!(saved["inject"]["session_start_chars"], 2000);
        assert_eq!(at(&saved, "codex")["on"], false);
        assert_eq!(at(&saved, "claude")["effective_model"], "sonnet");
        // Back to the entry's own values: the keys go.
        let back = save_to(
            &home,
            &posted(&saved, |v| {
                for e in v["chain"].as_array_mut().unwrap() {
                    e["on"] = json!(true);
                    e["daily_budget"] = Value::Null;
                    e["model"] = Value::Null;
                }
                let chain = v["chain"].as_array_mut().unwrap();
                chain.swap(0, 1);
            }),
        )
        .unwrap();
        let after = file(&home).unwrap();
        for key in ["order", "off", "daily_budget", "model"] {
            assert!(!after.contains(&format!("{key} =")), "{key}: {after}");
        }
        assert_eq!(at(&back, "codex")["on"], true);
    }

    /// Each refusal leaves the file's bytes as they were.
    #[test]
    fn a_refused_save_leaves_the_file() {
        let text = "[summary]\ncurate = false\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        let refusal = |change: &dyn Fn(&mut Value)| {
            let r = save_to(&home, &posted(&shown, change)).unwrap_err();
            assert_eq!(file(&home).as_deref(), Some(text));
            (r.status, r.code, r.field)
        };
        let named = |name: &'static str, key: &'static str, value: Value| {
            move |v: &mut Value| {
                for e in v["chain"].as_array_mut().unwrap() {
                    if e["name"] == name {
                        e[key] = value.clone();
                    }
                }
            }
        };
        assert_eq!(
            refusal(&|v| v["version"] = json!("0000")),
            (409, "stale", String::new())
        );
        assert_eq!(
            refusal(&|v| v["inject"]["session_start_chars"] = json!(999)),
            (422, "range", "inject.session_start_chars".into())
        );
        assert_eq!(
            refusal(&named("groq", "daily_budget", json!(0))),
            (422, "range", "chain.groq.daily_budget".into())
        );
        assert_eq!(
            refusal(&named("groq", "timeout_s", json!(4))),
            (422, "range", "chain.groq.timeout_s".into())
        );
        assert_eq!(
            refusal(&named("groq", "model", json!("m\nx"))),
            (422, "model", "chain.groq.model".into())
        );
        assert_eq!(
            refusal(&named("openrouter", "model", json!("openai/gpt-6"))),
            (422, "paid_model", "chain.openrouter.model".into())
        );
        assert_eq!(
            refusal(&|v| {
                v["chain"].as_array_mut().unwrap().pop();
            }),
            (422, "names", "chain".into())
        );
        assert_eq!(
            refusal(&|v| {
                let first = v["chain"][0].clone();
                v["chain"].as_array_mut().unwrap().push(first);
            }),
            (422, "names", "chain".into())
        );
        assert_eq!(
            refusal(&|v| {
                for e in v["chain"].as_array_mut().unwrap() {
                    e["on"] = json!(false);
                }
            }),
            (422, "chain_empty", "chain".into())
        );
        assert_eq!(
            refusal(&|v| v["capture"]["tool_output"] = json!("some")),
            (422, "type", String::new())
        );
        let r = save_to(&home, b"{not json").unwrap_err();
        assert_eq!((r.status, r.code), (400, "bad_request"));
        assert_eq!(file(&home).as_deref(), Some(text));
    }

    /// A model the entry cannot price is refused; an OpenRouter `:free` entry takes another
    /// `:free` model, and an entry with prices keeps its model.
    #[test]
    fn a_model_the_entry_cannot_price_is_refused() {
        let text = "gemini = \"after-subscriptions\"\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        assert_eq!(at(&shown, "gemini")["model_rule"], "fixed");
        assert_eq!(at(&shown, "openrouter")["model_rule"], "free");
        assert_eq!(at(&shown, "groq")["model_rule"], "any");
        let model = |name: &'static str, m: &'static str| {
            posted(&shown, move |v| {
                for e in v["chain"].as_array_mut().unwrap() {
                    if e["name"] == name {
                        e["model"] = json!(m);
                    }
                }
            })
        };
        let r = save_to(&home, &model("gemini", "gemini-9-pro")).unwrap_err();
        assert_eq!((r.status, r.code), (422, "paid_model"));
        let own = at(&shown, "gemini")["effective_model"]
            .as_str()
            .unwrap()
            .to_owned();
        let same = posted(&shown, |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == "gemini" {
                    e["model"] = json!(own);
                }
            }
        });
        save_to(&home, &same).unwrap();
        assert_eq!(file(&home).as_deref(), Some(text));
        let saved = save_to(&home, &model("openrouter", "qwen/qwen3.8-27b:free")).unwrap();
        assert_eq!(
            at(&saved, "openrouter")["effective_model"],
            "qwen/qwen3.8-27b:free"
        );
    }

    /// Task 8: the prompt's injection and its corrections are shown with their ranges, saved
    /// alone, and refused out of range by name.
    #[test]
    fn inject_settings_check_ranges_and_save_alone() {
        let home = home_with(None);
        let shown = show(home.path());
        assert_eq!(
            shown["inject"],
            json!({"session_start": true, "session_start_chars": 9000, "per_prompt": false,
                "per_prompt_chars": 1500, "correction": true, "correction_chars": 800})
        );
        assert_eq!(shown["ranges"]["per_prompt_chars"], json!([500, 6000]));
        assert_eq!(shown["ranges"]["correction_chars"], json!([300, 3000]));
        save_to(
            &home,
            &posted(&shown, |v| {
                v["inject"]["per_prompt"] = json!(true);
                v["inject"]["correction_chars"] = json!(1200);
            }),
        )
        .unwrap();
        assert_eq!(
            file(&home).as_deref(),
            Some("[inject]\nper_prompt = true\ncorrection_chars = 1200\n")
        );
        let i = config::inject(home.path()).unwrap();
        assert!(i.per_prompt && i.correction_chars == 1_200);
        for (key, bad) in [("per_prompt_chars", 499), ("correction_chars", 3_001)] {
            let shown = show(home.path());
            let r = save_to(&home, &posted(&shown, |v| v["inject"][key] = json!(bad))).unwrap_err();
            assert_eq!(
                (r.status, r.code, r.field),
                (422, "range", format!("inject.{key}"))
            );
        }
    }

    /// No file: the page shows the defaults and a save makes a file with only what it changes.
    #[test]
    fn a_save_without_a_file_makes_one() {
        let home = home_with(None);
        let shown = show(home.path());
        assert_eq!(shown["version"], "none");
        assert_eq!(shown["inject"]["session_start_chars"], 9_000);
        save_to(
            &home,
            &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
        )
        .unwrap();
        assert_eq!(
            file(&home).as_deref(),
            Some("[inject]\nsession_start = false\n")
        );
        assert!(!config::inject(home.path()).unwrap().session_start);
    }

    /// A chain with no entry in use already, by hand or with no entries at all, does not stop
    /// the rest of the settings from saving; turning off the last one in use does (cubic on #94).
    #[test]
    fn a_chain_with_none_in_use_still_saves_the_rest() {
        let names: Vec<String> = (show(home_with(None).path())["chain"].as_array().unwrap())
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect();
        let all_off = format!("[chain]\noff = {names:?}\n");
        for text in ["providers = []\n", all_off.as_str()] {
            let home = home_with(Some(text));
            let shown = show(home.path());
            let saved = save_to(
                &home,
                &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
            )
            .unwrap_or_else(|r| panic!("{text}: {r:?}"));
            assert_eq!(saved["inject"]["session_start"], false, "{text}");
            assert!(file(&home).unwrap().starts_with(text), "{text}");
        }
    }

    /// What is not the page's stays as the file has it: a name no entry has, and a value out of
    /// the page's range that the user wrote by hand (cubic on #94).
    #[test]
    fn a_save_keeps_what_is_not_the_pages() {
        let text = "[chain]\noff = [\"gone\"]\ndaily_budget = { old = 20, groq = 200000 }\n\
                    timeout_s = { groq = 1200 }\nmodel = { old = \"m x\" }\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        assert_eq!(at(&shown, "groq")["timeout_s"], 1200);
        let saved = save_to(
            &home,
            &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
        )
        .unwrap();
        assert_eq!(saved["inject"]["session_start"], false);
        assert!(file(&home).unwrap().starts_with(text), "{:?}", file(&home));
        let shown = show(home.path());
        save_to(
            &home,
            &posted(&shown, |v| {
                for e in v["chain"].as_array_mut().unwrap() {
                    match e["name"].as_str().unwrap() {
                        "groq-20b" => e["daily_budget"] = json!(50),
                        "nim" => e["on"] = json!(false),
                        _ => {}
                    }
                }
            }),
        )
        .unwrap();
        let chain = &config::load(home.path()).unwrap().chain;
        assert_eq!(chain.off, ["nim", "gone"]);
        assert_eq!(chain.daily_budget["old"], 20);
        assert_eq!(chain.daily_budget["groq"], 200_000);
        assert_eq!(chain.daily_budget["groq-20b"], 50);
        assert_eq!(chain.timeout_s["groq"], 1200);
        assert_eq!(chain.model["old"], "m x");
    }

    /// `[chain]` sets every entry of one name alike, so the page has one row per name, and a
    /// value is the entries' own only when it is each one's (Codex on #270).
    #[test]
    fn entries_of_one_name_are_one_row() {
        let entry = "[[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n";
        let text = format!("{entry}timeout_s = 60\n\n{entry}timeout_s = 90\n");
        let home = home_with(Some(&text));
        let shown = show(home.path());
        assert_eq!(shown["chain"].as_array().unwrap().len(), 1);
        assert_eq!(shown["chain"][0]["entries"], 2);
        let r = save_to(
            &home,
            &posted(&shown, |v| {
                let row = v["chain"][0].clone();
                v["chain"].as_array_mut().unwrap().push(row);
            }),
        )
        .unwrap_err();
        assert_eq!((r.status, r.code), (422, "names"));
        save_to(
            &home,
            &posted(&shown, |v| v["chain"][0]["timeout_s"] = json!(60)),
        )
        .unwrap();
        let timeouts: Vec<u64> = (config::load(home.path()).unwrap().providers.iter())
            .map(|p| match p {
                Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. } => *timeout_s,
            })
            .collect();
        assert_eq!(timeouts, [60, 60]);
    }

    /// A priced entry and a CLI entry of one name: one row with the stricter rule and `[chain]`'s
    /// model, which applies to the CLI entry. A save that leaves the model keeps it, and one that
    /// empties it removes it (Codex and CodeRabbit on #270).
    #[test]
    fn a_shared_name_shows_one_model_and_keeps_it() {
        let text = "[chain]\nmodel = { a = \"x\" }\n\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
                    limits = { usd_per_mtok_in = 1.0 }\n\n\
                    [[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        let row = &shown["chain"][0];
        assert_eq!(
            (&row["model"], &row["model_rule"]),
            (&json!("x"), &json!("fixed"))
        );
        save_to(
            &home,
            &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
        )
        .unwrap();
        assert_eq!(config::load(home.path()).unwrap().chain.model["a"], "x");
        let shown = show(home.path());
        save_to(
            &home,
            &posted(&shown, |v| v["chain"][0]["model"] = Value::Null),
        )
        .unwrap();
        assert!(config::load(home.path()).unwrap().chain.model.is_empty());
    }

    /// #274 item 1: a `[chain]` timeout past 2^53 reaches the page rounded, as a browser parses a
    /// JSON number into a double. An unrelated save sends it back rounded, and the file keeps its
    /// own value.
    #[test]
    fn a_timeout_past_2_53_survives_an_unrelated_save() {
        // The second: the entry's own value rounds as the file's `[chain]` one does (cubic on
        // #287), so taking the posted number for the entry's own would drop the file's.
        let own = "[[providers]]\nkind = \"openai\"\nname = \"groq\"\n\
                   base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                   timeout_s = 9007199254740992\n";
        for rest in ["", own] {
            let text = format!("[chain]\ntimeout_s = {{ groq = 9007199254740993 }}\n\n{rest}");
            let home = home_with(Some(&text));
            let shown = show(home.path());
            let body = posted(&shown, |v| {
                v["inject"]["session_start"] = json!(false);
                for e in v["chain"].as_array_mut().unwrap() {
                    if e["name"] == "groq" {
                        // What `JSON.parse` and then `JSON.stringify` make of it.
                        e["timeout_s"] = json!(9_007_199_254_740_992_u64);
                    }
                }
            });
            save_to(&home, &body).unwrap();
            let after = file(&home).unwrap();
            assert!(after.contains("groq = 9007199254740993"), "{after}");
            assert!(!crate::config::inject(home.path()).unwrap().session_start);
        }
    }

    /// #274 item 2: a `[chain]` model no entry of its name can take is shown, as not applied, and
    /// a save that leaves it keeps it; one that empties it removes it.
    #[test]
    fn a_model_no_entry_takes_is_shown_and_kept() {
        let text = "[chain]\nmodel = { a = \"x\" }\n\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
                    limits = { usd_per_mtok_in = 1.0 }\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        let row = &shown["chain"][0];
        assert_eq!(
            (
                &row["model"],
                &row["model_applied"],
                &row["effective_model"]
            ),
            (&json!("x"), &json!(false), &json!("m"))
        );
        save_to(
            &home,
            &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
        )
        .unwrap();
        assert!(file(&home).unwrap().contains("a = \"x\""));
        let shown = show(home.path());
        save_to(
            &home,
            &posted(&shown, |v| v["chain"][0]["model"] = Value::Null),
        )
        .unwrap();
        assert!(!file(&home).unwrap().contains("a = \"x\""));
    }

    /// A `[chain]` model the row's entry cannot take is marked, though another entry of its name
    /// takes it (cubic on #287): the row shows the first entry, and says the entries differ.
    #[test]
    fn a_model_the_rows_entry_does_not_take_is_marked() {
        let entry = |model: &str, priced: &str| {
            format!(
                "[[providers]]\nkind = \"openai\"\nname = \"a\"\nbase_url = \"https://example.invalid/v1\"\n\
                 model = \"{model}\"\n{priced}\n"
            )
        };
        let text = format!(
            "[chain]\nmodel = {{ a = \"x\" }}\n\n{}{}",
            entry("m", "limits = { usd_per_mtok_in = 1.0 }"),
            entry("n", "")
        );
        let home = home_with(Some(&text));
        let row = &show(home.path())["chain"][0];
        assert_eq!(
            (&row["model"], &row["model_applied"], &row["differs"]),
            (&json!("x"), &json!(false), &json!(["model"]))
        );
    }

    /// Entries of one name whose calls a day come from their keys, one read and one not, use
    /// different budgets, and the row says so (cubic on #287).
    #[test]
    fn entries_whose_keys_give_different_budgets_say_so() {
        let keys = tempfile::tempdir().unwrap();
        let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
        std::fs::write(&a, "# a\nkey-a\n").unwrap();
        std::fs::write(&b, "# b\nkey-b\n").unwrap();
        let entry = |f: &std::path::Path| {
            format!(
                "[[providers]]\nkind = \"openai\"\nname = \"o\"\n\
                 base_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m:free\"\nkey_file = {:?}\n",
                f.display().to_string()
            )
        };
        let home = home_with(Some(&(entry(&a) + &entry(&b))));
        let db = crate::providers_db::open(home.path()).unwrap();
        let first = (config::load(home.path()).unwrap().providers.into_iter())
            .next()
            .unwrap();
        let key = crate::provider::key_of(&first);
        crate::providers_db::set_key_limit(&db, "o", Some(1000), crate::db::now_ms(), &key)
            .unwrap();
        let row = &show(home.path())["chain"][0];
        assert_eq!(
            (&row["effective_daily_budget"], &row["differs"]),
            (&json!(200), &json!(["key_file", "daily_budget"]))
        );
    }

    /// #274 item 3: the row shows the first entry's values, and says where entries of its name
    /// differ.
    #[test]
    fn entries_of_one_name_say_where_they_differ() {
        let keys = tempfile::tempdir().unwrap();
        let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
        let text = api_entries(&[("twin", &a), ("same", &a), ("same", &a)])
            + &format!(
                "[[providers]]\nkind = \"openai\"\nname = \"twin\"\n\
                 base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"n\"\nkey_file = {:?}\n",
                b.display().to_string()
            );
        let home = home_with(Some(&text));
        let shown = show(home.path());
        assert_eq!(at(&shown, "twin")["differs"], json!(["key_file", "model"]));
        assert_eq!(at(&shown, "same")["differs"], json!([]));
    }

    /// The rule shown is the entry's own, without `[chain]`, as a save checks it: an OpenRouter
    /// entry of a paid model that `[chain]` gives a `:free` one still takes any model
    /// (OpenCodeReview on #270).
    #[test]
    fn the_rule_shown_is_the_entrys_own() {
        let text = "[chain]\nmodel = { o = \"m:free\" }\n\n[[providers]]\nkind = \"openai\"\n\
                    name = \"o\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"paid/m\"\n";
        let home = home_with(Some(text));
        let row = &show(home.path())["chain"][0];
        assert_eq!(
            (&row["model"], &row["model_rule"]),
            (&json!("m:free"), &json!("any"))
        );
    }

    /// A map written as a table of its own changes entry by entry: an entry that does not change
    /// keeps its comment and place, and one that does keeps its comment (cubic on #270).
    #[test]
    fn a_map_table_changes_entry_by_entry() {
        let text = "[chain.daily_budget]\ngroq = 100 # half\nnim = 50 # trying it\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        save_to(
            &home,
            &posted(&shown, |v| {
                for e in v["chain"].as_array_mut().unwrap() {
                    match e["name"].as_str().unwrap() {
                        "nim" => e["daily_budget"] = json!(60),
                        "groq-20b" => e["daily_budget"] = json!(20),
                        _ => {}
                    }
                }
            }),
        )
        .unwrap();
        let saved = file(&home).unwrap();
        assert!(
            saved.starts_with("[chain.daily_budget]\ngroq = 100 # half\nnim = 60 # trying it\n"),
            "{saved}"
        );
        let budgets = config::load(home.path()).unwrap().chain.daily_budget;
        assert_eq!(
            (budgets["groq"], budgets["nim"], budgets["groq-20b"]),
            (100, 60, 20)
        );
    }

    /// A key whose value changes keeps the comment after it (cubic on #270).
    #[test]
    fn a_changed_key_keeps_its_comment() {
        let text = "[chain]\noff = [\"groq\"] # paused until Friday\n\n[inject]\n\
                    session_start_chars = 3000 # short\n";
        let home = home_with(Some(text));
        let shown = show(home.path());
        save_to(
            &home,
            &posted(&shown, |v| {
                v["inject"]["session_start_chars"] = json!(4000);
                for e in v["chain"].as_array_mut().unwrap() {
                    if e["name"] == "nim" {
                        e["on"] = json!(false);
                    }
                }
            }),
        )
        .unwrap();
        let saved = file(&home).unwrap();
        assert!(
            saved.contains("off = [\"groq\", \"nim\"] # paused until Friday"),
            "{saved}"
        );
        assert!(
            saved.contains("session_start_chars = 4000 # short"),
            "{saved}"
        );
    }

    /// A file that does not parse: no form, and no save.
    #[test]
    fn a_file_that_does_not_parse_is_neither_shown_nor_written() {
        for text in [
            "[chain]\noff = 3\n",
            "[inject]\nsession_start = \"no\"\n",
            "[summary\n",
            // Read by backups and by capture's redaction alone (Codex on #270).
            "[backup]\ndir = 3\n",
            "[redaction]\nextra_rules = [{ id = \"a\", regex = '(' }]\n",
        ] {
            let home = home_with(Some(text));
            let shown = show(home.path());
            assert_eq!(shown["error"], "file_invalid", "{text}");
            let body = json!({"version": shown["version"], "inject": {"session_start": true,
                "session_start_chars": 6000, "per_prompt": false, "per_prompt_chars": 1500,
                "correction": true, "correction_chars": 800}, "capture": {"store_prompts": true,
                "tool_output": "full"}, "chain": [], "summary": {"curate": false,
                "language": "Japanese", "window_tokens": 5000, "idle_minutes": 10},
                "paid_usd_per_month": 5.0});
            let r = save_to(&home, &serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!((r.status, r.code), (422, "file_invalid"), "{text}");
            assert_eq!(file(&home).as_deref(), Some(text));
        }
    }
}
