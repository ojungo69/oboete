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

/// Decision 108 (docs/hook-time.md): hooks with the timeouts an older `setup` wrote are read
/// as stale, and as current once `setup` has written them again.
#[test]
fn hooks_with_an_older_setups_timeouts_are_stale_until_written_again() {
    let want = HookCommand {
        exe: "oboete".into(),
        home: None,
    };
    type Groups = fn(&HookCommand) -> Vec<(String, Value)>;
    // Each agent's groups, and what `setup` wrote for a start until decision 108.
    let agents: [(Groups, u32); 3] = [(claude_groups, 10), (codex_groups, 10), (grok_groups, 5)];
    for (agent, (groups, start)) in agents.into_iter().enumerate() {
        let mut root = json!({});
        native::merge_groups(&mut root, groups(&want));
        let read = |root: &Value| grouped_hooks(&Ok(Some(root.clone())), Some(&want), groups);
        assert_eq!(read(&root).matches_current, Some(true), "{agent}");
        // As `setup` wrote them until then: 5 s, a start as above, a session's end 3 s.
        for (event, groups) in root["hooks"].as_object_mut().unwrap() {
            let secs = match event.as_str() {
                "SessionStart" | "SubagentStart" => start,
                "SessionEnd" => 3,
                _ => 5,
            };
            groups[0]["hooks"][0]["timeout"] = json!(secs);
        }
        let older = read(&root);
        assert_eq!(older.matches_current, Some(false), "{agent}");
        assert!(matches!(older.state, State::Stale), "{agent}");
        native::merge_groups(&mut root, groups(&want));
        let again = read(&root);
        assert_eq!(again.matches_current, Some(true), "{agent}");
        assert!(matches!(again.state, State::Registered), "{agent}");
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
                row["mcp"]["state"] == "not_applicable" && row["mcp"]["matches_current"].is_null(),
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
