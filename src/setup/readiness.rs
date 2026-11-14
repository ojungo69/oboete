//! Passive agent-file inventory. Never invokes setup, doctor, a store, or an agent process.

use super::{
    AGENTS, HookCommand, MCP_NAME, PI_MARKER, agy_dir, agy_spec, claude_dir, claude_groups,
    claude_mcp_paths, claude_settings_file, codex_groups, codex_home, codex_trust_keys, cursor_dir,
    cursor_hook_spec, diagnostic_canonicalize, diagnostic_metadata, grok_config_file, grok_groups,
    has_ours, is_our_handler, launcher_files, mcp_command_in_json, mcp_command_in_toml,
    mcp_disabled_in_json, mcp_disabled_in_toml, only_ours, opencode_dir, opencode_plugin, pi_dir,
    pi_extension,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomeState {
    Missing,
    Present,
    Unreadable,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigState {
    Missing,
    Valid,
    Invalid,
    Unreadable,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Missing,
    Registered,
    Partial,
    Stale,
    Disabled,
    Invalid,
    Unreadable,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Component {
    pub(crate) state: State,
    pub(crate) matches_current: Option<bool>,
}

impl Component {
    fn unknown(state: State) -> Self {
        Self {
            state,
            matches_current: None,
        }
    }

    fn registered(matches_current: Option<bool>, disabled: bool) -> Self {
        Self {
            state: if disabled {
                State::Disabled
            } else if matches_current == Some(false) {
                State::Stale
            } else {
                State::Registered
            },
            matches_current,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Capture {
    pub(crate) kind: &'static str,
    #[serde(flatten)]
    pub(crate) component: Component,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Trust {
    Missing,
    Matching,
    Stale,
    Invalid,
    Unreadable,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct AgentReadiness {
    pub(crate) agent: &'static str,
    pub(crate) launch_file_found: Option<bool>,
    pub(crate) directory_found: Option<bool>,
    pub(crate) capture: Capture,
    pub(crate) mcp: Component,
    pub(crate) trust: Trust,
    pub(crate) live_verified: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Readiness {
    pub(crate) home: HomeState,
    pub(crate) config: ConfigState,
    pub(crate) agents: Vec<AgentReadiness>,
}

impl Readiness {
    pub(super) fn complete(&self) -> bool {
        let component = |component: &Component| match component.state {
            State::Missing | State::NotApplicable | State::Partial => true,
            State::Registered | State::Stale | State::Disabled => {
                component.matches_current.is_some()
            }
            State::Invalid | State::Unreadable | State::Unavailable => false,
        };
        !matches!(self.home, HomeState::Unreadable)
            && matches!(self.config, ConfigState::Missing | ConfigState::Valid)
            && self.agents.iter().all(|agent| {
                agent.launch_file_found.is_some()
                    && agent.directory_found.is_some()
                    && component(&agent.capture.component)
                    && component(&agent.mcp)
                    && !matches!(
                        agent.trust,
                        Trust::Invalid | Trust::Unreadable | Trust::Unavailable
                    )
            })
    }
}

type Observed<T> = std::result::Result<Option<T>, State>;

fn found(path: &Path, directory: bool) -> Option<bool> {
    match diagnostic_metadata(path) {
        Ok(metadata) => Some(if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

pub(crate) fn launch_found(bins: &[&str]) -> Option<bool> {
    let Some(paths) = std::env::var_os("PATH") else {
        return Some(false);
    };
    let mut result = Some(false);
    for dir in super::path_dirs(&paths) {
        for file in bins.iter().flat_map(|bin| launcher_files(&dir, bin)) {
            match found(&file, false) {
                Some(true) => return Some(true),
                None => result = None,
                Some(false) => {}
            }
        }
    }
    result
}

// Inventory is diagnostic: oversized files remain unavailable, never parsed as a prefix.
pub(super) const TEXT_LIMIT: u64 = 1024 * 1024;

fn bounded_text(reader: impl Read) -> std::result::Result<String, State> {
    let mut bytes = Vec::new();
    reader
        .take(TEXT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| State::Unreadable)?;
    if bytes.len() as u64 > TEXT_LIMIT {
        return Err(State::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| State::Invalid)
}

/// Missing and unreadable stay distinct. Read only regular files, including dotfile symlinks.
pub(super) fn text(file: &Path) -> Observed<String> {
    match diagnostic_metadata(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
            return Err(State::Unavailable);
        }
        Err(_) => return Err(State::Unreadable),
        Ok(metadata) if !metadata.is_file() => return Err(State::Unavailable),
        Ok(metadata) if metadata.len() > TEXT_LIMIT => return Err(State::Unavailable),
        Ok(_) => {}
    }
    let opened = super::diagnostic_read_file(file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::Unsupported {
            State::Unavailable
        } else {
            State::Unreadable
        }
    })?;
    let metadata = opened.metadata().map_err(|_| State::Unreadable)?;
    if !metadata.is_file() || metadata.len() > TEXT_LIMIT {
        return Err(State::Unavailable);
    }
    bounded_text(opened).map(Some)
}

fn json_file(file: &Path, empty_object: bool) -> Observed<Value> {
    let Some(text) = text(file)? else {
        return Ok(None);
    };
    let root = if empty_object && text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).map_err(|_| State::Invalid)?
    };
    if root.is_object() {
        Ok(Some(root))
    } else {
        Err(State::Invalid)
    }
}

fn claude_mcp_registration() -> Observed<Value> {
    let [legacy, current] = claude_mcp_paths();
    match diagnostic_metadata(&legacy) {
        Ok(_) => json_file(&legacy, true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json_file(&current, true),
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => Err(State::Unavailable),
        Err(_) => Err(State::Unreadable),
    }
}

fn toml_file(file: &Path) -> Observed<toml_edit::DocumentMut> {
    text(file)?
        .map(|text| text.parse().map_err(|_| State::Invalid))
        .transpose()
}

fn observed<T>(value: &Observed<T>) -> std::result::Result<&T, Component> {
    match value {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err(Component::unknown(State::Missing)),
        Err(state) => Err(Component::unknown(*state)),
    }
}

fn optional_object<'a>(
    root: &'a Value,
    name: &str,
) -> std::result::Result<Option<&'a serde_json::Map<String, Value>>, State> {
    root.get(name)
        .map(|value| value.as_object().ok_or(State::Invalid))
        .transpose()
}

fn boolean(root: &Value, name: &str, default: bool) -> std::result::Result<bool, State> {
    root.get(name)
        .map_or(Ok(default), |value| value.as_bool().ok_or(State::Invalid))
}

fn compare_groups(current: &[(String, Value)], wanted: Option<Vec<(String, Value)>>) -> Component {
    if current.is_empty() {
        return Component::unknown(State::Missing);
    }
    let Some(wanted) = wanted else {
        return Component::registered(None, false);
    };
    let matched = wanted
        .iter()
        .filter(|expected| current.contains(*expected))
        .count();
    let same = matched == wanted.len() && current.len() == wanted.len();
    if !same && matched > 0 && current.len() < wanted.len() {
        Component {
            state: State::Partial,
            matches_current: Some(false),
        }
    } else {
        Component::registered(Some(same), false)
    }
}

fn grouped_hooks(
    value: &Observed<Value>,
    want: Option<&HookCommand>,
    groups: fn(&HookCommand) -> Vec<(String, Value)>,
) -> Component {
    let root = match observed(value) {
        Ok(root) => root,
        Err(state) => return state,
    };
    let hooks = match optional_object(root, "hooks") {
        Ok(Some(hooks)) => hooks,
        Ok(None) => return Component::unknown(State::Missing),
        Err(state) => return Component::unknown(state),
    };
    let mut current = Vec::new();
    for (event, value) in hooks {
        let Some(entries) = value.as_array() else {
            return Component::unknown(State::Invalid);
        };
        for group in entries.iter().filter(|group| has_ours(group)) {
            current.push((event.clone(), only_ours(group)));
        }
    }
    compare_groups(&current, want.map(groups))
}

fn agy_hooks(value: &Observed<Value>, want: Option<&HookCommand>) -> Component {
    let root = match observed(value) {
        Ok(root) => root,
        Err(state) => return state,
    };
    let Some(own) = root.get(MCP_NAME) else {
        return Component::unknown(State::Missing);
    };
    if !own.is_object() {
        return Component::unknown(State::Invalid);
    }
    let disabled = match boolean(own, "enabled", true) {
        Ok(enabled) => !enabled,
        Err(state) => return Component::unknown(state),
    };
    let mut hooks = own.clone();
    hooks
        .as_object_mut()
        .expect("checked Agy object")
        .remove("enabled");
    let same = match want.map(|want| agy_spec(want, cfg!(windows))).transpose() {
        Ok(expected) => expected.map(|expected| hooks == expected),
        Err(_) => return Component::unknown(State::Unavailable),
    };
    Component::registered(same, disabled)
}

fn cursor_hooks(value: &Observed<Value>, want: Option<&HookCommand>) -> Component {
    let root = match observed(value) {
        Ok(root) => root,
        Err(state) => return state,
    };
    let hooks = match optional_object(root, "hooks") {
        Ok(Some(hooks)) => hooks,
        Ok(None) => return Component::unknown(State::Missing),
        Err(state) => return Component::unknown(state),
    };
    let mut current = Vec::new();
    for (step, entries) in hooks {
        let Some(entries) = entries.as_array() else {
            return Component::unknown(State::Invalid);
        };
        current.extend(
            entries
                .iter()
                .filter(|entry| is_our_handler(entry))
                .map(|entry| (step.clone(), entry.clone())),
        );
    }
    let wanted = match want
        .map(|want| cursor_hook_spec(want, cfg!(windows)))
        .transpose()
    {
        Ok(value) => value.map(|value| {
            value
                .as_object()
                .expect("native hook spec")
                .iter()
                .flat_map(|(step, entries)| {
                    entries
                        .as_array()
                        .expect("native entries")
                        .iter()
                        .map(|entry| (step.clone(), entry.clone()))
                })
                .collect()
        }),
        Err(_) => return Component::unknown(State::Unavailable),
    };
    compare_groups(&current, wanted)
}

fn template(
    file: &Path,
    want: Option<&HookCommand>,
    build: fn(&HookCommand) -> Result<String>,
    marker: Option<&str>,
) -> Component {
    let value = text(file);
    let actual = match observed(&value) {
        Ok(value) => value,
        Err(state) => return state,
    };
    if marker.is_some_and(|marker| !actual.starts_with(marker)) {
        return Component::unknown(State::Unavailable);
    }
    match want.map(build).transpose() {
        Ok(expected) => Component::registered(expected.map(|expected| actual == &expected), false),
        Err(_) => Component::unknown(State::Unavailable),
    }
}

fn json_mcp(value: &Observed<Value>, want: Option<&HookCommand>, opencode: bool) -> Component {
    let root = match observed(value) {
        Ok(root) => root,
        Err(state) => return state,
    };
    let servers = if opencode {
        match root.get("mcp") {
            None => return Component::unknown(State::Missing),
            Some(root) if root.is_object() => optional_object(root, "servers"),
            Some(_) => Err(State::Invalid),
        }
    } else {
        optional_object(root, "mcpServers")
    };
    let servers = match servers {
        Ok(Some(servers)) => servers,
        Ok(None) => return Component::unknown(State::Missing),
        Err(state) => return Component::unknown(state),
    };
    let Some(entry) = servers.get(MCP_NAME) else {
        return Component::unknown(State::Missing);
    };
    if !entry.is_object() {
        return Component::unknown(State::Invalid);
    }
    let disabled = match boolean(entry, "disabled", false) {
        Ok(value) => value,
        Err(state) => return Component::unknown(state),
    };
    let disabled = if opencode {
        disabled
    } else {
        mcp_disabled_in_json(root)
    };
    let same = if opencode {
        let Some(command) = entry.get("command").and_then(Value::as_array) else {
            return Component::unknown(State::Invalid);
        };
        if command.iter().any(|arg| !arg.is_string()) || !entry["type"].is_string() {
            return Component::unknown(State::Invalid);
        }
        want.map(|want| {
            let expected: Vec<_> = std::iter::once(want.exe.clone())
                .chain(want.mcp_args())
                .collect();
            entry["type"] == "local" && entry["command"] == json!(expected)
        })
    } else {
        let Some((command, args)) = mcp_command_in_json(root) else {
            return Component::unknown(State::Invalid);
        };
        want.map(|want| command == want.exe && args == want.mcp_args())
    };
    Component::registered(same, disabled)
}

fn toml_mcp_state(
    value: &Observed<toml_edit::DocumentMut>,
    want: Option<&HookCommand>,
) -> Component {
    let doc = match observed(value) {
        Ok(doc) => doc,
        Err(state) => return state,
    };
    let Some(servers) = doc.get("mcp_servers") else {
        return Component::unknown(State::Missing);
    };
    let Some(servers) = servers.as_table_like() else {
        return Component::unknown(State::Invalid);
    };
    let Some(entry) = servers.get(MCP_NAME) else {
        return Component::unknown(State::Missing);
    };
    let Some(entry) = entry.as_table_like() else {
        return Component::unknown(State::Invalid);
    };
    let Some(args) = entry.get("args").and_then(toml_edit::Item::as_array) else {
        return Component::unknown(State::Invalid);
    };
    // The legacy CLI filters non-string arguments; the typed inventory must expose that error.
    if args.iter().any(|arg| arg.as_str().is_none()) {
        return Component::unknown(State::Invalid);
    }
    if entry
        .get("enabled")
        .is_some_and(|enabled| enabled.as_bool().is_none())
    {
        return Component::unknown(State::Invalid);
    }
    let Some((command, args)) = mcp_command_in_toml(doc) else {
        return Component::unknown(State::Invalid);
    };
    Component::registered(
        want.map(|want| command == want.exe && args == want.mcp_args()),
        mcp_disabled_in_toml(doc),
    )
}

fn codex_trust(
    hooks: &Observed<Value>,
    config: &Observed<toml_edit::DocumentMut>,
    file: &Path,
) -> Trust {
    let root = match hooks {
        Ok(Some(root)) => root,
        Ok(None) => return Trust::Missing,
        Err(State::Invalid) => return Trust::Invalid,
        Err(State::Unavailable) => return Trust::Unavailable,
        Err(_) => return Trust::Unreadable,
    };
    let keys: Vec<_> = codex_trust_keys(file, root)
        .into_iter()
        .filter(|key| key.ours)
        .collect();
    if keys.is_empty() {
        return Trust::Missing;
    }
    let doc = match config {
        Ok(Some(doc)) => doc,
        Ok(None) => return Trust::Missing,
        Err(State::Invalid) => return Trust::Invalid,
        Err(State::Unavailable) => return Trust::Unavailable,
        Err(_) => return Trust::Unreadable,
    };
    let Some(hooks) = doc.get("hooks") else {
        return Trust::Missing;
    };
    let Some(hooks) = hooks.as_table_like() else {
        return Trust::Invalid;
    };
    let Some(state) = hooks.get("state") else {
        return Trust::Missing;
    };
    let Some(state) = state.as_table_like() else {
        return Trust::Invalid;
    };
    let mut matched = 0;
    for key in &keys {
        let Some(row) = state.get(&key.key) else {
            continue;
        };
        let Some(row) = row.as_table_like() else {
            return Trust::Invalid;
        };
        let Some(hash) = row.get("trusted_hash").and_then(toml_edit::Item::as_str) else {
            return Trust::Invalid;
        };
        matched += usize::from(hash == key.hash.as_str());
    }
    if matched == keys.len() {
        Trust::Matching
    } else {
        Trust::Stale
    }
}

fn row(agent: &'static str, want: Option<&HookCommand>) -> AgentReadiness {
    let dir = match agent {
        "claude" => claude_dir(),
        "codex" => codex_home(),
        "grok" => crate::hook::grok_home(),
        "agy" => agy_dir(),
        "opencode" => opencode_dir(),
        "pi" => pi_dir(),
        "cursor" => cursor_dir(),
        _ => unreachable!("the native seven-agent list"),
    };
    let launch_file_found = if agent == "cursor" {
        launch_found(&["cursor-agent", "agent"])
    } else {
        launch_found(&[agent])
    };
    let mut trust = Trust::NotApplicable;
    let (kind, capture, mcp) = match agent {
        "claude" => (
            "hooks",
            grouped_hooks(
                &json_file(&claude_settings_file(), true),
                want,
                claude_groups,
            ),
            json_mcp(&claude_mcp_registration(), want, false),
        ),
        "codex" => {
            let file = dir.join("hooks.json");
            let hooks = json_file(&file, true);
            let config = toml_file(&dir.join("config.toml"));
            let capture = grouped_hooks(&hooks, want, codex_groups);
            trust = match capture.state {
                State::Invalid => Trust::Invalid,
                _ => codex_trust(&hooks, &config, &file),
            };
            ("hooks", capture, toml_mcp_state(&config, want))
        }
        "grok" => (
            "hooks",
            grouped_hooks(
                &json_file(&crate::hook::grok_hooks_file(), true),
                want,
                grok_groups,
            ),
            toml_mcp_state(&toml_file(&grok_config_file()), want),
        ),
        "agy" => (
            "hooks",
            agy_hooks(&json_file(&dir.join("config/hooks.json"), true), want),
            json_mcp(
                &json_file(&dir.join("config/mcp_config.json"), true),
                want,
                false,
            ),
        ),
        "opencode" => {
            let config = match diagnostic_metadata(&dir.join("opencode.jsonc")) {
                Ok(_) => Err(State::Unavailable),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    json_file(&dir.join("opencode.json"), false)
                }
                Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    Err(State::Unavailable)
                }
                Err(_) => Err(State::Unreadable),
            };
            (
                "plugin",
                template(&dir.join("plugins/oboete.js"), want, opencode_plugin, None),
                json_mcp(&config, want, true),
            )
        }
        "pi" => (
            "extension",
            template(
                &dir.join("extensions/oboete.ts"),
                want,
                pi_extension,
                Some(PI_MARKER),
            ),
            Component::unknown(State::NotApplicable),
        ),
        "cursor" => (
            "hooks",
            cursor_hooks(&json_file(&dir.join("hooks.json"), false), want),
            json_mcp(&json_file(&dir.join("mcp.json"), false), want, false),
        ),
        _ => unreachable!("the native seven-agent list"),
    };
    AgentReadiness {
        agent,
        launch_file_found,
        directory_found: found(&dir, true),
        capture: Capture {
            kind,
            component: capture,
        },
        mcp,
        trust,
        live_verified: false,
    }
}

pub(super) fn current_command(
    home: &Path,
    canonicalize: impl Fn(&Path) -> std::io::Result<std::path::PathBuf>,
) -> Option<HookCommand> {
    let unavailable = std::cell::Cell::new(false);
    // The CLI deliberately defaults an unresolvable default home to custom-home args.
    // Passive alignment must retain errors from the actual comparison, not a prior probe.
    let command = HookCommand::current_with(home, |path| {
        let result = canonicalize(path);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        {
            unavailable.set(true);
        }
        result
    })
    .ok();
    command.filter(|_| !unavailable.get())
}

pub(crate) fn readiness(home: &Path) -> Readiness {
    let config = text(&home.join("config.toml"));
    readiness_from_text(
        home,
        config
            .as_ref()
            .map(|text| text.as_deref())
            .map_err(|state| *state),
    )
}

/// Doctor supplies the same admitted bytes used by its other configuration checks.
/// The cheap inventory GET retains its independent file read above.
pub(super) fn readiness_from_text(home: &Path, config: Observed<&str>) -> Readiness {
    let home_state = match diagnostic_metadata(home) {
        Ok(metadata) if metadata.is_dir() => HomeState::Present,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HomeState::Missing,
        _ => HomeState::Unreadable,
    };
    let path = home.join("config.toml");
    let config = match config {
        Ok(None) => ConfigState::Missing,
        Ok(Some(text)) if crate::settings::parsed(&path, text).is_some() => ConfigState::Valid,
        Ok(Some(_)) | Err(State::Invalid) => ConfigState::Invalid,
        Err(_) => ConfigState::Unreadable,
    };
    let want = if matches!(home_state, HomeState::Present) {
        current_command(home, diagnostic_canonicalize)
    } else {
        None
    };
    Readiness {
        home: home_state,
        config,
        agents: AGENTS
            .iter()
            .map(|agent| row(agent, want.as_ref()))
            .collect(),
    }
}

#[cfg(test)]
mod tests;
