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

pub(crate) mod claims;
pub(crate) mod embedding;
pub(crate) mod forget;
pub(crate) mod hide;
pub(crate) mod judge;
pub(crate) mod maintenance;
pub(crate) mod privacy;
mod providers;
pub(crate) mod recovery;

/// Why a save was refused: an HTTP status, a code the page puts in words, and the field it is
/// about ("chain.groq.timeout_s"), empty when it is about the whole request.
#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub field: String,
}

pub(crate) fn refused(status: u16, code: &'static str, field: impl Into<String>) -> Refusal {
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

/// All oboete config writers share this hold; a resident worker's lock is unrelated.
pub(crate) fn config_lock(home: &Path) -> std::io::Result<std::fs::File> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(state.join("config.lock"))?;
    file.lock()?;
    Ok(file)
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
pub(crate) fn parsed(
    path: &Path,
    text: &str,
) -> Option<(config::Config, config::Capture, config::Inject)> {
    let capture = config::parse_capture(Some(text)).ok()?;
    crate::redact::Rules::new(&capture.redaction).ok()?;
    crate::backup::location(text).ok()?;
    config::parse_worker(text).ok()?;
    config::parse_view(text).ok()?;
    config::parse_retention(text).ok()?;
    Some((
        config::from_text(path, text).ok()?,
        capture.capture,
        config::parse_inject(text).ok()?,
    ))
}

/// The chain's rests from one read of the ledger (69b-3c-1b): `stopped`, the names only the owner
/// ends a stop of, and `resting`, those resting until a time (ms). A name is listed only while its
/// state holds every enabled entry of it, as the curator decides (`provider::rest_holds`): a rest
/// codex's allowance set without credits does not hold an entry that may draw on them, and a
/// disabled entry is never tried, so it does not count. No ledger yet: both empty; one that does not
/// read: both null.
#[allow(clippy::type_complexity)]
fn rest_lists(
    ledger: &anyhow::Result<Option<rusqlite::Connection>>,
    providers: &[Provider],
) -> (Option<Vec<String>>, Option<Vec<Value>>) {
    let rows = match ledger {
        Ok(Some(db)) => match crate::providers_db::rests(db) {
            Ok(rows) => rows,
            Err(_) => return (None, None),
        },
        Ok(None) => return (Some(Vec::new()), Some(Vec::new())),
        Err(_) => return (None, None),
    };
    let (mut stopped, mut resting) = (Vec::new(), Vec::new());
    for (name, until, uncredited) in rows {
        let state = crate::providers_db::State {
            down_until: until,
            uncredited,
            ..Default::default()
        };
        // Listed while config.toml has an entry of the name in use and the state holds every one:
        // a name removed or renamed is not (the routes answer config.toml's names, past the
        // stores' gate; Codex's review of 69b-3c-1b), nor one whose entries are all turned off
        // (#817).
        let mut enabled = (providers.iter())
            .filter(|p| p.name() == name && p.enabled())
            .peekable();
        let holds =
            enabled.peek().is_some() && enabled.all(|p| crate::provider::rest_holds(&state, p));
        if !holds {
        } else if until == crate::providers_db::OWNER_HOLD {
            stopped.push(name);
        } else {
            resting.push(json!({"name": name, "until": until}));
        }
    }
    (Some(stopped), Some(resting))
}

/// `GET /api/rests` (69b-3c-1b): the chain's rests alone, which the page polls while Settings is
/// shown. A config.toml that does not read gives both null, as a ledger that does not read.
pub fn rests(home: &Path) -> Value {
    let cfg = config::load(home);
    // #855: curation paused by a rebuild or a rewrite of the stored records, from lock files only,
    // with whether curation is on now, so the page's words follow a change made elsewhere; a
    // config.toml that does not read stops curation too (#871, CodeRabbit on #872).
    let curate = !matches!(&cfg, Ok(c) if !c.summary.curate);
    let paused = crate::worker::curation_pause(home)
        .map(|(kind, since)| json!({"kind": kind, "since": since, "curate": curate}));
    let Ok(cfg) = cfg else {
        return json!({"stopped": null, "resting": null, "paused": paused});
    };
    let (stopped, resting) = rest_lists(&crate::providers_db::read_only(home), &cfg.providers);
    json!({"stopped": stopped, "resting": resting, "paused": paused})
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
        Some((
            parsed(&path, t)?,
            alone(&path, &doc)?,
            config::parse_worker(t).ok()?,
            config::parse_view(t).ok()?,
            provider_rows(&path, &doc)?,
            crate::backup::location(t).ok()?,
            config::parse_capture(Some(t)).ok()?.redaction,
            config::parse_retention(t).ok()?,
        ))
    });
    let Some((
        (cfg, capture, inject),
        alone,
        worker,
        view,
        providers,
        backup,
        redaction,
        retention,
    )) = read
    else {
        return json!({"version": version, "error": "file_invalid"});
    };
    let ledger = crate::providers_db::read_only(home);
    let db = ledger.as_ref().ok().and_then(Option::as_ref);
    let spend = match &ledger {
        Ok(Some(db)) => crate::providers_db::usd_this_month(db).ok(),
        Ok(None) => Some(0.0),
        Err(_) => None,
    };
    let (stopped, resting) = rest_lists(&ledger, &cfg.providers);
    // Workers AI's use as doctor counts it (W4).
    let (embed_usd, embed_requests) = match &ledger {
        Ok(Some(db)) => (
            crate::providers_db::embed_usd_this_month(db).ok(),
            crate::providers_db::calls_in_a_day(db, crate::embed::CALLS)
                .ok()
                .map(|(calls, _)| calls),
        ),
        Ok(None) => (Some(0.0), Some(0)),
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
    let mut shown = json!({
        "version": version,
        "first_run": bytes.is_none(),
        "resident_supported": cfg!(target_os = "linux"),
        "worker": {"resident": worker.resident},
        "view": {"port": view.port.get()},
        "summary": {
            "curate": cfg.summary.curate,
            "language": cfg.summary.language,
            "window_tokens": cfg.summary.window_tokens,
            "idle_minutes": cfg.summary.idle_minutes,
        },
        "my_work": {
            "curate": cfg.my_work.curate,
            "calls_a_day": cfg.my_work.calls_a_day,
            "estimate": my_work_estimate(home, &cfg),
        },
        // Decision 64: null is no cap, `inf` in config.toml, which JSON cannot carry.
        "paid_usd_per_month": cfg.paid_usd_per_month.is_finite().then_some(cfg.paid_usd_per_month),
        "gemini": cfg.gemini.map(gemini_place),
        "usd_this_month": spend,
        // G23: how the last prompt that could ask the resident viewer took its block.
        "prompt_side": crate::hook::last_side(home).map(|(at, side)| json!({"at": at, "side": side})),
        "stopped": stopped,
        "resting": resting,
        "inject": {
            "session_start": inject.session_start,
            "session_start_note": inject.session_start_note,
            "session_start_list": inject.session_start_list,
            "session_start_chars": inject.session_start_chars,
            "session_start_index": inject.session_start_index,
            "per_prompt": inject.per_prompt,
            "per_prompt_chars": inject.per_prompt_chars,
            "per_prompt_wait_ms": inject.per_prompt_wait_ms,
            "memory_lookup": inject.memory_lookup,
            "memory_instructions": inject.memory_instructions,
            "correction": inject.correction,
            "correction_chars": inject.correction_chars,
        },
        "capture": {
            "store_prompts": capture.store_prompts,
            "tool_output": tool_output(capture.tool_output),
        },
        "backup": {"dir": backup},
        // docs/retention.md: null is forever.
        "retention": {"days": retention.days},
        "redaction": {
            "enabled": redaction.enabled != Some(false),
            "extra_rules": redaction.extra_rules,
            "allowlist": redaction.allowlist,
        },
        "embedding": embedding::section(
            home,
            &cfg.embedding,
            embed_usd,
            embed_requests,
            crate::model_fetch::BGE_M3,
            crate::model_fetch::RUNTIME.as_ref(),
        ),
        "judge": judge::section(cfg.judge.as_ref(), &ledger),
        "chain": chain,
        "providers": providers,
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
            "session_start_index": range(config::SESSION_START_INDEX),
            "per_prompt_chars": range(config::PER_PROMPT_CHARS),
            "per_prompt_wait_ms": range(config::PER_PROMPT_WAIT_MS),
            "correction_chars": range(config::CORRECTION_CHARS),
            "daily_budget": [BUDGET.start(), BUDGET.end()],
            "daily_requests": [BUDGET.start(), BUDGET.end()],
            "timeout_s": [TIMEOUT_S.start(), TIMEOUT_S.end()],
            "view_port": [1, u16::MAX],
            "retention_days": [config::RETENTION_DAYS.start(), config::RETENTION_DAYS.end()],
            "my_work_calls": [MY_WORK_CALLS.start(), MY_WORK_CALLS.end()],
            // Calls a day that no day reaches: the no cap of each calls-a-day field (decision 64).
            "no_cap": config::no_daily_cap(),
        },
    });
    // Decision 81: the summaries' language when `[summary] language` is not set.
    shown["language_default"] = config::os_language().into();
    shown
}

/// docs/my-work-model.md A6: what My Work's jobs would cost on this home, priced at the chain's
/// first paid entry; none before a store exists or when it does not read.
fn my_work_estimate(home: &Path, cfg: &config::Config) -> Value {
    let Some(Ok(e)) = crate::my_work_run::home_estimate(home, cfg) else {
        return Value::Null;
    };
    // With no cap of calls a day there is no most (decision 64): null, not 4294967295 calls.
    let capped = cfg.my_work.calls_a_day != config::no_daily_cap();
    json!({
        "calls": [e.calls.0, capped.then_some(e.calls.1)],
        "usd": e.usd.map(|(typical, most)| json!([typical, capped.then_some(most)])),
        "priced": e.priced.map(|(name, model)| json!({"name": name, "model": model})),
    })
}

fn range(r: std::ops::RangeInclusive<usize>) -> [usize; 2] {
    [*r.start(), *r.end()]
}

fn cli_path_state(cli: &str) -> &'static str {
    match crate::setup::launch_found(&[cli]) {
        Some(true) => "on-path",
        Some(false) => "not-on-path",
        None => "unknown",
    }
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

/// A configured endpoint is shown only if it cannot contain URL credentials or query secrets.
/// HTTP is supported for numeric loopback hosts; other destinations require HTTPS.
fn endpoint_supported(url: &str) -> bool {
    crate::provider::endpoint_supported(url)
}

/// Raw array order is the editor's identity. The effective order may have been changed by
/// name-group overlays; duplicates retain their relative order under that stable sort.
fn provider_rows(path: &Path, doc: &toml_edit::DocumentMut) -> Option<Vec<Value>> {
    let mut raw = doc.clone();
    raw.remove("chain");
    raw.remove("gemini");
    let base = config::from_text(path, &raw.to_string()).ok()?;
    let effective = config::from_text(path, &doc.to_string()).ok()?;
    let source = if doc.contains_key("providers") {
        "file"
    } else {
        "builtin"
    };
    let mut rows = Vec::new();
    for (index, p) in base.providers.iter().enumerate() {
        let occurrence = base.providers[..index]
            .iter()
            .filter(|other| other.name() == p.name())
            .count();
        let (order, applied) = effective
            .providers
            .iter()
            .enumerate()
            .filter(|(_, other)| other.name() == p.name())
            .nth(occurrence)?;
        rows.push(provider_row(
            p,
            applied,
            &effective.chain,
            source,
            index,
            order,
        ));
    }
    if !base.providers.iter().any(|p| p.name() == "gemini")
        && let Some((order, p)) = effective
            .providers
            .iter()
            .enumerate()
            .find(|(_, p)| p.name() == "gemini")
    {
        let before_overlay = alone(path, doc)?;
        let own = before_overlay
            .providers
            .iter()
            .find(|p| p.name() == "gemini")?;
        rows.push(provider_row(own, p, &effective.chain, "gemini", 0, order));
    }
    Some(rows)
}

fn provider_row(
    saved: &Provider,
    effective: &Provider,
    chain: &ChainOverlay,
    source: &str,
    index: usize,
    order: usize,
) -> Value {
    let fields = |p: &Provider| match p {
        Provider::Openai {
            base_url,
            api,
            key_file,
            model,
            daily_budget,
            timeout_s,
            retry_429,
            subscription,
            ..
        } => json!({
            "kind": "openai", "api": api, "base_url": endpoint_supported(base_url).then_some(base_url),
            "endpoint_supported": endpoint_supported(base_url), "model": model,
            "daily_budget": daily_budget, "timeout_s": timeout_s, "retry_429": retry_429,
            "subscription": subscription, "key_file": key_file,
            "key": match key_file {
                Some(f) if f.exists() => "ok", Some(_) => "missing", None => "none",
            },
        }),
        Provider::Cli {
            cli,
            model,
            timeout_s,
            credits,
            ..
        } => json!({
            "kind": "cli", "cli": cli, "model": model, "timeout_s": timeout_s,
            "subscription": true, "credits": credits, "key": cli_path_state(cli),
        }),
    };
    let limits = saved.limits();
    let mut own = fields(saved);
    own["enabled"] = json!(saved.enabled());
    own["limits"] = json!({
        "max_request_tokens": limits.max_request_tokens, "daily_tokens": limits.daily_tokens,
        "usd_per_mtok_in": limits.usd_per_mtok_in, "usd_per_mtok_out": limits.usd_per_mtok_out,
        "max_output_tokens": limits.max_output_tokens,
    });
    let mut applied = fields(effective);
    applied["on"] = json!(effective.enabled() && !chain.turns_off(saved.name()));
    applied["order"] = json!(order);
    json!({"selector": {"source": source, "index": index}, "name": saved.name(),
        "saved": own, "effective": applied})
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
            cli_path_state(cli),
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
        "subscription": same.iter().any(|p| p.subscription()),
        "on": !chain.turns_off(name),
        "key": key,
        "key_file": key_file,
        "model": chain.model.get(name),
        "model_applied": applied,
        "effective_model": model,
        "model_rule": rule,
        "daily_budget": chain.daily_budget.get(name),
        "effective_daily_budget": (budget != config::no_daily_cap()).then_some(budget),
        // Whether any entry of the name stops at a calls a day, where `effective_daily_budget` is the
        // first entry's: a subscription's No cap switch shows while one does (#800 item 3, 69b-3c).
        "capped": same.iter().any(|q| budget_of(q) != config::no_daily_cap()),
        "budget_from_key": p.budget_from_key(),
        "timeout_s": chain.timeout_s.get(name),
        "effective_timeout_s": timeout_s,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    version: String,
    /// An older page omitting this field keeps its saved mode.
    worker: Option<WorkerIn>,
    /// Older pages omitting this field keep the saved port.
    view: Option<ViewIn>,
    summary: SummaryIn,
    /// Null is no cap (decision 64). Left out (None) is refused, as before, not read as no cap.
    #[serde(default, deserialize_with = "given")]
    paid_usd_per_month: Option<Option<f64>>,
    gemini: Option<config::GeminiPlace>,
    inject: InjectIn,
    capture: CaptureIn,
    /// Omission preserves the existing location, including legacy spellings.
    backup: Option<BackupIn>,
    /// Older pages leave the custom rules and exact-value exceptions alone.
    redaction: Option<config::Redaction>,
    /// Older pages leave `[retention]` as it is (docs/retention.md); a null is refused, not read
    /// as left out.
    #[serde(default, deserialize_with = "given")]
    retention: Option<RetentionIn>,
    /// Workers AI's account and caps; older pages leave them alone (W4).
    embedding: Option<embedding::Values>,
    /// `[my_work]` (docs/my-work-model.md A6); older pages leave it alone.
    my_work: Option<MyWorkIn>,
    /// `[judge] enabled` (docs/my-work-model.md J1); older pages leave it alone.
    judge: Option<JudgeIn>,
    /// Every chain entry once, in the order the page wants.
    chain: Vec<EntryIn>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupIn {
    /// Null removes the override and follows backup::dir's default.
    dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionIn {
    /// Null removes the key: forever. Left out (None) is refused, not read as forever.
    #[serde(default, deserialize_with = "given")]
    days: Option<Option<u32>>,
}

/// A field that may be left out (None) but is read as a `T` when given, so a null where `T` takes
/// none is refused rather than read as left out.
fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerIn {
    resident: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewIn {
    port: std::num::NonZeroU16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MyWorkIn {
    curate: bool,
    calls_a_day: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeIn {
    enabled: bool,
    /// `[judge] daily_budget`; older pages leave it alone.
    #[serde(default)]
    daily_budget: Option<u32>,
    /// `[judge] destination` (J7); older pages leave it alone.
    #[serde(default)]
    destination: Option<config::Destination>,
}

/// `[judge]` as the page saves it (J1, J7).
fn judge_in(
    doc: &mut toml_edit::DocumentMut,
    j: &JudgeIn,
    saved: Option<&config::Judge>,
) -> Result<(), Refusal> {
    let Some(saved) = saved else {
        return judge_table_in(doc, j);
    };
    if let Some(n) = j.daily_budget.filter(|n| *n != saved.daily_budget) {
        judge_budget_in(doc, n)?;
    }
    let to = j.destination.unwrap_or(saved.destination);
    if let Some(refusal) = judge_on_refused(j, saved, to) {
        return Err(refusal);
    }
    let switching = to != saved.destination;
    if switching {
        put(doc, "judge", "destination", to.id().into());
    }
    if saved.enabled != j.enabled {
        put(doc, "judge", "enabled", j.enabled.into());
    }
    Ok(())
}

/// The refusal of a judge turned on, or sent to `to` while on, without what that destination sends
/// with (J7): its key, and Cloudflare's account id. One on and left as it is stays as it was.
fn judge_on_refused(
    j: &JudgeIn,
    saved: &config::Judge,
    to: config::Destination,
) -> Option<Refusal> {
    let switching = to != saved.destination;
    let chosen = config::Judge {
        enabled: true,
        destination: to,
        ..saved.clone()
    };
    if !j.enabled || !(switching || !saved.enabled) || crate::judge::ready(&chosen) {
        return None;
    }
    let field = if switching {
        "judge.destination"
    } else {
        "judge.enabled"
    };
    Some(refused(422, "missing_key", field))
}

/// With no `[judge]`, a key is set first (`judge::save_key`), except for d1 on this PC, which needs
/// none: turned on, it writes the table, `enabled` with it.
fn judge_table_in(doc: &mut toml_edit::DocumentMut, j: &JudgeIn) -> Result<(), Refusal> {
    match (j.enabled, j.destination) {
        (true, Some(config::Destination::D1Local)) => {}
        (true, _) => return Err(refused(422, "missing_key", "judge.enabled")),
        _ if j.daily_budget.is_some() => {
            return Err(refused(422, "missing_key", "judge.daily_budget"));
        }
        _ => return Ok(()),
    }
    if let Some(n) = j.daily_budget {
        judge_budget_in(doc, n)?;
    }
    put(doc, "judge", "destination", "d1-local".into());
    put(doc, "judge", "enabled", true.into());
    Ok(())
}

/// `[judge] daily_budget`: one in the page's range, or no cap (decision 64).
fn judge_budget_in(doc: &mut toml_edit::DocumentMut, n: u32) -> Result<(), Refusal> {
    if !calls_cap(n, &BUDGET) {
        return Err(refused(422, "range", "judge.daily_budget"));
    }
    put(doc, "judge", "daily_budget", i64::from(n).into());
    Ok(())
}

/// `[my_work] calls_a_day` as the page takes it: one call a day at the least.
const MY_WORK_CALLS: std::ops::RangeInclusive<u32> = 1..=10_000;

/// A calls-a-day cap the page takes: one in `range`, or none (decision 64).
fn calls_cap(n: u32, range: &std::ops::RangeInclusive<u32>) -> bool {
    range.contains(&n) || n == config::no_daily_cap()
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
    /// Older pages omit the terminal note choice and keep its saved value.
    session_start_note: Option<bool>,
    /// Older pages omit the terminal list choice and keep its saved value (decision 51).
    session_start_list: Option<bool>,
    session_start_chars: usize,
    /// Older pages omit the index's lines and keep their saved value (decision 75).
    session_start_index: Option<usize>,
    per_prompt: bool,
    per_prompt_chars: usize,
    /// Older pages omit it and keep its saved value (G23).
    per_prompt_wait_ms: Option<usize>,
    /// Older pages omit the memory lookup's and the memory instructions' switches and keep their
    /// saved values.
    memory_lookup: Option<bool>,
    memory_instructions: Option<bool>,
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

/// The body of a key typed for one table's own key file (`[embedding]`, `[judge]`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedKey {
    pub key: String,
    pub version: String,
}

/// A key typed on the page, written as a provider's is (`keyfile::managed`: a new owner-only file
/// outside the store), and `[<table>] key_file` pointed at it with `then`'s other changes, against
/// the version the page showed. The old file is left as it is. The answer is the settings with the
/// key's state: the key is in no answer. `authorized` is the request's token checked again under
/// config.lock, as for a choice. `homes` is where the file may go (the owner's home and
/// `XDG_DATA_HOME`), the owner's own when None (tests give their own).
pub(super) fn managed_key(
    home: &Path,
    auth: (&Mutex<()>, &dyn Fn() -> bool),
    body: &[u8],
    homes: Option<(&Path, Option<&Path>)>,
    table: &str,
    then: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<Value, Refusal> {
    let posted: ManagedKey =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    managed_key_at(home, auth, &posted, homes, (table, "key_file"), then)
}

/// `managed_key` for a body its caller read, with the key's file in `[<table>] <field>` (the
/// judge's services each have their own, J7).
pub(super) fn managed_key_at(
    home: &Path,
    (saving, authorized): (&Mutex<()>, &dyn Fn() -> bool),
    posted: &ManagedKey,
    homes: Option<(&Path, Option<&Path>)>,
    (table, key_field): (&str, &str),
    then: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<Value, Refusal> {
    let owner_home = crate::config::home_dir();
    let data_home = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from);
    let (owner, data) = homes.unwrap_or((&owner_home, data_home.as_deref()));
    let field = format!("{table}.key");
    if !crate::keyfile::valid(&posted.key) {
        return Err(refused(422, "bad_key", field));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _config = config_lock(home).map_err(|_| refused(500, "write_failed", ""))?;
    if !authorized() {
        return Err(refused(401, "unauthorized", ""));
    }
    let was = bytes(home).map_err(|_| invalid())?;
    if version(was.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    let text = utf8(was.as_deref()).ok_or_else(invalid)?;
    parsed(&home.join("config.toml"), text).ok_or_else(invalid)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid())?;
    let new_key = crate::keyfile::managed(&posted.key, home, owner, data)
        .map_err(|r| refused(r.status(), r.code(), field.clone()))?;
    let filename =
        (new_key.path().to_str()).ok_or_else(|| refused(422, "not_utf8", field.clone()))?;
    put(&mut doc, table, key_field, filename.into());
    then(&mut doc);
    let mut answer = providers::commit(home, &posted.version, doc)?;
    answer["key_saved"] = json!({"durable": new_key.durable});
    new_key.retain();
    Ok(answer)
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
    let _config = config_lock(home).map_err(|_| refused(500, "write_failed", ""))?;
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
    let readers: Vec<&str> = (cfg.providers.iter())
        .filter(|p| matches!(p, Provider::Openai { key_file: Some(f), .. } if f == file))
        .map(|p| p.name())
        .collect();
    key_saved(home, &readers);
    Ok(json!({"entry": posted.entry, "key": "ok", "durable": written.durable}))
}

/// Parity row 11: a saved key ends the rests of the entries that read it, and what waits on time
/// or on the owner is tried at the worker's next pass (`providers_db::key_saved`). The key is
/// saved whatever happens here: an error is said on stderr, and their rests run out by themselves.
pub(crate) fn key_saved(home: &Path, entries: &[&str]) {
    if entries.is_empty() || !home.join("providers.db").exists() {
        return;
    }
    let ended = (|| -> anyhow::Result<()> {
        let mut db = crate::providers_db::open(home)?;
        let tx = db.transaction()?;
        crate::providers_db::key_saved(&tx, entries)?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = ended {
        eprintln!("oboete: a saved key's rest not ended: {e:#}");
    }
}

/// Changes one physical or virtual provider entry against the version the page read.
pub fn save_provider(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    providers::save(home, saving, body)
}

pub fn save_provider_key(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let owner = config::home_dir();
    let data = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from);
    providers::save_key_at(home, saving, body, &owner, data.as_deref())
}

pub fn preview_provider_test(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    providers::preview(home, body)
}

pub fn test_provider(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    providers::test(home, body)
}

/// Decision 78: an entry's model list.
pub fn provider_models(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    providers::models(home, body)
}

/// `save_held` with its holds taken, as the tests save.
#[cfg(test)]
pub fn save(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    // Two tabs saving at once: one after the other, and the second finds the file changed.
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _config = config_lock(home).map_err(|_| refused(500, "write_failed", ""))?;
    save_held(home, body)
}

/// The page's save: checks the request against the file it read (`version`), writes the page's
/// keys into it and checks the result as every reader parses it before it replaces the file, and
/// answers the new `show`. A value that equals the entry's own is not written, so it keeps
/// following later changes to the defaults. The caller holds `saving` and config.lock, under
/// which the viewer checks the page's token first.
pub fn save_held(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    if let Some(staged) = stage_held(home, body)? {
        staged
            .commit()
            .map_err(|_| refused(500, "write_failed", ""))?;
    }
    Ok(show(home))
}

/// `save_held` short of its rename: every refusal it gives, and the new file staged beside
/// config.toml (`None` when nothing would change). A port saved on the resident page renames it
/// only once the new token is in place (docs/resident.md R6).
pub(crate) fn stage_held(home: &Path, body: &[u8]) -> Result<Option<StagedSettings>, Refusal> {
    let posted: Save = serde_json::from_slice(body).map_err(|e| match e.classify() {
        serde_json::error::Category::Data => refused(422, "type", ""),
        _ => refused(400, "bad_request", ""),
    })?;
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
    write_privacy_config(&mut doc, text, &posted)?;
    if let Some(values) = &posted.embedding {
        embedding::write(&mut doc, values, &now.embedding)?;
    }
    // The order changes when the chain's does, not when `order` would be spelled another way.
    let reordered = !(posted.chain.iter().map(|e| e.name.as_str())).eq(names(&now.providers));
    if let Some(worker) = &posted.worker {
        let saved = doc
            .get("worker")
            .and_then(|table| table.get("resident"))
            .and_then(toml_edit::Item::as_bool);
        if saved != Some(worker.resident) {
            // An explicit first off choice is a real setting too, even though off was the default.
            put(&mut doc, "worker", "resident", worker.resident.into());
        }
    }
    if let Some(view) = &posted.view {
        let port = i64::from(view.port.get());
        let saved = doc
            .get("view")
            .and_then(|table| table.get("port"))
            .and_then(toml_edit::Item::as_integer);
        if saved != Some(port) {
            put(&mut doc, "view", "port", port.into());
        }
    }
    if let Some(m) = &posted.my_work {
        // A value the file holds stays as it is when it is not changed, in range or not, as a
        // provider's daily budget does (#680).
        if m.calls_a_day != now.my_work.calls_a_day && !calls_cap(m.calls_a_day, &MY_WORK_CALLS) {
            return Err(refused(422, "range", "my_work.calls_a_day"));
        }
        // Turning it on is the owner's act on this page, its estimate shown first (A6).
        if m.curate != now.my_work.curate {
            put(&mut doc, "my_work", "curate", m.curate.into());
        }
        if m.calls_a_day != now.my_work.calls_a_day {
            put(
                &mut doc,
                "my_work",
                "calls_a_day",
                i64::from(m.calls_a_day).into(),
            );
        }
    }
    if let Some(j) = &posted.judge {
        judge_in(&mut doc, j, now.judge.as_ref())?;
    }
    // A null is no cap, written `inf` (decision 64); left out, `checked` refused it.
    if let Some(cap) = posted.paid_usd_per_month {
        let cap = cap.unwrap_or(config::no_monthly_cap());
        if cap != now.paid_usd_per_month {
            put_root(&mut doc, "paid_usd_per_month", Some(cap.into()));
        }
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
    // Decision 81: left empty, the computer's language: the key goes, not an empty language, its
    // comment kept as the other removals keep theirs (cubic on #864).
    if s.language.trim().is_empty() {
        let comments = (doc.get("summary").and_then(|t| t.as_table_like()))
            .map(|table| key_comments(table, "language"))
            .unwrap_or_default();
        if let Some(t) = doc.get_mut("summary").and_then(|t| t.as_table_like_mut()) {
            t.remove("language");
        }
        keep_comments(&mut doc, &comments);
    } else if s.language != now.summary.language {
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
    for (key, posted, was) in [
        (
            "session_start_note",
            i.session_start_note,
            inject.session_start_note,
        ),
        (
            "session_start_list",
            i.session_start_list,
            inject.session_start_list,
        ),
    ] {
        if let Some(now) = posted
            && now != was
        {
            put(&mut doc, "inject", key, now.into());
        }
    }
    for (key, now, was) in [
        ("memory_lookup", i.memory_lookup, inject.memory_lookup),
        (
            "memory_instructions",
            i.memory_instructions,
            inject.memory_instructions,
        ),
    ] {
        if let Some(on) = now
            && on != was
        {
            put(&mut doc, "inject", key, on.into());
        }
    }
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
            "session_start_index",
            i.session_start_index.unwrap_or(inject.session_start_index),
            inject.session_start_index,
        ),
        (
            "per_prompt_chars",
            i.per_prompt_chars,
            inject.per_prompt_chars,
        ),
        (
            "per_prompt_wait_ms",
            i.per_prompt_wait_ms.unwrap_or(inject.per_prompt_wait_ms),
            inject.per_prompt_wait_ms,
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
        return Ok(None);
    }
    parsed(&path, &candidate).ok_or_else(invalid)?;
    // Owner decision 43: a change of the redaction rules (the switch turned on, say) waits for each
    // call already admitted to send and holds the dispatch lock until the file is replaced, so no
    // text gated under the rules before leaves after the save (Codex on #502).
    let rules = |text: &str| {
        config::parse_capture(Some(text))
            .ok()
            .and_then(|c| crate::redact::Rules::new(&c.redaction).ok())
            .map(|r| r.version().to_owned())
    };
    let dispatch = (rules(text) != rules(&candidate))
        .then(|| crate::dispatch::exclusive(home))
        .transpose()
        .map_err(|_| refused(409, "dispatch_busy", ""))?;
    let staged =
        crate::setup::stage(&path, &candidate).map_err(|_| refused(500, "write_failed", ""))?;
    // A hand edit or another viewer between the read and here is not overwritten (the temp file
    // goes when `staged` drops).
    // A hand edit can still land between this check and rename; oboete's writers hold config.lock.
    if version(bytes(home).map_err(|_| invalid())?.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    Ok(Some(StagedSettings {
        staged,
        _dispatch: dispatch,
    }))
}

/// The page's new config.toml staged beside it, with the dispatch lock while it changes the
/// redaction rules (`stage_held`), released once the file is replaced.
pub(crate) struct StagedSettings {
    staged: crate::setup::Staged,
    _dispatch: Option<std::fs::File>,
}

impl StagedSettings {
    pub(crate) fn commit(self) -> anyhow::Result<()> {
        self.staged.commit()
    }
}

/// `[view] port` set to `port`, the rest of config.toml as it was (`oboete view --new-token`,
/// docs/resident.md R6); a file changed by hand between the read and the write is not overwritten.
/// The caller holds config.lock (`view::rotate`).
pub fn set_view_port(home: &Path, port: u16) -> anyhow::Result<()> {
    let path = home.join("config.toml");
    let was = bytes(home)?;
    let text = utf8(was.as_deref()).ok_or_else(|| anyhow::anyhow!("config.toml is not UTF-8"))?;
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    put(&mut doc, "view", "port", i64::from(port).into());
    let candidate = doc.to_string();
    config::parse_capture(Some(&candidate))?;
    let staged = crate::setup::stage(&path, &candidate)?;
    anyhow::ensure!(
        version(bytes(home)?.as_deref()) == version(was.as_deref()),
        "config.toml changed while the port was written; try again"
    );
    staged.commit()
}

/// Decision 81: the computer's language written as `[summary] language` into a config.toml that
/// has none (`oboete setup`), so a later change of the computer's locale changes no summary's
/// language. A value already there is kept, and a home with no config.toml is left without one.
/// The language written, if any.
pub fn pin_language(home: &Path) -> anyhow::Result<Option<String>> {
    let _config = config_lock(home)?;
    let path = home.join("config.toml");
    let Some(was) = bytes(home)? else {
        return Ok(None);
    };
    let text = utf8(Some(&was)).ok_or_else(|| anyhow::anyhow!("config.toml is not UTF-8"))?;
    // As the other config writers: a file one of its readers refuses is left as it is, and the error
    // names none of its values, a header's key or a masking rule (Codex on decision 81).
    anyhow::ensure!(parsed(&path, text).is_some(), "config.toml is invalid");
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| anyhow::anyhow!("config.toml is invalid"))?;
    // A language left blank is not set, as `config` reads it (cubic on #864).
    let set = (doc.get("summary").and_then(|t| t.as_table_like()))
        .and_then(|t| t.get("language")?.as_str())
        .is_some_and(|l| !l.trim().is_empty());
    if set {
        return Ok(None);
    }
    let language = config::os_language();
    put(&mut doc, "summary", "language", language.as_str().into());
    let candidate = doc.to_string();
    anyhow::ensure!(
        parsed(&path, &candidate).is_some(),
        "config.toml is invalid"
    );
    let staged = crate::setup::stage(&path, &candidate)?;
    anyhow::ensure!(
        version(bytes(home)?.as_deref()) == version(Some(&was)),
        "config.toml changed while the language was written; try again"
    );
    staged.commit()?;
    Ok(Some(language))
}

/// `[embedding] provider` set to `provider`, the rest of config.toml as it was (`oboete setup
/// --embeddings`). False when it already was. A file one of its readers would refuse is not
/// written, and the refusal is the error (`workers-ai` without its account).
pub fn set_embedding_provider(home: &Path, provider: &str) -> anyhow::Result<bool> {
    let _config = config_lock(home)?;
    set_embedding_provider_held(home, provider)
}

/// `set_embedding_provider` for a caller holding config.lock (the settings page, W4).
pub(crate) fn set_embedding_provider_held(home: &Path, provider: &str) -> anyhow::Result<bool> {
    let path = home.join("config.toml");
    let was = bytes(home)?;
    let text = utf8(was.as_deref()).ok_or_else(|| anyhow::anyhow!("config.toml is not UTF-8"))?;
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let now = (doc.get("embedding").and_then(|t| t.get("provider"))).and_then(|v| v.as_str());
    if now.unwrap_or("none") == provider {
        return Ok(false);
    }
    put(&mut doc, "embedding", "provider", provider.into());
    let candidate = doc.to_string();
    config::from_text(&path, &candidate)?;
    anyhow::ensure!(
        parsed(&path, &candidate).is_some(),
        "config.toml is invalid"
    );
    let staged = crate::setup::stage(&path, &candidate)?;
    anyhow::ensure!(
        version(bytes(home)?.as_deref()) == version(was.as_deref()),
        "config.toml changed while the embedder was written; try again"
    );
    staged.commit()?;
    Ok(true)
}

/// Parity row 50: `setup all --remove` takes `[worker] resident = true` out of config.toml, so the
/// resident page leaves at its next tick (R4) and no worker stays when idle; a later setup puts it
/// back (`resident_defaults`). Any other value is left as it is. Whether it was taken out.
pub fn resident_off(home: &Path) -> anyhow::Result<bool> {
    let _config = config_lock(home)?;
    let path = home.join("config.toml");
    let was = bytes(home)?;
    let text = utf8(was.as_deref()).ok_or_else(|| anyhow::anyhow!("config.toml is not UTF-8"))?;
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let on = doc
        .get("worker")
        .and_then(|t| t.get("resident"))
        .and_then(|v| v.as_bool());
    if on != Some(true) {
        return Ok(false);
    }
    if let Some(worker) = doc.get_mut("worker").and_then(|t| t.as_table_like_mut()) {
        worker.remove("resident");
    }
    let candidate = doc.to_string();
    anyhow::ensure!(
        parsed(&path, &candidate).is_some(),
        "config.toml is invalid"
    );
    let staged = crate::setup::stage(&path, &candidate)?;
    anyhow::ensure!(
        version(bytes(home)?.as_deref()) == version(was.as_deref()),
        "config.toml changed during setup; try again"
    );
    staged.commit()?;
    Ok(true)
}

/// R2/A111: setup fills only absent values. Reading agent wiring or removing one agent's never
/// does this; removing every agent's takes `resident` out (`resident_off`).
#[cfg(target_os = "linux")]
pub fn resident_defaults(home: &Path) -> anyhow::Result<(config::Worker, config::View)> {
    let _config = config_lock(home)?;
    let path = home.join("config.toml");
    let was = bytes(home)?;
    let text = utf8(was.as_deref()).ok_or_else(|| anyhow::anyhow!("config.toml is not UTF-8"))?;
    anyhow::ensure!(parsed(&path, text).is_some(), "config.toml is invalid");
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    if doc.get("worker").and_then(|t| t.get("resident")).is_none() {
        put(&mut doc, "worker", "resident", true.into());
    }
    if doc.get("view").and_then(|t| t.get("port")).is_none() {
        put(
            &mut doc,
            "view",
            "port",
            i64::from(config::View::default().port.get()).into(),
        );
    }
    let candidate = doc.to_string();
    anyhow::ensure!(
        parsed(&path, &candidate).is_some(),
        "config.toml is invalid"
    );
    if candidate != text {
        let staged = crate::setup::stage(&path, &candidate)?;
        anyhow::ensure!(
            version(bytes(home)?.as_deref()) == version(was.as_deref()),
            "config.toml changed during setup; try again"
        );
        staged.commit()?;
    }
    Ok((
        config::parse_worker(&candidate)?,
        config::parse_view(&candidate)?,
    ))
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
    let Some(cap) = posted.paid_usd_per_month else {
        return Err(refused(422, "type", "paid_usd_per_month"));
    };
    if cap.is_some_and(|v| !v.is_finite() || v < 0.0) {
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
            "inject.session_start_index",
            i.session_start_index
                .unwrap_or(*config::SESSION_START_INDEX.start()),
            config::SESSION_START_INDEX,
        ),
        (
            "inject.per_prompt_chars",
            i.per_prompt_chars,
            config::PER_PROMPT_CHARS,
        ),
        (
            "inject.per_prompt_wait_ms",
            i.per_prompt_wait_ms
                .unwrap_or(*config::PER_PROMPT_WAIT_MS.start()),
            config::PER_PROMPT_WAIT_MS,
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
    // A `judge:` entry written by hand is never in the chain (`config::load_chain`).
    let judge = |name: &str| name.starts_with(crate::config::JUDGE_PREFIX);
    let in_use = |p: &Provider| p.enabled() && !now.chain.turns_off(p.name()) && !judge(p.name());
    if posted.chain.iter().all(|e| !e.on || judge(&e.name)) && now.providers.iter().any(in_use) {
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
        if !had && !calls_cap(n, &BUDGET) {
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
        let comments = key_comments(doc.as_table(), key);
        doc.remove(key);
        keep_comments(doc, &comments);
    }
}

fn key_comments(table: &dyn toml_edit::TableLike, key: &str) -> String {
    let prefix = table
        .key(key)
        .and_then(|name| name.leaf_decor().prefix())
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let suffix = table
        .get(key)
        .and_then(toml_edit::Item::as_value)
        .and_then(|v| v.decor().suffix())
        .and_then(|s| s.as_str())
        .unwrap_or("");
    format!("{prefix}{suffix}")
}

fn keep_comments(doc: &mut toml_edit::DocumentMut, comments: &str) {
    if comments.contains('#') {
        doc.set_trailing(format!(
            "{}\n{comments}\n",
            doc.trailing().as_str().unwrap_or("")
        ));
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

/// Write the optional backup and redaction choices using the existing preservation rules.
fn write_privacy_config(
    doc: &mut toml_edit::DocumentMut,
    text: &str,
    posted: &Save,
) -> Result<(), Refusal> {
    if let Some(backup) = &posted.backup {
        let old = crate::backup::location(text).map_err(|_| invalid())?;
        if backup.dir.as_deref().map(Path::new) != old.as_deref() {
            match &backup.dir {
                Some(dir) => put(doc, "backup", "dir", dir.as_str().into()),
                None => {
                    let comments = doc
                        .get("backup")
                        .and_then(toml_edit::Item::as_table)
                        .map(|table| key_comments(table, "dir"))
                        .unwrap_or_default();
                    if let Some(table) = doc
                        .get_mut("backup")
                        .and_then(toml_edit::Item::as_table_like_mut)
                    {
                        table.remove("dir");
                    }
                    keep_comments(doc, &comments);
                }
            }
        }
    }
    if let Some(retention) = &posted.retention {
        let Some(days) = retention.days else {
            return Err(refused(422, "type", "retention.days"));
        };
        if days.is_some_and(|d| !config::RETENTION_DAYS.contains(&d)) {
            return Err(refused(422, "range", "retention.days"));
        }
        let old = config::parse_retention(text).map_err(|_| invalid())?.days;
        if days != old {
            match days {
                Some(days) => put(doc, "retention", "days", i64::from(days).into()),
                None => {
                    let comments = doc
                        .get("retention")
                        .and_then(toml_edit::Item::as_table)
                        .map(|table| key_comments(table, "days"))
                        .unwrap_or_default();
                    if let Some(table) = doc
                        .get_mut("retention")
                        .and_then(toml_edit::Item::as_table_like_mut)
                    {
                        table.remove("days");
                    }
                    keep_comments(doc, &comments);
                }
            }
        }
    }
    if let Some(redaction) = &posted.redaction {
        crate::redact::Rules::new(redaction)
            .map_err(|_| refused(422, "redaction_invalid", "redaction"))?;
        let old = config::parse_capture(Some(text))
            .map_err(|_| invalid())?
            .redaction;
        // Unnamed (an older page), the switch keeps what the file says (owner decision 43).
        if let Some(enabled) = redaction.enabled
            && enabled != (old.enabled != Some(false))
        {
            put(doc, "redaction", "enabled", enabled.into());
        }
        if serde_json::to_value(&redaction.extra_rules).expect("rules serialize")
            != serde_json::to_value(&old.extra_rules).expect("rules serialize")
        {
            write_extra_rules(doc, &old.extra_rules, &redaction.extra_rules);
        }
        if redaction.allowlist != old.allowlist {
            put(
                doc,
                "redaction",
                "allowlist",
                toml_edit::Value::Array(redaction.allowlist.iter().map(String::as_str).collect()),
            );
        }
    }
    Ok(())
}

/// Keep matching ids on their tables; renamed rows reuse the remaining old tables in order.
/// Both TOML spellings retain comments, formatting and unchanged optional values.
fn write_extra_rules(
    doc: &mut toml_edit::DocumentMut,
    old: &[config::ExtraRule],
    rules: &[config::ExtraRule],
) {
    let mut comments = String::new();
    // ponytail: editable ids leave bulk comment affinity ambiguous; add original-row metadata if needed.
    let mut renamed = old
        .iter()
        .filter(|previous| !rules.iter().any(|rule| rule.id == previous.id));
    let saved = doc
        .get("redaction")
        .and_then(|r| r.get("extra_rules"))
        .cloned();
    if let Some(tables) = saved.as_ref().and_then(toml_edit::Item::as_array_of_tables) {
        let mut next = toml_edit::ArrayOfTables::new();
        let mut reordered = false;
        for (index, rule) in rules.iter().enumerate() {
            let previous = old
                .iter()
                .find(|r| r.id == rule.id)
                .or_else(|| renamed.next());
            reordered |= old.get(index).map(|r| &r.id) != previous.map(|r| &r.id);
            let mut table = previous
                .and_then(|previous| {
                    tables.iter().find(|t| {
                        t.get("id").and_then(toml_edit::Item::as_str) == Some(&previous.id)
                    })
                })
                .cloned()
                .unwrap_or_default();
            comments.push_str(&write_rule(&mut table, previous, rule));
            next.push(table);
        }
        if reordered {
            for table in next.iter_mut() {
                table.set_position(Some(isize::MAX));
            }
        }
        doc["redaction"]["extra_rules"] = toml_edit::Item::ArrayOfTables(next);
    } else {
        let saved = saved.as_ref().and_then(toml_edit::Item::as_array);
        let mut next = saved.cloned().unwrap_or_default();
        next.clear();
        for rule in rules {
            let previous = old
                .iter()
                .find(|r| r.id == rule.id)
                .or_else(|| renamed.next());
            let mut table = previous
                .and_then(|previous| {
                    saved?.iter().find_map(|v| {
                        v.as_inline_table().filter(|t| {
                            t.get("id").and_then(toml_edit::Value::as_str) == Some(&previous.id)
                        })
                    })
                })
                .cloned()
                .unwrap_or_default();
            comments.push_str(&write_rule(&mut table, previous, rule));
            next.push_formatted(toml_edit::Value::InlineTable(table));
        }
        put(
            doc,
            "redaction",
            "extra_rules",
            toml_edit::Value::Array(next),
        );
    }
    keep_comments(doc, &comments);
}

fn write_rule(
    table: &mut dyn toml_edit::TableLike,
    old: Option<&config::ExtraRule>,
    rule: &config::ExtraRule,
) -> String {
    let mut comments = String::new();
    let values = [
        (
            "id",
            Some(rule.id.as_str().into()),
            old.is_none_or(|r| r.id != rule.id),
        ),
        (
            "regex",
            Some(rule.regex.as_str().into()),
            old.is_none_or(|r| r.regex != rule.regex),
        ),
        (
            "keywords",
            (!rule.keywords.is_empty()).then(|| {
                toml_edit::Value::Array(rule.keywords.iter().map(String::as_str).collect())
            }),
            old.is_none_or(|r| r.keywords != rule.keywords),
        ),
        (
            "entropy",
            rule.entropy.map(toml_edit::Value::from),
            old.is_none_or(|r| r.entropy != rule.entropy),
        ),
        (
            "secret_group",
            rule.secret_group.map(|g| toml_edit::Value::from(g as i64)),
            old.is_none_or(|r| r.secret_group != rule.secret_group),
        ),
    ];
    for (key, value, changed) in values {
        if !changed {
            continue;
        }
        if let Some(mut value) = value {
            if let Some(old) = table.get(key).and_then(toml_edit::Item::as_value) {
                *value.decor_mut() = old.decor().clone();
            }
            table.insert(key, toml_edit::Item::Value(value));
        } else {
            comments.push_str(&key_comments(table, key));
            comments.push('\n');
            table.remove(key);
        }
    }
    comments
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
mod tests;
