//! Version-bound edits of native entries. Name-group overlays remain separate settings.

use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    BUDGET, MODEL_CHARS, Refusal, TIMEOUT_S, alone, bytes, calls_cap, config_lock,
    endpoint_supported, invalid, parsed, put_root, refused, show, utf8, version,
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

/// Decision 78: an entry's model list, read again when `refresh`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsRequest {
    version: String,
    selector: Selector,
    #[serde(default)]
    refresh: bool,
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
        /// codex only (owner decision 41). Absent from an older page: kept as saved.
        #[serde(default)]
        credits: Option<bool>,
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
        if name.trim().is_empty()
            || name.chars().count() > 64
            || name.chars().any(char::is_control)
            || name.starts_with(crate::config::JUDGE_PREFIX)
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
                    extra: more,
                    ..
                } = &mut result
                else {
                    return Err(refused(422, "provider_kind", "providers.kind"));
                };
                // No new subscription daily-call cap. Legacy caps survive an unrelated edit.
                if *daily_budget != *budget
                    && (*subscription || daily_budget.is_some_and(|v| !calls_cap(v, &BUDGET)))
                {
                    return Err(refused(422, "range", "providers.daily_budget"));
                }
                n.clone_from(name);
                *on = *enabled;
                url.clone_from(base_url);
                // An entry's `extra` speaks its API: one switched to the other API sends none of
                // it (Codex on #409: OpenAI-only fields are refused by the Messages API).
                if *speaks != *api {
                    more.clear();
                }
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
                credits,
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
                if *credits == Some(true) && cli != "codex" {
                    return Err(refused(422, "range", "providers.credits"));
                }
                let mut result = old.cloned().unwrap_or_else(|| Provider::Cli {
                    credits: false,
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
                    credits: spends,
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
                // Only codex draws on credits: an entry that is no longer codex keeps none.
                *spends = cli == "codex" && credits.unwrap_or(*spends);
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
                table.set_position(Some(isize::MAX));
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
        "credits",
        // Only ever cleared here, when the API changes (`Draft`): the page shows no extra.
        "extra",
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
    let mut new = entry.checked(None)?;
    // A catalog preset needs the built-in's protocol fields (for example Go's session header).
    // Match the full connection before inheriting only these non-secret transport defaults;
    // keys still require managed registration, and every posted owner setting stays as given.
    let defaults = config::from_text(path, "").map_err(|_| invalid())?;
    for builtin in defaults.providers {
        if let (
            Provider::Openai {
                name,
                base_url,
                api,
                model,
                headers,
                extra,
                retry_429,
                ..
            },
            Provider::Openai {
                name: preset_name,
                base_url: preset_url,
                api: preset_api,
                model: preset_model,
                headers: preset_headers,
                extra: preset_extra,
                retry_429: preset_retry,
                ..
            },
        ) = (&mut new, builtin)
            && *name == preset_name
            && *base_url == preset_url
            && *api == preset_api
            && *model == preset_model
        {
            *headers = preset_headers;
            *extra = preset_extra;
            *retry_429 = preset_retry;
            break;
        }
    }
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
    let name = entry.name().to_owned();
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
    super::key_saved(home, &[&name]);
    answer["key_saved"] = json!({"durable": new_key.durable});
    new_key.retain();
    Ok(answer)
}

/// The shared config persistence contract: full validation, stage, stale check, then publish.
pub(super) fn commit(home: &Path, expected: &str, doc: DocumentMut) -> Result<Value, Refusal> {
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
    // A `judge:` name, written by hand, would test under the judge's ledger rows and state.
    if provider.name().starts_with(config::JUDGE_PREFIX) {
        return Err(refused(422, "bad_entry", "providers.name"));
    }
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
    // `p` is the probe, which writes no prompt cache: its input is priced once (its reservation,
    // `Provider::input_weight`, allows for a write it does not make).
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
                let holds = crate::provider::rest_holds(&state, &p);
                if holds && state.down_until == crate::providers_db::OWNER_HOLD {
                    code = Some("owner_hold");
                } else if holds && state.down_until > crate::db::now_ms() {
                    code = Some("cooldown");
                } else if let Some(r) = crate::budget::admit_with_history(
                    db,
                    (&p, &selected.provider),
                    "probe",
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
        "monthly_cap_usd": selected.paid_cap.is_finite().then_some(selected.paid_cap),
        "possible_charge": possible_charge,
        "ready": code.is_none(), "code": code}),
    )
}

/// Decision 78: the models the selected entry's service lists, ids only, from its own URL with its
/// key (`provider::list_models`); `from` says whether they were read now or kept from the last ten
/// minutes. The URL and headers pass the connection test's rules (`probe_provider`): no key goes to
/// a destination the page does not show. Nothing of the entry's key or URL is in the answer.
pub(super) fn models(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    let posted: ModelsRequest =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let selected = read_selection(home, &posted.version, posted.selector)?;
    let p = crate::provider::probe_provider(&selected.provider)
        .map_err(|code| refused(422, code, "providers.models"))?;
    match crate::provider::list_models(&p, posted.refresh) {
        Ok((models, kept)) => Ok(json!({
            "models": models,
            "from": if kept { "cache" } else { "service" },
        })),
        Err(e) => Err(refused(
            if e == crate::provider::ListError::Unsupported {
                422
            } else {
                502
            },
            e.code(),
            "providers.models",
        )),
    }
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
mod tests;
