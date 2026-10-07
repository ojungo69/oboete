//! Passive agent-file inventory. Never invokes setup, doctor, a store, or an agent process.

use super::{
    AGENTS, HookCommand, MCP_NAME, PI_MARKER, agy_dir, agy_spec, claude_dir, claude_groups,
    claude_mcp_paths, claude_settings_file, codex_groups, codex_home, codex_trust_keys, cursor_dir,
    cursor_hook_spec, diagnostic_canonicalize, diagnostic_metadata, grok_config_file, grok_groups,
    has_ours, is_our_handler, launcher_files, mcp_command_in_json, mcp_command_in_toml,
    mcp_disabled_in_json, mcp_disabled_in_toml, opencode_dir, opencode_plugin, pi_dir,
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
    for dir in std::env::split_paths(&paths) {
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
const TEXT_LIMIT: u64 = 1024 * 1024;

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
fn text(file: &Path) -> Observed<String> {
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
    #[cfg(not(windows))]
    let mut options = std::fs::OpenOptions::new();
    #[cfg(not(windows))]
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(windows))]
    let opened = options.open(file).map_err(|_| State::Unreadable)?;
    #[cfg(windows)]
    let opened = super::diagnostic_file(file, true).map_err(|error| {
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
            let mut own = group.clone();
            own["hooks"] = Value::Array(
                group["hooks"]
                    .as_array()
                    .expect("has_ours checked handlers")
                    .iter()
                    .filter(|handler| is_our_handler(handler))
                    .cloned()
                    .collect(),
            );
            current.push((event.clone(), own));
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

fn current_command(
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
    let home_state = match diagnostic_metadata(home) {
        Ok(metadata) if metadata.is_dir() => HomeState::Present,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HomeState::Missing,
        _ => HomeState::Unreadable,
    };
    let path = home.join("config.toml");
    let config = match text(&path) {
        Ok(None) => ConfigState::Missing,
        Ok(Some(text)) if crate::settings::parsed(&path, &text).is_some() => ConfigState::Valid,
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
mod tests {
    use super::*;
    use crate::setup as native;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn w6_agy_switch_is_separate_from_command_alignment() {
        let want = HookCommand {
            exe: "oboete".into(),
            home: None,
        };
        let other = HookCommand {
            exe: "synthetic-other-binary".into(),
            home: None,
        };
        for enabled in [Some(false), None] {
            let mut own = agy_spec(&want, cfg!(windows)).unwrap();
            if let Some(enabled) = enabled {
                own["enabled"] = json!(enabled);
            } else {
                own.as_object_mut().unwrap().remove("enabled");
            }
            let observed = Ok(Some(json!({"oboete": own})));
            let actual = agy_hooks(&observed, Some(&want));
            assert_eq!(
                actual.matches_current,
                Some(true),
                "Agy switch changed matching commands"
            );
            assert!(matches!(actual.state, State::Disabled) == (enabled == Some(false)));
            assert_eq!(
                agy_hooks(&observed, Some(&other)).matches_current,
                Some(false),
                "Agy switch hid a different command"
            );
        }
    }

    #[test]
    fn w6_current_command_retains_actual_comparison_errors() {
        let private = tempfile::tempdir().unwrap();
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotFound,
        ] {
            let calls = std::cell::Cell::new(0);
            let command = current_command(private.path(), |path| {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    Err(kind.into())
                } else {
                    Ok(path.to_owned())
                }
            });
            assert_eq!(calls.get(), 3, "actual default comparison was not observed");
            assert_eq!(
                command.is_some(),
                kind == std::io::ErrorKind::NotFound,
                "actual default comparison error became a known command"
            );
        }
    }

    #[test]
    fn w6_inventory_refuses_oversized_text_without_truncating() {
        let private = tempfile::tempdir().unwrap();
        let file = private.path().join("settings.json");
        let mut value = vec![b' '; 1024 * 1024 + 1];
        value[..2].copy_from_slice(b"{}");
        std::fs::write(&file, &value).unwrap();
        assert!(
            matches!(text(&file), Err(State::Unavailable)),
            "oversized inventory file was read"
        );
        assert!(
            std::fs::read(&file).unwrap() == value,
            "inspection changed source bytes"
        );
        value.truncate(1024 * 1024);
        std::fs::write(&file, &value).unwrap();
        assert!(
            matches!(json_file(&file, false), Ok(Some(_))),
            "file at the limit was refused"
        );
        std::fs::write(&file, [0xff]).unwrap();
        assert!(matches!(text(&file), Err(State::Invalid)));
        // Even a stream that grows after metadata inspection consumes only limit + 1 bytes.
        let mut growing = std::io::repeat(b' ').take(TEXT_LIMIT + 5);
        assert!(matches!(
            bounded_text(&mut growing),
            Err(State::Unavailable)
        ));
        assert_eq!(growing.limit(), 4);
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if metadata.is_dir() {
                files.insert(relative, None);
                pending.extend(
                    std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else if metadata.file_type().is_symlink() {
                files.insert(
                    relative,
                    Some(
                        std::fs::read_link(&path)
                            .unwrap()
                            .into_os_string()
                            .into_encoded_bytes(),
                    ),
                );
            } else {
                assert!(metadata.is_file(), "private fixture gained a special file");
                files.insert(relative, Some(std::fs::read(&path).unwrap()));
            }
        }
        files
    }

    fn agent<'a>(shown: &'a Value, name: &str) -> &'a Value {
        shown["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["agent"] == name)
            .expect("fixed agent row missing")
    }

    fn codex_guarded_files_keep_unknown_trust(root: &Path, home: &Path) {
        for (name, prefix) in [
            ("hooks.json", b"{}".as_slice()),
            ("config.toml", b"#".as_slice()),
        ] {
            let file = native::codex_home().join(name);
            let kept = std::fs::read(&file).unwrap();
            let mut oversized = vec![b' '; TEXT_LIMIT as usize + 1];
            oversized[..prefix.len()].copy_from_slice(prefix);
            std::fs::write(&file, oversized).unwrap();
            let before = snapshot(root);
            let shown = serde_json::to_value(native::readiness(home)).unwrap();
            assert!(
                snapshot(root) == before,
                "guarded Codex read changed private files"
            );
            assert!(
                agent(&shown, "codex")["trust"] == "unavailable",
                "guarded Codex trust was reported as a read failure"
            );
            std::fs::write(file, kept).unwrap();
        }
    }

    fn unavailable_cli_is_unknown_in_both_settings_views(root: &Path) {
        let home = root.join("provider-unknown");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("config.toml"),
            "[[providers]]\nkind = \"cli\"\nname = \"private-cli\"\ncli = \"claude\"\n[summary]\ncurate = false\n").unwrap();
        let before = snapshot(root);
        let shown = crate::settings::show(&home);
        assert!(
            snapshot(root) == before,
            "passive CLI settings changed private files"
        );
        assert!(
            shown["chain"][0]["key"] == "unknown",
            "unavailable CLI was reported missing in chain settings"
        );
        assert!(
            shown["providers"][0]["saved"]["key"] == "unknown"
                && shown["providers"][0]["effective"]["key"] == "unknown",
            "unavailable CLI was reported missing in provider settings"
        );
    }

    #[cfg(windows)]
    fn windows_junction_checks(root: &Path, home: &Path, changed: &Value) {
        // A local junction exercises the kernel's parent-reparse refusal without ever
        // naming or contacting a remote share. cmd is the fixed OS junction creator.
        let junction = |link: &Path, target: &Path, failure: &str| {
            let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32")
                .join("cmd.exe");
            let output = std::process::Command::new(cmd)
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            let scrub = |bytes: &[u8]| {
                String::from_utf8_lossy(bytes)
                    .replace(root.to_string_lossy().as_ref(), "<private-root>")
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .take(600)
                    .collect::<String>()
            };
            assert!(
                output.status.success(),
                "{failure}; status={:?}; stdout={}; stderr={}",
                output.status.code(),
                scrub(&output.stdout),
                scrub(&output.stderr)
            );
        };
        let dir = native::claude_dir();
        let kept = root.join("claude-before-junction");
        std::fs::rename(&dir, &kept).unwrap();
        junction(&dir, &kept, "private local junction fixture failed");
        assert!(
            std::fs::metadata(dir.join("settings.json"))
                .unwrap()
                .is_file(),
            "junction control did not resolve to a local file"
        );
        let link = std::fs::read_link(&dir).unwrap();
        let bytes = std::fs::read(kept.join("settings.json")).unwrap();
        let shown = serde_json::to_value(native::readiness(home)).unwrap();
        let claude = agent(&shown, "claude");
        assert!(
            claude["directory_found"].is_null() && claude["capture"]["state"] == "unavailable",
            "Windows diagnostics followed a parent junction"
        );
        assert!(
            std::fs::read_link(&dir).unwrap() == link
                && std::fs::read(kept.join("settings.json")).unwrap() == bytes,
            "junction inspection changed its target or source bytes"
        );
        for name in ["codex", "grok", "agy", "opencode", "pi", "cursor"] {
            assert!(
                agent(&shown, name) == agent(changed, name),
                "junction failure changed an independent row"
            );
        }
        let bin = root.join("bin");
        let kept_bin = root.join("bin-before-junction");
        std::fs::rename(&bin, &kept_bin).unwrap();
        std::fs::write(kept_bin.join("grok"), b"inert, never executed").unwrap();
        junction(&bin, &kept_bin, "private launcher junction fixture failed");
        assert!(
            native::on_path("grok"),
            "native CLI lost its local-link launcher"
        );
        assert_eq!(
            native::launch_found(&["grok"]),
            None,
            "passive launcher probe followed a junction"
        );
    }

    fn registered_matrix_root() -> Option<PathBuf> {
        const CHILD: &str = "OBOETE_W6_REGISTERED_MATRIX_CHILD";
        const TEST: &str = "setup::readiness::tests::w6_registered_agents_matrix_is_read_only";
        let Some(root) = std::env::var_os(CHILD).map(PathBuf::from) else {
            // The inventory reads process environment; isolate the entire test rather than
            // changing environment variables seen by other Rust tests.
            let private = tempfile::Builder::new()
                .prefix("oboete w6 ")
                .tempdir()
                .unwrap();
            let root = private.path();
            for directory in [
                "store",
                "owner",
                "owner/.gemini",
                "tmp",
                "bin",
                "cwd",
                "xdg/config",
                "xdg/cache",
                "xdg/data",
                "xdg/state",
                "agents/claude",
                "agents/codex",
                "agents/grok",
                "agents/opencode",
                "agents/pi",
                "agents/cursor",
            ] {
                std::fs::create_dir_all(root.join(directory)).unwrap();
            }
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--nocapture"])
                .env_clear()
                .env(CHILD, root)
                .env("HOME", root.join("owner"))
                .env("USERPROFILE", root.join("owner"))
                .env("OBOETE_HOME", root.join("store"))
                .env("OBOETE_NO_SPAWN", "1")
                .env("CLAUDE_CONFIG_DIR", root.join("agents/claude"))
                .env("CODEX_HOME", root.join("agents/codex"))
                .env("GROK_HOME", root.join("agents/grok"))
                .env("OPENCODE_CONFIG_DIR", root.join("agents/opencode"))
                .env("PI_CODING_AGENT_DIR", root.join("agents/pi"))
                .env("CURSOR_CONFIG_DIR", root.join("agents/cursor"))
                .env("XDG_CONFIG_HOME", root.join("xdg/config"))
                .env("XDG_CACHE_HOME", root.join("xdg/cache"))
                .env("XDG_DATA_HOME", root.join("xdg/data"))
                .env("XDG_STATE_HOME", root.join("xdg/state"))
                .env("TMPDIR", root.join("tmp"))
                .env("TMP", root.join("tmp"))
                .env("TEMP", root.join("tmp"))
                .env("PATH", root.join("bin"))
                .current_dir(root.join("cwd"));
            #[cfg(windows)]
            command.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
            // Keep only explicitly supplied instrumentation output, outside child cleanup.
            if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
                let profile = PathBuf::from(profile);
                let profile = if profile.as_os_str().is_empty() || profile.is_absolute() {
                    profile
                } else {
                    std::env::current_dir().unwrap().join(profile)
                };
                command.env("LLVM_PROFILE_FILE", profile);
            }
            let status = command.status().unwrap();
            assert!(status.success(), "isolated registered-agent matrix failed");
            assert!(
                root.join("matrix-complete").is_file(),
                "exact child test did not finish"
            );
            return None;
        };

        Some(root)
    }

    #[test]
    fn w6_registered_agents_matrix_is_read_only() {
        let Some(root) = registered_matrix_root() else {
            return;
        };

        let home = root.join("store");
        std::fs::write(
            home.join("config.toml"),
            "providers = []\n[summary]\ncurate = false\n",
        )
        .unwrap();
        let current = native::HookCommand::current(&home).unwrap();
        let current_mcp = json!({"mcpServers":{
            "oboete":{"command":current.exe,"args":current.mcp_args()}
        }});
        let agy_supported = native::agy_spec(&current, cfg!(windows)).is_ok();
        let cursor_supported = native::cursor_hook_spec(&current, cfg!(windows)).is_ok();
        // An inert registration exposes Windows' honest current-path refusal. Keep the native
        // platform checks; no fixture command is executed, including this safe relative binary.
        let inert = native::HookCommand {
            exe: "oboete".into(),
            home: None,
        };
        // Seed real native file formats. Never call wire/run/claude_mcp or any agent CLI.
        native::claude(&current, false).unwrap();
        native::write_json(&native::claude_mcp_file(), &current_mcp).unwrap();
        native::codex(&current, false).unwrap();
        native::toml_mcp(&native::codex_home().join("config.toml"), &current, false).unwrap();
        native::grok(&current, false).unwrap();
        native::toml_mcp(&native::grok_config_file(), &current, false).unwrap();
        native::agy_files(
            &native::agy_dir(),
            if agy_supported { &current } else { &inert },
            false,
            cfg!(windows),
        )
        .unwrap();
        if !agy_supported {
            native::write_json(
                &native::agy_dir().join("config/mcp_config.json"),
                &current_mcp,
            )
            .unwrap();
        }
        native::opencode_files(&native::opencode_dir(), &current, false).unwrap();
        native::pi_files(&native::pi_dir(), &current, false).unwrap();
        native::cursor_files(
            &native::cursor_dir(),
            if cursor_supported { &current } else { &inert },
            false,
            cfg!(windows),
        )
        .unwrap();
        if !cursor_supported {
            native::write_json(&native::cursor_dir().join("mcp.json"), &current_mcp).unwrap();
        }

        let inspect = || {
            let before = snapshot(&root);
            let shown = serde_json::to_value(native::readiness(&home)).unwrap();
            assert!(
                snapshot(&root) == before,
                "readiness changed private files or paths"
            );
            assert!(
                shown["home"] == "present" && shown["config"] == "valid",
                "private home/config state changed"
            );
            let rows = shown["agents"]
                .as_array()
                .expect("serialized agent rows missing");
            let expected = ["claude", "codex", "grok", "agy", "opencode", "pi", "cursor"];
            assert!(
                rows.len() == expected.len(),
                "registered inventory lost an agent"
            );
            for (row, name) in rows.iter().zip(expected) {
                assert!(row["agent"] == name, "agent order or identity changed");
                assert!(
                    row["directory_found"] == true,
                    "native integration directory not found"
                );
                assert!(
                    row["launch_file_found"] == false,
                    "private empty PATH reported a launcher"
                );
                assert!(
                    row["live_verified"] == false,
                    "file registration became live verification"
                );
            }
            shown
        };

        let registered = inspect();
        for name in ["claude", "codex", "grok", "agy", "opencode", "pi", "cursor"] {
            let row = agent(&registered, name);
            let supported = match name {
                "agy" => agy_supported,
                "cursor" => cursor_supported,
                _ => true,
            };
            if supported {
                assert!(
                    row["capture"]["state"] == "registered",
                    "native capture registration not recognized"
                );
                assert!(
                    row["capture"]["matches_current"] == true,
                    "native capture does not match current command"
                );
            } else {
                assert!(
                    row["capture"]["state"] == "unavailable",
                    "native Windows path refusal was hidden"
                );
                assert!(
                    row["capture"]["matches_current"].is_null(),
                    "unsupported capture claimed alignment"
                );
            }
            if name == "pi" {
                assert!(
                    row["mcp"]["state"] == "not_applicable"
                        && row["mcp"]["matches_current"].is_null(),
                    "Pi was assigned an MCP client"
                );
            } else {
                assert!(
                    row["mcp"]["state"] == "registered" && row["mcp"]["matches_current"] == true,
                    "native MCP registration not recognized"
                );
            }
            assert!(
                row["trust"]
                    == if name == "codex" {
                        "matching"
                    } else {
                        "not_applicable"
                    },
                "native trust state is not truthful"
            );
        }

        // Each change represents a distinct real integration state, not a helper's output.
        let claude = native::claude_settings_file();
        let mut hooks = native::read_json_object(&claude).unwrap();
        hooks["hooks"].as_object_mut().unwrap().remove("SessionEnd");
        native::write_json(&claude, &hooks).unwrap();

        let codex = native::codex_home().join("config.toml");
        let mut config: toml_edit::DocumentMut =
            std::fs::read_to_string(&codex).unwrap().parse().unwrap();
        let (_, first_trust) = config["hooks"]["state"]
            .as_table_like_mut()
            .unwrap()
            .iter_mut()
            .next()
            .unwrap();
        first_trust["trusted_hash"] = toml_edit::value("sha256:stale");
        std::fs::write(&codex, config.to_string()).unwrap();

        let grok = native::grok_config_file();
        let mut config: toml_edit::DocumentMut =
            std::fs::read_to_string(&grok).unwrap().parse().unwrap();
        config["mcp_servers"]["oboete"]["enabled"] = toml_edit::value(false);
        std::fs::write(&grok, config.to_string()).unwrap();

        let agy = native::agy_dir().join("config/hooks.json");
        let mut hooks = native::read_json_object(&agy).unwrap();
        hooks["oboete"]["enabled"] = json!(false);
        native::write_json(&agy, &hooks).unwrap();

        for file in [
            native::opencode_dir().join("plugins/oboete.js"),
            native::pi_dir().join("extensions/oboete.ts"),
        ] {
            let mut contents = std::fs::read_to_string(&file).unwrap();
            contents.push_str("\n// private fixture changed after registration\n");
            std::fs::write(file, contents).unwrap();
        }
        let cursor = native::cursor_dir().join("mcp.json");
        let mut config = native::read_json_object(&cursor).unwrap();
        config["mcpServers"]["oboete"]["command"] = json!("synthetic-other-binary");
        native::write_json(&cursor, &config).unwrap();

        let changed = inspect();
        for (name, capture, mcp, trust) in [
            ("claude", "partial", "registered", "not_applicable"),
            ("codex", "registered", "registered", "stale"),
            ("grok", "registered", "disabled", "not_applicable"),
            (
                "agy",
                if agy_supported {
                    "disabled"
                } else {
                    "unavailable"
                },
                "registered",
                "not_applicable",
            ),
            ("opencode", "stale", "registered", "not_applicable"),
            ("pi", "stale", "not_applicable", "not_applicable"),
            (
                "cursor",
                if cursor_supported {
                    "registered"
                } else {
                    "unavailable"
                },
                "stale",
                "not_applicable",
            ),
        ] {
            let row = agent(&changed, name);
            assert!(
                row["capture"]["state"] == capture
                    && row["mcp"]["state"] == mcp
                    && row["trust"] == trust,
                "integration mutation produced the wrong fixed states"
            );
        }
        assert!(
            agent(&changed, "grok")["mcp"]["matches_current"] == true,
            "disabled MCP lost its matching command"
        );
        for name in ["claude", "opencode", "pi"] {
            assert!(
                agent(&changed, name)["capture"]["matches_current"] == false,
                "changed capture remained current"
            );
        }
        assert!(
            agent(&changed, "cursor")["mcp"]["matches_current"] == false,
            "changed MCP remained current"
        );

        let opencode = native::opencode_dir().join("opencode.json");
        let kept_codex = std::fs::read(&codex).unwrap();
        let kept_opencode = std::fs::read(&opencode).unwrap();
        std::fs::write(&codex, "[invalid").unwrap();
        std::fs::write(&opencode, "{invalid").unwrap();
        let invalid = inspect();
        for name in ["codex", "opencode"] {
            let row = agent(&invalid, name);
            assert!(
                row["mcp"]["state"] == "invalid" && row["mcp"]["matches_current"].is_null(),
                "malformed registration was treated as absent or current"
            );
            assert!(
                row["capture"] == agent(&changed, name)["capture"],
                "MCP parse failure discarded capture facts"
            );
        }
        assert!(
            agent(&invalid, "codex")["trust"] == "invalid",
            "malformed Codex TOML hid trust failure"
        );
        for name in ["claude", "grok", "agy", "pi", "cursor"] {
            assert!(
                agent(&invalid, name) == agent(&changed, name),
                "one parser failure changed an independent agent"
            );
        }
        std::fs::write(&codex, kept_codex).unwrap();
        std::fs::write(&opencode, kept_opencode).unwrap();
        assert!(
            inspect() == changed,
            "readiness did not recover from restored private settings"
        );
        codex_guarded_files_keep_unknown_trust(&root, &home);
        assert!(
            inspect() == changed,
            "guarded Codex inspection did not recover"
        );
        #[cfg(unix)]
        {
            let default_home = root.join("owner/.oboete");
            std::os::unix::fs::symlink(".oboete", &default_home).unwrap();
            let before = snapshot(&root);
            let unknown = serde_json::to_value(native::readiness(&home)).unwrap();
            assert!(
                snapshot(&root) == before,
                "unknown default home changed private files"
            );
            for row in unknown["agents"].as_array().unwrap() {
                assert!(
                    row["capture"]["matches_current"].is_null()
                        && row["mcp"]["matches_current"].is_null(),
                    "unreadable default home claimed current-command alignment"
                );
            }
            let legacy = native::HookCommand::current(&home).unwrap();
            assert!(
                legacy.exe == current.exe && legacy.home == current.home,
                "native CLI default-home fallback changed"
            );
            std::fs::remove_file(default_home).unwrap();
            assert!(
                inspect() == changed,
                "default-home inspection did not recover"
            );
            // A symlink loop returns a real metadata error even under a privileged test user.
            // It must not become "not found". Only this child's private roots are changed.
            let dir = native::claude_dir();
            std::fs::rename(&dir, root.join("claude-held")).unwrap();
            std::os::unix::fs::symlink(&dir, &dir).unwrap();
            let launcher = root.join("bin/claude");
            std::os::unix::fs::symlink(&launcher, &launcher).unwrap();
            let before = snapshot(&root);
            let shown = serde_json::to_value(native::readiness(&home)).unwrap();
            assert!(
                snapshot(&root) == before,
                "metadata errors changed private files"
            );
            let claude = agent(&shown, "claude");
            assert!(
                claude["directory_found"].is_null(),
                "unreadable agent directory appeared absent"
            );
            assert!(
                claude["launch_file_found"].is_null(),
                "unreadable launcher appeared absent"
            );
            assert!(claude["capture"]["state"] == "unreadable");
            for name in ["codex", "grok", "agy", "opencode", "pi", "cursor"] {
                assert!(
                    agent(&shown, name) == agent(&changed, name),
                    "metadata failure changed an independent row"
                );
            }
            let cursor = root.join("bin/cursor-agent");
            std::os::unix::fs::symlink(&cursor, &cursor).unwrap();
            assert_eq!(launch_found(&["cursor-agent", "agent"]), None);
            std::fs::write(root.join("bin/agent"), b"inert, never executed").unwrap();
            assert_eq!(launch_found(&["cursor-agent", "agent"]), Some(true));
        }
        #[cfg(windows)]
        windows_junction_checks(&root, &home, &changed);
        unavailable_cli_is_unknown_in_both_settings_views(&root);
        std::fs::write(root.join("matrix-complete"), b"passed").unwrap();
    }
}
