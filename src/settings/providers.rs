//! Version-bound edits of native entries. Name-group overlays remain separate settings.

use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    BUDGET, MODEL_CHARS, Refusal, TIMEOUT_S, alone, bytes, config_lock, endpoint_supported,
    invalid, parsed, put_root, refused, show, utf8, version,
};
use crate::config::{self, Provider};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike};

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Source {
    File,
    Builtin,
    Gemini,
}

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selector {
    pub source: Source,
    pub index: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    version: String,
    action: Action,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyEdit {
    version: String,
    selector: Selector,
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestSelection {
    version: String,
    selector: Selector,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestRun {
    version: String,
    selector: Selector,
    confirmed: bool,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Action {
    Create { entry: Draft },
    Edit { selector: Selector, entry: Draft },
    Remove { selector: Selector },
    Move { selector: Selector, to: usize },
    Enabled { selector: Selector, enabled: bool },
}

/// The visible native fields. Headers, extras, credential paths and process arguments are not
/// writable through this operation; an edit keeps the old entry's other fields intact.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Draft {
    Openai {
        name: String,
        enabled: bool,
        base_url: String,
        /// Absent from an older page: the OpenAI-compatible API.
        #[serde(default)]
        api: config::Api,
        model: String,
        daily_budget: Option<u32>,
        timeout_s: u64,
        subscription: bool,
        limits: config::Limits,
    },
    Cli {
        name: String,
        enabled: bool,
        cli: String,
        model: Option<String>,
        timeout_s: u64,
        limits: config::Limits,
    },
}

impl Draft {
    fn check_common_fields(&self) -> Result<(), Refusal> {
        let (name, timeout, limits) = match self {
            Self::Openai {
                name,
                timeout_s,
                limits,
                ..
            }
            | Self::Cli {
                name,
                timeout_s,
                limits,
                ..
            } => (name, *timeout_s, limits),
        };
        if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control)
        {
            return Err(refused(422, "bad_entry", "providers.name"));
        }
        if !TIMEOUT_S.contains(&timeout)
            || limits.max_output_tokens == 0
            || limits.max_request_tokens == Some(0)
            || limits.daily_tokens == Some(0)
        {
            return Err(refused(422, "range", "providers.limits"));
        }
        Ok(())
    }

    fn checked(&self, old: Option<&Provider>) -> Result<Provider, Refusal> {
        self.check_common_fields()?;
        let model_ok = |m: &str| {
            !m.is_empty() && m.chars().count() <= MODEL_CHARS && !m.chars().any(char::is_control)
        };
        match self {
            Self::Openai {
                name,
                enabled,
                base_url,
                api,
                model,
                daily_budget,
                timeout_s,
                subscription,
                limits,
            } => {
                if !endpoint_supported(base_url) {
                    return Err(refused(422, "bad_endpoint", "providers.base_url"));
                }
                if !model_ok(model) {
                    return Err(refused(422, "range", "providers.model"));
                }
                let mut result = old.cloned().unwrap_or_else(|| Provider::Openai {
                    name: name.clone(),
                    enabled: *enabled,
                    base_url: base_url.clone(),
                    api: *api,
                    key_file: None,
                    model: model.clone(),
                    daily_budget: None,
                    timeout_s: *timeout_s,
                    retry_429: false,
                    extra: Default::default(),
                    headers: Default::default(),
                    limits: limits.clone(),
                    subscription: *subscription,
                });
                let Provider::Openai {
                    name: n,
                    enabled: on,
                    base_url: url,
                    api: speaks,
                    model: m,
                    daily_budget: budget,
                    timeout_s: timeout,
                    subscription: sub,
                    limits: cap,
                    ..
                } = &mut result
                else {
                    return Err(refused(422, "provider_kind", "providers.kind"));
                };
                // No new subscription daily-call cap. Legacy caps survive an unrelated edit.
                if *daily_budget != *budget
                    && (*subscription || daily_budget.is_some_and(|v| !BUDGET.contains(&v)))
                {
                    return Err(refused(422, "range", "providers.daily_budget"));
                }
                n.clone_from(name);
                *on = *enabled;
                url.clone_from(base_url);
                *speaks = *api;
                m.clone_from(model);
                *budget = *daily_budget;
                *timeout = *timeout_s;
                *sub = *subscription;
                *cap = limits.clone();
                Ok(result)
            }
            Self::Cli {
                name,
                enabled,
                cli,
                model,
                timeout_s,
                limits,
            } => {
                if !matches!(cli.as_str(), "claude" | "codex") {
                    return Err(refused(422, "unsupported_provider", "providers.cli"));
                }
                if model
                    .as_deref()
                    .is_some_and(|m| !model_ok(m) || m.starts_with('-'))
                    || limits.is_paid()
                {
                    return Err(refused(422, "range", "providers.model"));
                }
                let mut result = old.cloned().unwrap_or_else(|| Provider::Cli {
                    name: name.clone(),
                    enabled: *enabled,
                    cli: cli.clone(),
                    model: model.clone(),
                    daily_budget: config::no_daily_cap(),
                    timeout_s: *timeout_s,
                    limits: limits.clone(),
                });
                let Provider::Cli {
                    name: n,
                    enabled: on,
                    cli: adapter,
                    model: m,
                    timeout_s: timeout,
                    limits: cap,
                    ..
                } = &mut result
                else {
                    return Err(refused(422, "provider_kind", "providers.kind"));
                };
                n.clone_from(name);
                *on = *enabled;
                adapter.clone_from(cli);
                m.clone_from(model);
                *timeout = *timeout_s;
                *cap = limits.clone();
                Ok(result)
            }
        }
    }
}

fn base(path: &Path, doc: &DocumentMut) -> Result<config::Config, Refusal> {
    let mut raw = doc.clone();
    raw.remove("chain");
    raw.remove("gemini");
    config::from_text(path, &raw.to_string()).map_err(|_| invalid())
}

fn table(p: &Provider) -> Result<Table, Refusal> {
    toml::to_string(p)
        .ok()
        .and_then(|t| t.parse::<DocumentMut>().ok())
        // Newly materialized entries carry inline children, independent of the small source
        // document's table positions; those positions would interleave different entries.
        .map(|doc| doc.as_table().clone().into_inline_table().into_table())
        .ok_or_else(invalid)
}

fn materialize(doc: &mut DocumentMut, cfg: &config::Config) -> Result<(), Refusal> {
    if !doc.contains_key("providers") {
        let mut tables = ArrayOfTables::new();
        for p in &cfg.providers {
            tables.push(table(p)?);
        }
        doc.insert("providers", Item::ArrayOfTables(tables));
    }
    Ok(())
}

fn insert(doc: &mut DocumentMut, at: usize, new: Table) -> Result<(), Refusal> {
    match doc.get_mut("providers") {
        Some(Item::ArrayOfTables(tables)) => {
            if at > tables.len() {
                return Err(refused(422, "range", "providers.order"));
            }
            let mut ordered: Vec<Table> = tables.iter().cloned().collect();
            ordered.insert(at, new);
            // Parsed tables retain their old document positions. Keep the edited provider
            // block in logical array order, with each entry's children immediately after it.
            fn in_array_order(table: &mut Table) {
                table.set_position(isize::MAX);
                for (_, item) in table.iter_mut() {
                    match item {
                        Item::Table(child) => in_array_order(child),
                        Item::ArrayOfTables(children) => {
                            for child in children.iter_mut() {
                                in_array_order(child);
                            }
                        }
                        _ => {}
                    }
                }
            }
            for table in &mut ordered {
                in_array_order(table);
            }
            *tables = ordered.into_iter().collect();
        }
        Some(Item::Value(toml_edit::Value::Array(entries))) if at <= entries.len() => {
            entries.insert(at, new.into_inline_table());
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

fn remove(doc: &mut DocumentMut, at: usize) -> Result<Table, Refusal> {
    match doc.get_mut("providers") {
        Some(Item::ArrayOfTables(tables)) if at < tables.len() => Ok(tables.remove(at)),
        Some(Item::Value(toml_edit::Value::Array(entries))) if at < entries.len() => {
            match entries.remove(at) {
                toml_edit::Value::InlineTable(entry) => Ok(entry.into_table()),
                _ => Err(invalid()),
            }
        }
        _ => Err(refused(404, "no_entry", "providers")),
    }
}

fn table_mut(doc: &mut DocumentMut, at: usize) -> Option<&mut dyn TableLike> {
    match doc.get_mut("providers")? {
        Item::ArrayOfTables(tables) => tables.get_mut(at).map(|t| t as &mut dyn TableLike),
        Item::Value(toml_edit::Value::Array(entries)) => entries
            .get_mut(at)?
            .as_inline_table_mut()
            .map(|t| t as &mut dyn TableLike),
        _ => None,
    }
}

/// Selects against the unoverlaid native list, and materializes only where the edit needs it.
fn selected(
    path: &Path,
    doc: &mut DocumentMut,
    selector: Selector,
) -> Result<(usize, Provider), Refusal> {
    let raw = base(path, doc)?;
    match selector.source {
        Source::File if doc.contains_key("providers") => raw
            .providers
            .get(selector.index)
            .cloned()
            .map(|p| (selector.index, p))
            .ok_or_else(|| refused(404, "no_entry", "providers")),
        Source::Builtin if !doc.contains_key("providers") => {
            let p = raw
                .providers
                .get(selector.index)
                .cloned()
                .ok_or_else(|| refused(404, "no_entry", "providers"))?;
            materialize(doc, &raw)?;
            Ok((selector.index, p))
        }
        Source::Gemini
            if selector.index == 0 && !raw.providers.iter().any(|p| p.name() == "gemini") =>
        {
            let with = alone(path, doc).ok_or_else(invalid)?;
            let (at, p) = with
                .providers
                .iter()
                .enumerate()
                .find(|(_, p)| p.name() == "gemini")
                .ok_or_else(|| refused(404, "no_entry", "providers"))?;
            materialize(doc, &raw)?;
            insert(doc, at, table(p)?)?;
            Ok((at, p.clone()))
        }
        _ => Err(refused(404, "no_entry", "providers")),
    }
}

/// Only changed visible values are copied from the typed entry. Existing decorations, unknown
/// fields and secret extras stay on their original TOML nodes.
fn edit(table: &mut dyn TableLike, old: &Provider, new: &Provider) -> Result<(), Refusal> {
    let old_table = self::table(old)?;
    let new_table = self::table(new)?;
    let fields = [
        "name",
        "enabled",
        "base_url",
        "api",
        "model",
        "cli",
        "daily_budget",
        "timeout_s",
        "subscription",
    ];
    for key in fields {
        let a = old_table.get(key);
        let b = new_table.get(key);
        if a.map(ToString::to_string) == b.map(ToString::to_string) {
            continue;
        }
        if let Some(item) = b {
            let mut item = item.clone();
            if let (Some(old), Some(value)) =
                (table.get(key).and_then(Item::as_value), item.as_value_mut())
            {
                *value.decor_mut() = old.decor().clone();
            }
            table.insert(key, item);
        } else {
            table.remove(key);
        }
    }
    if toml::to_string(old.limits()).ok() != toml::to_string(new.limits()).ok() {
        let fresh = new_table.get("limits").ok_or_else(invalid)?;
        if let Some(current) = table.get_mut("limits").and_then(Item::as_table_like_mut) {
            let prior = old_table
                .get("limits")
                .and_then(Item::as_table_like)
                .ok_or_else(invalid)?;
            let next = fresh.as_table_like().ok_or_else(invalid)?;
            for (key, item) in next.iter() {
                if prior.get(key).map(ToString::to_string) == Some(item.to_string()) {
                    continue;
                }
                let mut item = item.clone();
                if let (Some(old), Some(value)) = (
                    current.get(key).and_then(Item::as_value),
                    item.as_value_mut(),
                ) {
                    *value.decor_mut() = old.decor().clone();
                }
                current.insert(key, item);
            }
            let removed: Vec<_> = prior
                .iter()
                .filter(|(key, _)| !next.contains_key(key))
                .map(|(key, _)| key.to_owned())
                .collect();
            for key in removed {
                current.remove(&key);
            }
        } else {
            table.insert("limits", fresh.clone());
        }
    }
    Ok(())
}

fn create(path: &Path, doc: &mut DocumentMut, entry: Draft) -> Result<(), Refusal> {
    let new = entry.checked(None)?;
    let raw = base(path, doc)?;
    let virtual_entry =
        if new.name() == "gemini" && !raw.providers.iter().any(|p| p.name() == "gemini") {
            let with = alone(path, doc).ok_or_else(invalid)?;
            with.providers
                .iter()
                .enumerate()
                .find(|(_, p)| p.name() == "gemini")
                .map(|(at, p)| (at, p.clone()))
        } else {
            None
        };
    materialize(doc, &raw)?;
    let added = usize::from(virtual_entry.is_some());
    if let Some((at, p)) = virtual_entry {
        insert(doc, at, table(&p)?)?;
    }
    insert(doc, raw.providers.len() + added, table(&new)?)
}

pub(super) fn save(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Edit = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
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
    parsed(&path, text).ok_or_else(invalid)?;
    let mut doc: DocumentMut = text.parse().map_err(|_| invalid())?;
    let remove_gemini = matches!(
        &posted.action,
        Action::Remove {
            selector: Selector {
                source: Source::Gemini,
                index: 0
            }
        }
    );
    if remove_gemini {
        let raw = base(&path, &doc)?;
        if raw.gemini.is_some()
            || raw.providers.iter().any(|p| p.name() == "gemini")
            || !doc.contains_key("gemini")
        {
            return Err(refused(404, "no_entry", "providers"));
        }
        put_root(&mut doc, "gemini", None);
    } else {
        match posted.action {
            Action::Create { entry } => create(&path, &mut doc, entry)?,
            Action::Edit { selector, entry } => {
                let (at, old) = selected(&path, &mut doc, selector)?;
                let new = entry.checked(Some(&old))?;
                edit(table_mut(&mut doc, at).ok_or_else(invalid)?, &old, &new)?;
                if old.name() == "gemini"
                    && new.name() != "gemini"
                    && !base(&path, &doc)?
                        .providers
                        .iter()
                        .any(|p| p.name() == "gemini")
                {
                    put_root(&mut doc, "gemini", None);
                }
            }
            Action::Remove { selector } => {
                let (at, old) = selected(&path, &mut doc, selector)?;
                remove(&mut doc, at)?;
                if old.name() == "gemini"
                    && !base(&path, &doc)?
                        .providers
                        .iter()
                        .any(|p| p.name() == "gemini")
                {
                    put_root(&mut doc, "gemini", None);
                }
            }
            Action::Move { selector, to } => {
                let (at, _) = selected(&path, &mut doc, selector)?;
                let n = base(&path, &doc)?.providers.len();
                if to >= n {
                    return Err(refused(422, "range", "providers.order"));
                }
                if at != to {
                    let item = remove(&mut doc, at)?;
                    insert(&mut doc, to, item)?;
                }
            }
            Action::Enabled { selector, enabled } => {
                let (at, old) = selected(&path, &mut doc, selector)?;
                let mut new = old.clone();
                let (Provider::Openai { enabled: on, .. } | Provider::Cli { enabled: on, .. }) =
                    &mut new;
                *on = enabled;
                if enabled {
                    crate::provider::probe_provider(&new)
                        .map_err(|code| refused(422, code, "providers.enabled"))?;
                }
                edit(table_mut(&mut doc, at).ok_or_else(invalid)?, &old, &new)?;
            }
        }
    }
    commit(home, &posted.version, doc)
}

pub(super) fn save_key_at(
    home: &Path,
    saving: &Mutex<()>,
    body: &[u8],
    owner: &Path,
    data: Option<&Path>,
) -> Result<Value, Refusal> {
    let posted: KeyEdit =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !crate::keyfile::valid(&posted.key) {
        return Err(refused(422, "bad_key", "providers.key"));
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
    parsed(&path, text).ok_or_else(invalid)?;
    let mut doc: DocumentMut = text.parse().map_err(|_| invalid())?;
    let (at, entry) = selected(&path, &mut doc, posted.selector)?;
    if !matches!(entry, Provider::Openai { .. }) {
        return Err(refused(422, "no_key_file", "providers.key"));
    }
    let new_key = crate::keyfile::managed(&posted.key, home, owner, data)
        .map_err(|r| refused(r.status(), r.code(), "providers.key"))?;
    let filename = new_key
        .path()
        .to_str()
        .ok_or_else(|| refused(422, "not_utf8", "providers.key"))?;
    let table = table_mut(&mut doc, at).ok_or_else(invalid)?;
    let mut reference = toml_edit::Value::from(filename);
    if let Some(old) = table.get("key_file").and_then(Item::as_value) {
        *reference.decor_mut() = old.decor().clone();
    }
    table.insert("key_file", Item::Value(reference));
    let mut answer = commit(home, &posted.version, doc)?;
    answer["key_saved"] = json!({"durable": new_key.durable});
    new_key.retain();
    Ok(answer)
}

/// The shared config persistence contract: full validation, stage, stale check, then publish.
fn commit(home: &Path, expected: &str, doc: DocumentMut) -> Result<Value, Refusal> {
    let path = home.join("config.toml");
    let candidate = doc.to_string();
    let current = bytes(home).map_err(|_| invalid())?;
    if version(current.as_deref()) != expected {
        return Err(refused(409, "stale", ""));
    }
    if utf8(current.as_deref()) == Some(candidate.as_str()) {
        return Ok(show(home));
    }
    parsed(&path, &candidate).ok_or_else(invalid)?;
    let staged =
        crate::setup::stage(&path, &candidate).map_err(|_| refused(500, "write_failed", ""))?;
    if version(bytes(home).map_err(|_| invalid())?.as_deref()) != expected {
        return Err(refused(409, "stale", ""));
    }
    staged
        .commit()
        .map_err(|_| refused(500, "write_failed", ""))?;
    Ok(show(home))
}

struct Selected {
    provider: Provider,
    paid_cap: f64,
    rules: crate::redact::Rules,
}

fn unavailable() -> Refusal {
    refused(503, "unavailable", "providers.test")
}

/// Only fixed, user-actionable causes cross the boundary. SQLite/IO messages can contain
/// private paths or file contents, and unknown errors must not be reflected into the page.
fn probe_failure(error: anyhow::Error) -> Refusal {
    for cause in error.chain() {
        if let Some(rusqlite::Error::SqliteFailure(sqlite, _)) = cause.downcast_ref() {
            let code = match sqlite.code {
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                    "provider_busy"
                }
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                    "ledger_invalid"
                }
                _ => continue,
            };
            return refused(503, code, "providers.test");
        }
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::WouldBlock)
        {
            return refused(503, "provider_busy", "providers.test");
        }
    }
    unavailable()
}

/// Select from one readonly snapshot. Materialization here changes only a private document in
/// memory; preview never persists it or creates a coordination/ledger file.
fn read_selection(home: &Path, expected: &str, selector: Selector) -> Result<Selected, Refusal> {
    let path = home.join("config.toml");
    let data = bytes(home).map_err(|_| invalid())?;
    if version(data.as_deref()) != expected {
        return Err(refused(409, "stale", ""));
    }
    let text = utf8(data.as_deref()).ok_or_else(invalid)?;
    let (cfg, ..) = parsed(&path, text).ok_or_else(invalid)?;
    let mut doc: DocumentMut = text.parse().map_err(|_| invalid())?;
    let (at, own) = selected(&path, &mut doc, selector)?;
    let native = base(&path, &doc)?;
    let occurrence = native.providers[..at]
        .iter()
        .filter(|p| p.name() == own.name())
        .count();
    let mut provider = cfg
        .providers
        .iter()
        .filter(|p| p.name() == own.name())
        .nth(occurrence)
        .cloned()
        .ok_or_else(|| refused(404, "no_entry", "providers"))?;
    if cfg.chain.turns_off(provider.name()) {
        let (Provider::Openai { enabled, .. } | Provider::Cli { enabled, .. }) = &mut provider;
        *enabled = false;
    }
    let capture = config::parse_capture(Some(text)).map_err(|_| invalid())?;
    let rules = crate::redact::Rules::new(&capture.redaction).map_err(|_| invalid())?;
    Ok(Selected {
        provider,
        paid_cap: cfg.paid_usd_per_month,
        rules,
    })
}

fn egress_ok(selected: &Selected) -> bool {
    crate::redact::outbound_with(crate::provider::PROBE_PROMPT, &selected.rules)
        == crate::provider::PROBE_PROMPT
}

pub(super) fn preview(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    let posted: TestSelection =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let selected = read_selection(home, &posted.version, posted.selector)?;
    let p = crate::provider::probe_provider(&selected.provider)
        .map_err(|code| refused(422, code, "providers.test"))?;
    let ledger = crate::providers_db::read_only(home).map_err(probe_failure)?;
    let factor = match &ledger {
        Some(db) => crate::budget::factor(db, p.name()).map_err(probe_failure)?,
        None => 1.0,
    };
    let input = f64::from(crate::provider::probe_estimate(&p)) * factor;
    let output = crate::budget::output_bound(&p);
    let usd = p
        .limits()
        .is_paid()
        .then(|| p.limits().usd(input, f64::from(output)));
    let mut code = if !p.enabled() {
        Some("disabled")
    } else if !egress_ok(&selected) {
        Some("egress")
    } else {
        crate::provider::probe_unavailable(&p)
    };
    if code.is_none() {
        match &ledger {
            Some(db) => {
                let state = crate::providers_db::state(db, p.name()).map_err(probe_failure)?;
                if state.down_until == crate::providers_db::OWNER_HOLD {
                    code = Some("owner_hold");
                } else if state.down_until > crate::db::now_ms() {
                    code = Some("cooldown");
                } else if let Some(r) = crate::budget::admit_with_history(
                    db,
                    (&p, &selected.provider),
                    input,
                    selected.paid_cap,
                    &[],
                )
                .map_err(probe_failure)?
                {
                    code = Some(if r.outcome == "too_big" {
                        "too_big"
                    } else {
                        "budget"
                    });
                }
            }
            None => {
                // No ledger means known empty usage, default calibration and no cooldown/rate
                // snapshot. Execution still reserves atomically before any send.
                let limits = p.limits();
                if limits.max_request_tokens.is_some_and(|max| {
                    input > f64::from(max) * crate::budget::CEILING_SHARE
                        || input + f64::from(output) > f64::from(max)
                }) {
                    code = Some("too_big");
                } else if p.daily_budget() == 0
                    || limits
                        .daily_tokens
                        .is_some_and(|max| input + f64::from(output) > max as f64)
                    || usd.is_some_and(|amount| !amount.is_finite() || amount > selected.paid_cap)
                {
                    code = Some("budget");
                }
            }
        }
    }
    let (destination, model, max_output, possible_charge) = match &p {
        Provider::Openai {
            base_url,
            api,
            model,
            limits,
            ..
        } => (
            crate::provider::endpoint(base_url, *api),
            Some(model.as_str()),
            Some(limits.max_output_tokens),
            limits.is_paid() || !crate::provider::is_loopback(base_url),
        ),
        Provider::Cli { cli, model, .. } => (cli.clone(), model.as_deref(), None, false),
    };
    if version(bytes(home).map_err(|_| invalid())?.as_deref()) != posted.version {
        return Err(refused(409, "stale", ""));
    }
    Ok(
        json!({"version": posted.version, "selector": posted.selector, "destination": destination,
        "model": model, "fixture": crate::provider::PROBE_PROMPT, "max_output_tokens": max_output,
        "estimated_input_tokens": input, "estimated_usd": usd.filter(|v| v.is_finite()),
        "monthly_cap_usd": selected.paid_cap, "possible_charge": possible_charge,
        "ready": code.is_none(), "code": code}),
    )
}

pub(super) fn test(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    let posted: TestRun =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !posted.confirmed {
        return Err(refused(422, "confirmation", "providers.test"));
    }
    let selected = read_selection(home, &posted.version, posted.selector)?;
    crate::provider::probe_provider(&selected.provider)
        .map_err(|code| refused(422, code, "providers.test"))?;
    if !egress_ok(&selected) {
        return Err(refused(422, "egress", "providers.test"));
    }
    if let Some(code) = crate::provider::probe_unavailable(&selected.provider) {
        return Err(refused(422, code, "providers.test"));
    }
    let db = crate::providers_db::open(home).map_err(probe_failure)?;
    let refused_gate = std::cell::RefCell::new(None);
    let gate = || {
        let checked = (|| {
            let admission = crate::dispatch::Admission::shared(home).map_err(probe_failure)?;
            let fresh = read_selection(home, &posted.version, posted.selector)?;
            if !egress_ok(&fresh) {
                return Err(refused(422, "egress", "providers.test"));
            }
            Ok(admission)
        })();
        match checked {
            Ok(admission) => Ok(Some(admission)),
            Err(error) => {
                *refused_gate.borrow_mut() = Some(error);
                Err(anyhow::anyhow!("connection test gate refused"))
            }
        }
    };
    match crate::provider::probe(&db, &selected.provider, selected.paid_cap, &gate) {
        Ok(answer) => Ok(answer),
        Err(error) => Err(refused_gate
            .into_inner()
            .unwrap_or_else(|| probe_failure(error))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(text: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        home
    }

    fn send(home: &Path, action: Value) -> Result<Value, Refusal> {
        let body = serde_json::to_vec(&json!({"version": show(home)["version"], "action": action}))
            .unwrap();
        save(home, &Mutex::new(()), &body)
    }

    fn http(name: &str, endpoint: &str) -> Value {
        json!({"kind": "openai", "name": name, "enabled": true, "base_url": endpoint,
            "model": "synthetic", "timeout_s": 30, "daily_budget": 20, "subscription": false,
            "limits": {"max_request_tokens": null, "daily_tokens": null,
                "usd_per_mtok_in": 0.0, "usd_per_mtok_out": 0.0, "max_output_tokens": 4000}})
    }

    fn cli(name: &str, model: Option<&str>, enabled: bool) -> Value {
        json!({"kind": "cli", "name": name, "enabled": enabled, "cli": "claude", "model": model,
            "timeout_s": 90, "limits": {"max_request_tokens": null, "daily_tokens": null,
                "usd_per_mtok_in": 0.0, "usd_per_mtok_out": 0.0, "max_output_tokens": 4000}})
    }

    #[test]
    fn an_unsupported_legacy_entry_can_be_disabled_without_rewriting_its_private_fields() {
        let text = "[[providers]]\nkind = \"cli\"\nname = \"legacy\"\ncli = \"gemini\"\n\
            daily_budget = 7 # old cap\nfuture = \"retained\"\n";
        let home = home(text);
        let shown = show(home.path());
        let saved = send(home.path(), json!({"op": "enabled", "selector": shown["providers"][0]["selector"], "enabled": false})).unwrap();
        assert_eq!(saved["providers"][0]["effective"]["on"], false);
        assert!(
            config::load_chain(home.path())
                .unwrap()
                .providers
                .is_empty()
        );
        let disabled = bytes(home.path()).unwrap();
        let refused = send(home.path(), json!({"op": "enabled", "selector": saved["providers"][0]["selector"], "enabled": true})).unwrap_err();
        assert_eq!((refused.status, refused.code), (422, "unsupported_cli"));
        assert_eq!(bytes(home.path()).unwrap(), disabled);
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(after.contains("daily_budget = 7 # old cap"));
        assert!(after.contains("future = \"retained\""));
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn invalid_native_limits_and_cli_arguments_leave_the_usable_file_unchanged() {
        let home = home("providers = [] # still usable\n");
        let before = bytes(home.path()).unwrap();
        for (field, value) in [
            ("name", json!(" ")),
            ("model", json!("")),
            ("timeout_s", json!(0)),
            ("daily_budget", json!(0)),
        ] {
            let mut entry = http("local", "https://example.invalid/v1");
            entry[field] = value;
            assert!(
                send(home.path(), json!({"op": "create", "entry": entry})).is_err(),
                "{field}"
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for field in ["max_request_tokens", "daily_tokens", "max_output_tokens"] {
            let mut entry = http("local", "https://example.invalid/v1");
            entry["limits"][field] = json!(0);
            assert_eq!(
                send(home.path(), json!({"op": "create", "entry": entry}))
                    .unwrap_err()
                    .code,
                "range",
                "{field}"
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for model in ["--config=untrusted", "unsafe\nargument"] {
            assert!(
                send(
                    home.path(),
                    json!({"op": "create", "entry": cli("a", Some(model), true)})
                )
                .is_err()
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn a_provider_save_rejects_unsafe_destinations_without_traffic_or_config_changes() {
        let home = home("providers = [] # preserved\n");
        let before = bytes(home.path()).unwrap();
        for endpoint in [
            "http://example.invalid/v1",
            "http://localhost:8080/v1",
            "file:///tmp/key",
            "ftp://example.invalid/v1",
            "https://user:password@example.invalid/v1",
            "https://example.invalid/v1?token=private",
            "https://example.invalid/v1#private",
            "https://example.invalid:99999/v1",
            "https://example.invalid:/v1",
            "https://example.invalid:abc/v1",
            "https://example.invalid:0/v1",
            "https://example.invalid/\nprivate",
            "https://example.invalid\\private",
        ] {
            let r = send(
                home.path(),
                json!({"op": "create", "entry": http("local", endpoint)}),
            )
            .expect_err("unsafe endpoint must be refused");
            assert_eq!((r.status, r.code), (422, "bad_endpoint"), "{endpoint}");
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for endpoint in [
            "https://example.invalid/v1",
            "http://127.0.0.1:12345/v1",
            "http://[::1]:12345/v1",
        ] {
            let saved = send(
                home.path(),
                json!({"op": "create", "entry": http("local", endpoint)}),
            )
            .unwrap();
            assert_eq!(saved["providers"][0]["saved"]["base_url"], endpoint);
            send(
                home.path(),
                json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
            )
            .unwrap();
        }
        assert!(!home.path().join("providers.db").exists());
        assert!(!home.path().join("worker.lock").exists());
    }

    #[test]
    fn creating_a_native_gemini_keeps_the_existing_virtual_entry_until_explicit_removal() {
        let home = home("gemini = \"after-subscriptions\" # placement note\nproviders = []\n");
        let shown = show(home.path());
        assert_eq!(shown["providers"].as_array().unwrap().len(), 1);
        assert_eq!(shown["providers"][0]["selector"]["source"], "gemini");
        let saved = send(
            home.path(),
            json!({"op": "create", "entry": http("gemini", "https://example.invalid/v1")}),
        )
        .unwrap();
        assert_eq!(
            saved["providers"].as_array().unwrap().len(),
            2,
            "creating an entry must not silently replace the pre-existing virtual provider"
        );
        assert_eq!(
            saved["providers"][0]["saved"]["model"],
            shown["providers"][0]["saved"]["model"]
        );
        let saved = send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][1]["selector"]}),
        )
        .unwrap();
        assert_eq!(saved["gemini"], "after-subscriptions");
        assert_eq!(saved["providers"].as_array().unwrap().len(), 1);
        let saved = send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        assert_eq!(saved["gemini"], Value::Null);
        assert!(saved["providers"].as_array().unwrap().is_empty());
        assert!(
            std::fs::read_to_string(home.path().join("config.toml"))
                .unwrap()
                .contains("# placement note")
        );
        assert!(!home.path().join("providers.db").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_key_registration_binds_a_physical_entry_without_revealing_or_replacing_old_keys() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let legacy = owner.path().join("LEGACY_KEY.md");
        std::fs::write(&legacy, "old key\nOldSyntheticKey000\nnotes unchanged\n").unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = format!(
            "# original\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\nkey_file = {legacy:?}\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\nfuture = 7\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let canary = "NewSyntheticKeyCanary999";
        let body = serde_json::to_vec(&json!({"version": shown["version"],
            "selector": shown["providers"][1]["selector"], "key": canary}))
        .unwrap();
        let answer = save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap();
        assert!(!answer.to_string().contains(canary));
        let shown = show(home.path());
        assert!(!shown.to_string().contains(canary));
        assert_eq!(
            shown["providers"][0]["saved"]["key_file"],
            legacy.to_str().unwrap()
        );
        let key_path =
            std::path::PathBuf::from(shown["providers"][1]["saved"]["key_file"].as_str().unwrap());
        assert!(key_path.starts_with(owner.path()) && !key_path.starts_with(home.path()));
        assert_eq!(key_path.extension().unwrap(), "md");
        assert_eq!(
            std::fs::read_to_string(&key_path).unwrap().lines().nth(1),
            Some(canary)
        );
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_to_string(&legacy).unwrap(),
            "old key\nOldSyntheticKey000\nnotes unchanged\n"
        );
        let config = bytes(home.path()).unwrap();
        assert!(
            !String::from_utf8(config.clone().unwrap())
                .unwrap()
                .contains(canary)
        );
        assert!(
            String::from_utf8(config.clone().unwrap())
                .unwrap()
                .contains("future = 7")
        );
        let r = save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap_err();
        assert_eq!((r.status, r.code), (409, "stale"));
        assert_eq!(bytes(home.path()).unwrap(), config);
        assert!(!home.path().join("providers.db").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_config_save_discards_only_the_new_managed_key() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let old = owner.path().join("EXISTING_KEY.md");
        std::fs::write(&old, "key\nOldSyntheticKey999\nretained\n").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = "[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n";
        let home = home(text);
        let config = home.path().join("config.toml");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o444)).unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(&json!({"version": shown["version"],
            "selector": shown["providers"][0]["selector"], "key": "UnusedSyntheticKey999"}))
        .unwrap();
        let refused =
            save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap_err();
        assert_eq!((refused.status, refused.code), (500, "write_failed"));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
        let managed = owner.path().join(".local/share/oboete/keys");
        assert!(
            managed.exists(),
            "the managed registration reached the subsequent config stage"
        );
        assert_eq!(std::fs::read_dir(managed).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_to_string(old).unwrap(),
            "key\nOldSyntheticKey999\nretained\n"
        );
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn provider_edits_keep_native_noops_inline_fields_and_legacy_subscription_caps() {
        let text = "# kept\nproviders = [\n\
            { kind = \"cli\", name = \"a\", cli = \"claude\", model = \"first\", timeout_s = 90, daily_budget = 7, future = \"kept\" },\n\
            { kind = \"cli\", name = \"b\", cli = \"claude\", model = \"second\", timeout_s = 90 }\n\
            ] # list note\n[chain]\norder = [\"b\", \"a\"] # overlay note\n";
        let home = home(text);
        let shown = show(home.path());
        send(
            home.path(),
            json!({"op": "edit", "selector": shown["providers"][0]["selector"],
            "entry": cli("a", Some("first"), true)}),
        )
        .unwrap();
        assert_eq!(bytes(home.path()).unwrap().unwrap(), text.as_bytes());
        let moved = send(
            home.path(),
            json!({"op": "move", "selector": shown["providers"][0]["selector"], "to": 1}),
        )
        .unwrap();
        assert_eq!(moved["providers"][0]["name"], "b");
        assert_eq!(moved["providers"][1]["name"], "a");
        assert_eq!(moved["providers"][1]["effective"]["order"], 1);
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": moved["providers"][1]["selector"],
            "entry": cli("日本語の名前", Some("changed"), true)}),
        )
        .unwrap();
        assert_eq!(saved["providers"][1]["name"], "日本語の名前");
        let cfg = config::load(home.path()).unwrap();
        assert_eq!(
            cfg.providers
                .iter()
                .find(|p| p.name() == "日本語の名前")
                .unwrap()
                .daily_budget(),
            7
        );
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        for kept in [
            "# kept",
            "future = \"kept\"",
            "# list note",
            "# overlay note",
            "order = [\"b\", \"a\"]",
        ] {
            assert!(after.contains(kept), "{kept}: {after}");
        }
        let mut wrong = cli("x", None, true);
        wrong["daily_budget"] = json!(5);
        assert!(send(home.path(), json!({"op": "create", "entry": wrong})).is_err());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            after
        );
    }

    #[test]
    fn editing_other_native_fields_keeps_legacy_http_caps_but_rejects_new_invalid_caps() {
        for budget in [0_u32, u32::MAX] {
            for subscription in [false, true] {
                let text = format!(
                    "# original\n[[providers]]\nkind = \"openai\"\nname = \"legacy\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"before\"\n\
                    daily_budget = {budget} # legacy cap\nsubscription = {subscription}\nfuture = \"kept\"\n"
                );
                let home = home(&text);
                let shown = show(home.path());
                let mut entry = http("legacy", "https://example.invalid/v1");
                entry["model"] = json!("after");
                entry["enabled"] = json!(false);
                entry["daily_budget"] = json!(budget);
                entry["subscription"] = json!(subscription);
                let selector = shown["providers"][0]["selector"].clone();
                let saved = send(
                    home.path(),
                    json!({"op": "edit", "selector": selector, "entry": entry}),
                )
                .unwrap();
                let native = &saved["providers"][0]["saved"];
                assert_eq!(native["daily_budget"], budget);
                assert_eq!(native["subscription"], subscription);
                assert_eq!(native["model"], "after");
                assert_eq!(native["enabled"], false);
                let after = bytes(home.path()).unwrap();
                let text = String::from_utf8(after.clone().unwrap()).unwrap();
                for kept in ["# original", "# legacy cap", "future = \"kept\""] {
                    assert!(text.contains(kept), "{text}");
                }
                entry["daily_budget"] = json!(if budget == 0 { u32::MAX } else { 0 });
                let r = send(
                    home.path(),
                    json!({"op": "edit", "selector": selector, "entry": entry}),
                )
                .unwrap_err();
                assert_eq!(
                    (r.status, r.code, r.field.as_str()),
                    (422, "range", "providers.daily_budget")
                );
                assert_eq!(bytes(home.path()).unwrap(), after);
                let r = send(home.path(), json!({"op": "create", "entry": entry})).unwrap_err();
                assert_eq!((r.status, r.code), (422, "range"));
                assert_eq!(bytes(home.path()).unwrap(), after);
            }
        }
    }

    #[test]
    fn moving_native_tables_keeps_their_nested_fields_and_unrelated_comments() {
        let text = "# root note\n[summary]\ncurate = false # summary note\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"first\"\n\
            [providers.extra]\nmarker = \"belongs-to-a\" # a note\n\
            [backup]\ndir = \"retained-backup\" # unrelated note\n\
            [[providers]]\nkind = \"openai\"\nname = \"b\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"second\"\n\
            [providers.headers]\nx-marker = \"belongs-to-b\" # b note\n\
            [[providers]]\nkind = \"cli\"\nname = \"c\"\ncli = \"claude\"\n\
            [providers.limits]\ndaily_tokens = 3000 # c note\n";
        let home = home(text);
        let shown = show(home.path());
        let saved = send(
            home.path(),
            json!({"op": "move", "selector": shown["providers"][2]["selector"], "to": 0}),
        )
        .unwrap();
        let cfg = config::load(home.path()).unwrap();
        let names: Vec<_> = cfg.providers.iter().map(Provider::name).collect();
        assert_eq!(names, ["c", "a", "b"]);
        assert_eq!(saved["providers"][0]["name"], "c");
        assert_eq!(cfg.providers[0].limits().daily_tokens, Some(3000));
        let Provider::Openai { extra, .. } = &cfg.providers[1] else {
            panic!("a's kind changed")
        };
        assert_eq!(extra["marker"], "belongs-to-a");
        let Provider::Openai { headers, .. } = &cfg.providers[2] else {
            panic!("b's kind changed")
        };
        assert_eq!(headers["x-marker"], "belongs-to-b");
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        for kept in [
            "# root note",
            "# summary note",
            "# unrelated note",
            "# a note",
            "# b note",
            "# c note",
            "dir = \"retained-backup\"",
        ] {
            assert!(after.contains(kept), "{kept}: {after}");
        }
        send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        assert_eq!(
            config::load(home.path())
                .unwrap()
                .providers
                .iter()
                .map(Provider::name)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn builtin_edits_materialize_the_native_base_without_losing_effective_overlays() {
        let home = home("[chain]\norder = [\"claude\"]\ntimeout_s = { claude = 150 }\n");
        let shown = show(home.path());
        let row = shown["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "claude")
            .unwrap();
        let old = config::load(home.path()).unwrap();
        let mut draft = cli("claude", Some("haiku"), false);
        draft["timeout_s"] = row["saved"]["timeout_s"].clone();
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": row["selector"], "entry": draft}),
        )
        .unwrap();
        let new = config::load(home.path()).unwrap();
        assert_eq!(new.providers.len(), old.providers.len());
        for (a, b) in old.providers.iter().zip(&new.providers) {
            assert_eq!(a.name(), b.name());
            if a.name() != "claude" {
                assert_eq!(toml::to_string(a).unwrap(), toml::to_string(b).unwrap());
            }
        }
        assert_eq!(saved["chain"][0]["name"], "claude");
        assert_eq!(saved["chain"][0]["effective_timeout_s"], 150);
        assert!(
            saved["providers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["selector"]["source"] == "file")
        );
        assert!(
            config::load_chain(home.path())
                .unwrap()
                .providers
                .iter()
                .all(|p| p.name() != "claude")
        );
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn a_probe_preview_is_bound_to_saved_selection_and_makes_no_files_or_requests() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = {endpoint:?}\nmodel = \"synthetic\"\n\
            limits = {{ usd_per_mtok_in = 1.0, usd_per_mtok_out = 1.0 }}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let mut body =
            json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]});
        let ready = preview(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(ready["version"], shown["version"]);
        assert_eq!(ready["selector"], body["selector"]);
        assert_eq!(ready["fixture"], crate::provider::PROBE_PROMPT);
        assert_eq!(ready["destination"], format!("{endpoint}/chat/completions"));
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["max_output_tokens"], 128);
        assert!(ready["estimated_usd"].as_f64().unwrap() > 0.0);
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            text
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
        body["version"] = json!("stale");
        assert_eq!(
            preview(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .status,
            409
        );
        body["version"] = shown["version"].clone();
        body["prompt"] = json!("private history must not be accepted");
        assert!(preview(home.path(), &serde_json::to_vec(&body).unwrap()).is_err());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_cli_probe_preview_is_truthfully_unavailable_without_ledger_or_process_work() {
        for cli in ["claude", "codex"] {
            let text =
                format!("[[providers]]\nkind = \"cli\"\nname = \"synthetic\"\ncli = {cli:?}\n");
            let home = home(&text);
            let shown = show(home.path());
            let body = serde_json::to_vec(&json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]})).unwrap();
            let answer = preview(home.path(), &body).unwrap();
            assert_eq!(answer["ready"], false);
            assert_eq!(answer["code"], "cli_probe_unbounded");
            assert_eq!(answer["max_output_tokens"], Value::Null);
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
            let mut confirmed: Value = serde_json::from_slice(&body).unwrap();
            confirmed["confirmed"] = json!(true);
            let r = test(home.path(), &serde_json::to_vec(&confirmed).unwrap()).unwrap_err();
            assert_eq!((r.status, r.code), (422, "cli_probe_unbounded"));
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
            assert_eq!(
                std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
                text
            );
        }
    }

    #[test]
    fn an_unreadable_ledger_reports_a_safe_cause_without_sending_or_rewriting_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let home = home(&format!(
            "[[providers]]\nkind = \"openai\"\nname = \"synthetic\"\nbase_url = {endpoint:?}\nmodel = \"synthetic\"\n"
        ));
        let shown = show(home.path());
        let body =
            json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]});
        let ledger = home.path().join("providers.db");
        let canary = b"not a database: PrivateSyntheticCanary999";
        std::fs::write(&ledger, canary).unwrap();
        let r = preview(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!((r.status, r.code), (503, "ledger_invalid"));
        let mut confirmed = body.clone();
        confirmed["confirmed"] = json!(true);
        let r = test(home.path(), &serde_json::to_vec(&confirmed).unwrap()).unwrap_err();
        assert_eq!((r.status, r.code), (503, "ledger_invalid"));
        assert!(!format!("{r:?}").contains("PrivateSyntheticCanary999"));
        assert!(!format!("{r:?}").contains(home.path().to_str().unwrap()));
        assert_eq!(std::fs::read(&ledger).unwrap(), canary);
        assert!(!home.path().join("dispatch.lock").exists());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn a_busy_ledger_preview_is_retryable_and_does_not_send_or_create_files() {
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"synthetic\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"synthetic\"\n",
        );
        let db = crate::providers_db::open(home.path()).unwrap();
        db.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")
            .unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]}),
        )
        .unwrap();
        let r = preview(home.path(), &body).unwrap_err();
        assert_eq!((r.status, r.code), (503, "provider_busy"));
        assert!(!home.path().join("dispatch.lock").exists());
        assert!(!home.path().join("raw.db").exists());
        db.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(preview(home.path(), &body).unwrap()["ready"], true);
    }

    #[test]
    fn a_probe_does_not_shrink_the_normal_fallback_for_legacy_unmetered_calls() {
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n\
            [providers.limits]\ndaily_tokens = 4100\nmax_output_tokens = 4000\nusd_per_mtok_in = 1.0\n",
        );
        let db = crate::providers_db::open(home.path()).unwrap();
        crate::providers_db::record(
            &db,
            &crate::providers_db::Call {
                provider: "local",
                role: "curator",
                span: "legacy",
                outcome: "error",
                ms: 1,
                detail: Some("transport"),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: crate::providers_db::Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]}),
        )
        .unwrap();
        let answer = preview(home.path(), &body).unwrap();
        assert_eq!(
            answer["ready"], false,
            "the current normal fallback already consumes the daily token cap: {answer}"
        );
        assert_eq!(answer["code"], "budget");
        assert_eq!(
            db.query_row("SELECT count(*) FROM provider_calls", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT detail FROM provider_calls", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "transport"
        );
        assert!(!home.path().join("dispatch.lock").exists());
    }

    #[test]
    fn the_explicit_test_requires_consent_and_current_selection_then_sends_one_fixed_request() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = {endpoint:?}\nmodel = \"synthetic\"\n\
            extra = {{ messages = [\"private history canary\"], models = [\"fallback\"], stream = true }}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let mut body = json!({"version": shown["version"], "selector": shown["providers"][0]["selector"], "confirmed": false});
        assert_eq!(
            test(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .code,
            "confirmation"
        );
        body["confirmed"] = json!(true);
        body["version"] = json!("stale");
        assert_eq!(
            test(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .code,
            "stale"
        );
        body["version"] = shown["version"].clone();
        assert!(!home.path().join("providers.db").exists());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        let server = std::thread::spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < until =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    result => panic!("private test connection: {result:?}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                assert!(request.len() <= 64 * 1024);
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&request[..at]).unwrap();
                    let size: usize = header
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .unwrap()
                        .1
                        .trim()
                        .parse()
                        .unwrap();
                    if request.len() >= at + 4 + size {
                        break;
                    }
                }
            }
            let data = json!({"choices": [{"message": {"content": "{\"ok\":true}"}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 4}})
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len()).unwrap();
            drop(stream);
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
            request
        });
        let result = test(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap();
        let request = server.join().unwrap();
        assert_eq!(result["status"], "ok");
        assert!(!result.to_string().contains("private history canary"));
        let at = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let payload: Value = serde_json::from_slice(&request[at..]).unwrap();
        assert_eq!(
            payload["messages"],
            json!([{"role": "user", "content": crate::provider::PROBE_PROMPT}])
        );
        assert_eq!(payload["stream"], false);
        assert!(payload.get("models").is_none());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            text
        );
        assert!(!home.path().join("raw.db").exists() && !home.path().join("knowledge.db").exists());
        let db = crate::providers_db::read_only(home.path())
            .unwrap()
            .unwrap();
        assert_eq!(crate::providers_db::last_calls(&db, 10).unwrap().len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn legacy_key_saves_wait_for_the_config_writer_and_cannot_retarget_a_stale_group() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let key = owner.path().join("EXISTING_KEY.md");
        std::fs::write(&key, "key\nUnchangedSynthetic999\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"a\"\nbase_url = \"https://example.invalid/v1\"\n\
            model = \"synthetic\"\nkey_file = {key:?}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "entry": "a", "key": "ReplacementSynthetic999"}),
        )
        .unwrap();
        let held = config_lock(home.path()).unwrap();
        let path = home.path().to_path_buf();
        let (started, waiting) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            started.send(()).unwrap();
            finished
                .send(crate::settings::save_key(&path, &Mutex::new(()), &body))
                .unwrap();
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            matches!(
                result.recv_timeout(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "legacy key save escaped the shared writer lock"
        );
        let changed = text.replace("name = \"a\"", "name = \"renamed\"");
        std::fs::write(home.path().join("config.toml"), &changed).unwrap();
        std::fs::File::unlock(&held).unwrap();
        let r = result
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        writer.join().unwrap();
        assert_eq!((r.status, r.code), (409, "stale"));
        assert_eq!(
            std::fs::read_to_string(key).unwrap(),
            "key\nUnchangedSynthetic999\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            changed
        );
    }

    /// The owner, 2026-10-09: an HTTP entry's API is chosen on the page and saved as `api`, left
    /// out for the OpenAI-compatible default; its test previews the Messages endpoint.
    #[test]
    fn an_entrys_api_is_saved_shown_and_previewed() {
        let home = home("providers = []\n");
        let mut entry = http("claude", "https://api.anthropic.com/v1");
        entry["api"] = json!("anthropic");
        let saved = send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        assert_eq!(saved["providers"][0]["saved"]["api"], "anthropic");
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(text.contains("api = \"anthropic\""), "{text}");
        let body = serde_json::to_vec(
            &json!({"version": saved["version"], "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        let shown = preview(home.path(), &body).unwrap();
        assert_eq!(
            shown["destination"],
            "https://api.anthropic.com/v1/messages"
        );
        let mut entry = http("claude", "https://api.anthropic.com/v1");
        entry["api"] = json!("openai");
        let selector = &saved["providers"][0]["selector"];
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": entry}),
        )
        .unwrap();
        assert_eq!(saved["providers"][0]["saved"]["api"], "openai");
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(!text.contains("api = "), "{text}");
        // An unknown API is refused with the file kept; an older page's entry, without one, is
        // OpenAI-compatible.
        let before = bytes(home.path()).unwrap();
        let mut entry = http("other", "https://example.invalid/v1");
        entry["api"] = json!("anthropic-ish");
        let refused = send(home.path(), json!({"op": "create", "entry": entry})).unwrap_err();
        assert_eq!((refused.status, refused.code), (400, "bad_request"));
        assert_eq!(bytes(home.path()).unwrap(), before);
        let entry = http("old", "https://example.invalid/v1");
        let saved = send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        assert_eq!(saved["providers"][1]["saved"]["api"], "openai");
    }
}
