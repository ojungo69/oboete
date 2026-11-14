    use super::*;

    /// docs/hook-time.md, decision 108: a hook's own time is counted from the timeout every
    /// `setup` has written at least, so a settings file an older one wrote is kept to; `setup`
    /// writes 10 s more than it for Claude Code, Codex and Grok, a session's end apart; and there
    /// is none where oboete writes no timeout.
    #[test]
    fn a_hooks_time_is_counted_from_no_more_than_setup_writes() {
        fn secs(agent: &str, event: &str) -> Option<u64> {
            hook_timeout(agent, event).map(|t| t.as_secs())
        }
        fn written(groups: &[(String, Value)], event: &str) -> Option<u64> {
            let group = &groups.iter().find(|(e, _)| e == event)?.1;
            group["hooks"][0]["timeout"].as_u64()
        }
        assert_eq!(secs("claude", "UserPromptSubmit"), Some(5));
        assert_eq!(secs("claude", "SessionStart"), Some(10));
        assert_eq!(secs("claude", "SessionEnd"), Some(3));
        assert_eq!(secs("codex", "PostToolUse"), Some(5));
        assert_eq!(secs("codex", "SessionStart"), Some(10));
        assert_eq!(secs("grok", "SessionStart"), Some(5));
        assert_eq!(secs("agy", "PreInvocation"), Some(10));
        assert_eq!(secs("agy", "PostToolUse"), Some(10));
        assert_eq!(secs("cursor", "UserPromptSubmit"), Some(10));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let (claude, codex, grok) = (claude_groups(&cmd), codex_groups(&cmd), grok_groups(&cmd));
        assert_eq!(written(&claude, "UserPromptSubmit"), Some(15));
        assert_eq!(written(&claude, "PreToolUse"), Some(15));
        assert_eq!(written(&claude, "SessionStart"), Some(20));
        assert_eq!(written(&claude, "SessionEnd"), Some(3));
        assert_eq!(written(&codex, "PostToolUse"), Some(15));
        assert_eq!(written(&codex, "SubagentStart"), Some(20));
        assert_eq!(written(&codex, "SessionEnd"), Some(3));
        assert_eq!(written(&grok, "SessionStart"), Some(15));
        assert_eq!(written(&grok, "SessionEnd"), Some(3));
        // Every hook `setup` writes for the three has both, 10 s apart but at a session's end.
        for (agent, groups) in [("claude", claude), ("codex", codex), ("grok", grok)] {
            assert!(!groups.is_empty());
            for (event, group) in groups {
                let written = group["hooks"][0]["timeout"].as_u64();
                assert!(written.is_some(), "{agent} {event}");
                let room = if event == "SessionEnd" { 0 } else { 10 };
                assert_eq!(
                    secs(agent, &event).map(|s| s + room),
                    written,
                    "{agent} {event}"
                );
            }
        }
        // agy's and Cursor's are the ones written: each agent's own spec is read.
        let agy = agy_spec(&cmd, false).unwrap();
        assert_eq!(agy.as_object().unwrap().len(), 4);
        for (event, hooks) in agy.as_object().unwrap() {
            let written = match event.as_str() {
                "PostToolUse" => hooks[0]["hooks"][0]["timeout"].as_u64(),
                _ => hooks[0]["timeout"].as_u64(),
            };
            assert!(written.is_some(), "agy {event}");
            assert_eq!(secs("agy", event), written, "agy {event}");
        }
        let cursor = cursor_hook_spec(&cmd, false).unwrap();
        for (step, event) in CURSOR_EVENTS {
            let written = cursor[step][0]["timeout"].as_u64();
            assert!(written.is_some(), "cursor {step}");
            assert_eq!(secs("cursor", event), written, "cursor {step}");
        }
        for (agent, event) in [
            ("claude", "Nope"),
            ("agy", "SessionEnd"),
            ("cursor", "beforeSubmitPrompt"),
            ("opencode", "SessionStart"),
            ("pi", "SessionStart"),
            ("aider", "Stop"),
        ] {
            assert_eq!(secs(agent, event), None, "{agent} {event}");
        }
    }

    /// Parity row 50 (CodeRabbit on #618): the delete command is copied as it is, so neither
    /// PowerShell nor a POSIX shell reads anything in the home's path.
    #[test]
    fn the_delete_command_quotes_the_home_for_its_shell() {
        let home = Path::new("/h/it's $x `y` -z");
        assert_eq!(
            delete_command(home, true),
            "Remove-Item -Recurse -Force -LiteralPath '/h/it''s $x `y` -z'"
        );
        assert_eq!(
            delete_command(home, false),
            "rm -rf -- '/h/it'\\''s $x `y` -z'"
        );
        assert_eq!(
            delete_command(Path::new("/home/u/.oboete"), false),
            "rm -rf -- /home/u/.oboete"
        );
    }

    /// docs/unwritten.md U5: a hook whose store does not open waits for it, then for a keep in
    /// progress, and still has time for its file and its marker within the shortest deadline it
    /// is given (Codex on #440).
    #[test]
    fn a_failed_write_and_its_keep_fit_the_shortest_hook_deadline() {
        let shortest = CLAUDE_EVENTS
            .iter()
            .map(|e| e.1)
            .chain(CODEX_EVENTS.iter().map(|e| e.2))
            .chain(GROK_EVENTS.iter().map(|e| e.1))
            .min()
            .unwrap();
        let needed = failed_write_and_keep();
        assert!(needed <= std::time::Duration::from_secs(shortest.into()));
    }

    /// What a hook whose store does not open takes to its end: the store's wait, a keep in
    /// progress, and its own file and marker.
    fn failed_write_and_keep() -> std::time::Duration {
        crate::hook::STORE_WAIT
            + crate::unwritten::KEEP_WAIT
            + std::time::Duration::from_millis(500)
    }

    #[test]
    fn w6_windows_namespace_paths_never_reach_metadata() {
        use std::cell::Cell;
        for path in [
            r"\\server\share\codex.exe",
            "//server/share/grok.cmd",
            r"\\?\UNC\server\share\agent",
            r"\\.\UNC\server\share\agent",
            r"\??\UNC\server\share\agent",
            r"\\?\GLOBALROOT\Device\Mup\server\share",
        ] {
            let calls = Cell::new(0);
            let result: std::io::Result<std::fs::Metadata> =
                diagnostic_path_with(Path::new(path), true, |_, _| {
                    calls.set(calls.get() + 1);
                    Err(std::io::ErrorKind::NotFound.into())
                });
            assert_eq!(calls.get(), 0, "UNC/device path reached metadata");
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
        }
        for path in [r"C:\agents\codex.exe", r"\\?\C:\agents\codex.exe"] {
            let calls = Cell::new(0);
            let result: std::io::Result<std::fs::Metadata> =
                diagnostic_path_with(Path::new(path), true, |_, _| {
                    calls.set(calls.get() + 1);
                    Err(std::io::ErrorKind::NotFound.into())
                });
            assert_eq!(calls.get(), 1);
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        }
    }

    #[test]
    fn w6_windows_parent_reparse_refuses_before_remote_resolution() {
        let remote_resolutions = std::cell::Cell::new(0);
        let result: std::io::Result<()> = diagnostic_path_with(
            Path::new(r"C:\private\link\codex.exe"),
            true,
            |_, attributes| {
                if attributes & 0x1000 != 0 {
                    // OBJ_DONT_REPARSE: decline any parent reparse.
                    Err(std::io::ErrorKind::Unsupported.into())
                } else {
                    remote_resolutions.set(remote_resolutions.get() + 1);
                    Err(std::io::ErrorKind::NotFound.into())
                }
            },
        );
        assert_eq!(
            remote_resolutions.get(),
            0,
            "parent link attempted remote resolution"
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    #[cfg(unix)]
    fn w6a_native_claude_command_is_bounded_and_discards_private_output() {
        use std::time::{Duration, Instant};
        let shell = Path::new("/bin/sh");
        assert_eq!(
            claude_cli_at(
                shell,
                &[
                    "-c",
                    "test \"$OBOETE_SKIP\" = 1 && test -z \"$(cat)\" && printf private-output-canary && printf private-error-canary >&2"
                ],
                Duration::from_secs(1),
            ),
            ClaudeCliExit::Success
        );
        assert_eq!(
            claude_cli_at(shell, &["-c", "exit 23"], Duration::from_secs(1)),
            ClaudeCliExit::Nonzero
        );
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            claude_cli_at(
                &root.path().join("missing-claude"),
                &[],
                Duration::from_secs(1)
            ),
            ClaudeCliExit::SpawnFailed
        );
        let start = Instant::now();
        assert_eq!(
            claude_cli_at(shell, &["-c", "sleep 2"], Duration::from_millis(30)),
            ClaudeCliExit::Unknown
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    #[cfg(unix)]
    fn w6a_first_backup_preserves_a_dangling_existing_backup_alias() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("settings.json");
        let backup = root.path().join("settings.json.oboete.bak");
        let target = root.path().join("must-stay-missing");
        std::fs::write(&source, "original private configuration").unwrap();
        std::os::unix::fs::symlink(&target, &backup).unwrap();
        assert!(
            backup_once(&source).is_err(),
            "a missing recovery copy was accepted"
        );
        assert!(
            !target.exists(),
            "first backup followed and wrote through an existing alias"
        );
        assert!(backup.is_symlink());
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "original private configuration"
        );
        // An empty, readable earlier copy remains valid; retain its alias and bytes.
        std::fs::write(&target, "").unwrap();
        backup_once(&source).unwrap();
        assert!(backup.is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "");
    }

    #[test]
    #[cfg(unix)]
    fn w6a_native_edit_refuses_a_missing_retained_recovery_copy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("config.toml");
        let backup = root.path().join("config.toml.oboete.bak");
        let missing = root.path().join("missing-recovery");
        let original = "[mcp_servers.foreign]\ncommand = 'foreign'\n";
        std::fs::write(&source, original).unwrap();
        std::os::unix::fs::symlink(&missing, &backup).unwrap();
        let command = HookCommand {
            exe: "/tools/oboete".into(),
            home: None,
        };
        assert!(toml_mcp(&source, &command, false).is_err());
        assert_eq!(std::fs::read_to_string(source).unwrap(), original);
        assert!(backup.is_symlink());
        assert!(!missing.exists());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn w6a_failed_first_backup_never_publishes_partial_bytes() {
        let root = tempfile::tempdir().unwrap();
        let backup = root.path().join("settings.json.oboete.bak");
        // This regular proc file reports zero bytes but yields data. The bounded
        // copy must fail its source-consistency check after writing one byte.
        assert!(copy_first_backup(Path::new("/proc/self/cmdline"), &backup).is_err());
        assert!(
            !backup.exists(),
            "failed copy was published as a retained backup"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        let source = root.path().join("settings.json");
        std::fs::write(&source, "complete original").unwrap();
        copy_first_backup(&source, &backup).unwrap();
        assert_eq!(
            std::fs::read_to_string(backup).unwrap(),
            "complete original"
        );
    }

    #[test]
    fn w6a_mcp_payload_is_checked_before_a_destructive_remove() {
        assert!(claude_mcp_arguments_fit(
            Path::new("claude"),
            &json!({"command":"/tools/oboete","args":["mcp"]})
        ));
        assert!(!claude_mcp_arguments_fit(
            Path::new("claude"),
            &json!({"env":{"retained": "x".repeat(70*1024)}})
        ));
    }

    #[test]
    fn one_agent_that_fails_leaves_the_others_wired() {
        let mut wired = Vec::new();
        let (reported, failed) = wire_each(&["claude", "codex", "grok", "pi"], |a| {
            if a == "claude" {
                return Err(anyhow!("settings.json is not JSON"));
            }
            if a == "pi" {
                return Ok(false);
            }
            wired.push(a.to_owned());
            Ok(true)
        });
        assert_eq!(failed, ["claude"]);
        assert_eq!(wired, ["codex", "grok"]);
        assert_eq!(reported, ["codex", "grok"]);
    }

    #[test]
    fn opencode_setup_round_trip_preserves_config_and_other_plugins() {
        let dir =
            std::env::temp_dir().join(format!("oboete-opencode-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        let file = dir.join("opencode.json");
        let original = "{\"theme\":\"dark\",\"mcp\":{\"servers\":{\"other\":{\"type\":\"remote\",\"url\":\"https://example.test\"},\"oboete\":{\"disabled\":true,\"timeout\":42}}}}\n";
        std::fs::write(&file, original).unwrap();
        std::fs::write(dir.join("plugins/other.js"), "export default {};\n").unwrap();
        let cmd = HookCommand {
            exe: "/tools with spaces/oboete".into(),
            home: Some("/memory with spaces".into()),
        };
        opencode_files(&dir, &cmd, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("plugins/oboete.js")).unwrap(),
            opencode_plugin(&cmd).unwrap()
        );
        let root = read_json_object(&file).unwrap();
        assert_eq!(root["theme"], "dark");
        assert_eq!(
            root["mcp"]["servers"]["other"]["url"],
            "https://example.test"
        );
        assert_eq!(
            root["mcp"]["servers"]["oboete"],
            json!({"disabled":true,"timeout":42,"type":"local","command":["/tools with spaces/oboete","--home","/memory with spaces","mcp"]})
        );
        let installed = std::fs::read(&file).unwrap();
        opencode_files(&dir, &cmd, false).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), installed);
        assert_eq!(
            std::fs::read_to_string(dir.join("opencode.json.oboete.bak")).unwrap(),
            original
        );
        opencode_files(&dir, &cmd, true).unwrap();
        assert!(!dir.join("plugins/oboete.js").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("plugins/other.js")).unwrap(),
            "export default {};\n"
        );
        assert_eq!(
            read_json_object(&file).unwrap(),
            json!({"theme":"dark","mcp":{"servers":{"other":{"type":"remote","url":"https://example.test"}}}})
        );
        opencode_files(&dir, &cmd, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("opencode.json.oboete.bak")).unwrap(),
            original
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opencode_existing_plugin_is_backed_up_once_before_replace_or_remove() {
        let cmd = HookCommand {
            exe: "/tools/oboete".into(),
            home: Some("/invented-memory".into()),
        };
        for remove in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let plugin = dir.path().join("plugins/oboete.js");
            let backup = dir.path().join("plugins/oboete.js.oboete.bak");
            std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
            let original = "// existing same-name plugin\nexport default {};\n";
            std::fs::write(&plugin, original).unwrap();
            opencode_files(dir.path(), &cmd, remove).unwrap();
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
            if remove {
                assert!(!plugin.exists());
            } else {
                assert_eq!(
                    std::fs::read_to_string(&plugin).unwrap(),
                    opencode_plugin(&cmd).unwrap()
                );
            }
            // A later replacement/removal must retain the first recoverable copy.
            std::fs::write(&plugin, "// later owner edit\n").unwrap();
            opencode_files(dir.path(), &cmd, remove).unwrap();
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        }
    }

    #[test]
    fn opencode_jsonc_and_invalid_json_are_untouched_while_plugin_is_managed() {
        let dir =
            std::env::temp_dir().join(format!("oboete-opencode-jsonc-{}", std::process::id()));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h space".into()),
        };
        for (name, text) in [
            ("opencode.jsonc", "{ // owner comment\n\"mcp\": {}}\n"),
            // OpenCode merges both files: with a JSONC next to it, the JSON is not ours to edit.
            ("both", "{\"mcp\": {}}\n"),
            ("opencode.json", "{ // owner comment\n\"mcp\": {}}\n"),
            ("opencode.json", "{broken"),
            ("opencode.json", ""),
            ("opencode.json", "[]"),
        ] {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join(if name == "both" {
                "opencode.json"
            } else {
                name
            });
            std::fs::write(&file, text).unwrap();
            if name == "both" {
                std::fs::write(dir.join("opencode.jsonc"), "{}\n").unwrap();
            }
            let (_, status) = opencode_files(&dir, &cmd, false).unwrap();
            assert!(status.contains("left untouched"));
            let command = if cfg!(windows) {
                "opencode mcp add oboete --global -- '/x/oboete' '--home' '/h space' 'mcp'"
            } else {
                "opencode mcp add oboete --global -- /x/oboete --home '/h space' mcp"
            };
            assert!(status.contains(command));
            assert!(dir.join("plugins/oboete.js").is_file());
            assert!(opencode_mcp_status(&dir, &cmd).starts_with("unverified:"));
            opencode_files(&dir, &cmd, true).unwrap();
            assert!(!dir.join("plugins/oboete.js").exists());
            assert_eq!(std::fs::read_to_string(file).unwrap(), text);
            if name.ends_with("jsonc") {
                assert!(!dir.join("opencode.json").exists());
            }
            assert!(!dir.join("opencode.json.oboete.bak").exists());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opencode_doctor_checks_plugin_binary_home_and_disabled_mcp() {
        let dir =
            std::env::temp_dir().join(format!("oboete-opencode-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        assert!(opencode_plugin_status(&dir, &cmd).starts_with("missing"));
        opencode_files(&dir, &cmd, true).unwrap();
        assert!(!dir.exists());
        opencode_files(&dir, &cmd, false).unwrap();
        assert!(opencode_plugin_status(&dir, &cmd).contains("matches current"));
        assert_eq!(opencode_mcp_status(&dir, &cmd), "registered");
        cmd.home = Some("/different home".into());
        assert!(opencode_plugin_status(&dir, &cmd).contains("differs"));
        assert!(opencode_mcp_status(&dir, &cmd).contains("another command"));
        cmd.home = None;
        cmd.exe = "/different/oboete".into();
        assert!(opencode_plugin_status(&dir, &cmd).contains("differs"));
        let file = dir.join("opencode.json");
        let mut root = read_json_object(&file).unwrap();
        root["mcp"]["servers"]["oboete"]["disabled"] = json!(true);
        write_json(&file, &root).unwrap();
        assert!(opencode_mcp_status(&dir, &cmd).contains("turned off"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn opencode_stages_both_files_before_install_or_remove() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("oboete-opencode-locked-{}", std::process::id()));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let other = HookCommand {
            exe: "/new/oboete".into(),
            home: None,
        };
        for remove in [false, true] {
            for locked in ["plugins/oboete.js", "opencode.json"] {
                let _ = std::fs::remove_dir_all(&dir);
                opencode_files(&dir, &cmd, false).unwrap();
                let plugin = dir.join("plugins/oboete.js");
                let mcp = dir.join("opencode.json");
                let before = (
                    std::fs::read(&plugin).unwrap(),
                    std::fs::read(&mcp).unwrap(),
                );
                let target = dir.join(locked);
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();
                assert!(opencode_files(&dir, &other, remove).is_err());
                assert_eq!(std::fs::read(&plugin).unwrap(), before.0);
                assert_eq!(std::fs::read(&mcp).unwrap(), before.1);
                assert!(!dir.join("opencode.json.oboete.bak").exists());
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// MUST-M10: every cell of the status table names tests that exist.
    #[test]
    fn every_claimed_cell_has_its_test() {
        let src = |f: &str| {
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(f))
                .unwrap()
        };
        let tests = (src("hook/tests.rs") + &src("setup/tests.rs")).replace("\r\n", "\n");
        let lines: Vec<&str> = tests.lines().map(str::trim).collect();
        assert_eq!(AGENT_STATUS.map(|(a, _)| a), AGENTS);
        for (agent, cells) in AGENT_STATUS {
            for (point, names) in POINTS.iter().zip(cells) {
                assert!(!names.is_empty(), "{agent} {point}");
                for name in names {
                    let declared = format!("fn {name}() {{");
                    assert!(
                        lines.windows(2).any(|pair| pair == ["#[test]", declared.as_str()]),
                        "{agent} {point}: {name}"
                    );
                }
            }
        }
        assert_eq!(
            status_line("pi"),
            "pi: injection handling at start, resume (existing context reused), compaction, prompt, correction, failure line is \
             implemented and tested, not yet checked in a live session"
        );
    }

    #[test]
    fn opencode_plugin_escapes_paths_and_runs_on_node() {
        let dir = std::env::temp_dir().join(format!("oboete-opencode-node-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("oboete.js");
        let cmd = HookCommand {
            exe: "C:\\Program Files\\quote\"'\\oboete.exe".into(),
            home: Some("C:\\Users\\Jane Doe\\\"memory\"\\back\\slash\nline".into()),
        };
        let plugin = opencode_plugin(&cmd).unwrap();
        let constants: Vec<&str> = plugin.lines().take(2).collect();
        for (line, prefix, expected) in [
            (constants[0], "const exe = ", json!(cmd.exe)),
            (constants[1], "const home = ", json!(cmd.home)),
        ] {
            let literal = line
                .strip_prefix(prefix)
                .unwrap()
                .strip_suffix(';')
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(literal).unwrap(), expected);
        }
        std::fs::write(&file, &plugin).unwrap();
        if on_path("node") {
            // Node versions before syntax detection need an ESM package for a .js file.
            std::fs::write(dir.join("package.json"), "{\"type\":\"module\"}").unwrap();
            let out = std::process::Command::new("node")
                .arg("--check")
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let harness = dir.join("test.mjs");
            std::fs::write(&harness, include_str!("../testdata/opencode/test.mjs")).unwrap();
            for home in [cmd.home.clone(), None] {
                let cmd = HookCommand {
                    exe: cmd.exe.clone(),
                    home,
                };
                std::fs::write(&file, opencode_plugin(&cmd).unwrap()).unwrap();
                let out = std::process::Command::new("node")
                    .arg(&harness)
                    .arg(&file)
                    .arg(&cmd.exe)
                    .args(&cmd.home)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        } else {
            eprintln!("node not on PATH: skipping generated plugin syntax and runtime checks");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A plugin that kills its capture hook lets it live through the store's wait, a keep in
    /// progress and its own file and marker (docs/unwritten.md U2, U5), as the shortest timeout
    /// `setup` writes does. Killed sooner, a hook whose store is held dies before it keeps what
    /// it could not write, and nothing says an event was lost. The numbers are in two
    /// languages: this holds them together.
    #[test]
    fn a_plugin_kills_no_capture_hook_before_it_can_keep_what_it_could_not_write() {
        let least = failed_write_and_keep();
        let ms_after = |text: &str, mark: &str| {
            let at = text
                .find(mark)
                .unwrap_or_else(|| panic!("{mark} not found"))
                + mark.len();
            let digits: String = text[at..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            std::time::Duration::from_millis(digits.parse().unwrap())
        };
        let pi = ms_after(include_str!("../pi.ts"), "const CAPTURE_KILL_MS = ");
        assert!(pi >= least, "Pi's capture hook: {pi:?}");
        // OpenCode's `send` spawns every hook with this timeout.
        let opencode = include_str!("../opencode.js");
        let hook = opencode.find("\"hook\", \"opencode\", event]").unwrap();
        let opencode = ms_after(&opencode[hook..], "timeout: ");
        assert!(opencode >= least, "OpenCode's hook: {opencode:?}");
    }

    #[test]
    fn pi_generated_extension_matches_event_and_tool_contract() {
        let dir = std::env::temp_dir().join(format!("oboete-pi-node-{}", std::process::id()));
        for home in [None, Some("C:\\Memory Store\\\"quoted\" 'home'".into())] {
            let cmd = HookCommand {
                exe: "C:\\Program Files\\oboete \"quoted\" 'x'.exe".into(),
                home,
            };
            pi_files(&dir, &cmd, false).unwrap();
            let args = match &cmd.home {
                Some(home) => vec!["--home", home.as_str()],
                None => Vec::new(),
            };
            let result = std::process::Command::new("node")
                .arg("--experimental-vm-modules")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/pi/test.mjs"))
                .arg(dir.join("extensions/oboete.ts"))
                .arg(&cmd.exe)
                .arg(serde_json::to_string(&args).unwrap())
                .output();
            std::fs::remove_dir_all(&dir).unwrap();
            let output = match result {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("Pi extension harness skipped: node is not installed");
                    return;
                }
                other => other.unwrap(),
            };
            assert!(
                output.status.success(),
                "Pi extension harness failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    #[test]
    fn pi_setup_round_trip_preserves_other_extensions_and_checks_home() {
        assert!(AGENTS.contains(&"pi"));
        let dir = std::env::temp_dir().join(format!("oboete-pi-setup-{}", std::process::id()));
        let file = dir.join("extensions/oboete.ts");
        let other = dir.join("extensions/git-ai.ts");
        let cmd = HookCommand {
            exe: "C:\\Program Files\\oboete \"quoted\" 'x'.exe".into(),
            home: Some("C:\\Memory Store\\\"quoted\" 'home'".into()),
        };
        assert!(pi_files(&dir, &cmd, true).unwrap().is_empty());
        assert!(!dir.exists());
        assert!(pi_extension_status(&file, &cmd).contains("missing"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&other, "export default function gitAi() {}\n").unwrap();
        // An unmarked extension is not ours to remove. Installation preserves a backup.
        let original = "// local extension\n";
        std::fs::write(&file, original).unwrap();
        assert!(pi_files(&dir, &cmd, true).unwrap().is_empty());
        assert_eq!(read_text(&file).unwrap(), original);
        assert!(pi_extension_status(&file, &cmd).contains("stale"));
        assert_eq!(pi_files(&dir, &cmd, false).unwrap().len(), 1);
        let generated = read_text(&file).unwrap();
        let constants: Vec<Value> = generated
            .lines()
            .filter_map(|line| line.strip_prefix("const OBOETE_"))
            .map(|line| {
                serde_json::from_str(line.split_once(" = ").unwrap().1.trim_end_matches(';'))
                    .unwrap()
            })
            .collect();
        assert_eq!(constants, [json!(cmd.exe), json!(["--home", cmd.home])]);
        assert_eq!(
            pi_extension_status(&file, &cmd),
            "extension wired (current binary and home)"
        );
        assert!(pi_files(&dir, &cmd, false).unwrap().is_empty());
        let other_home = HookCommand {
            exe: cmd.exe.clone(),
            home: None,
        };
        assert!(pi_extension_status(&file, &other_home).contains("stale"));
        let other_exe = HookCommand {
            exe: "/new/oboete".into(),
            home: cmd.home.clone(),
        };
        assert!(pi_extension_status(&file, &other_exe).contains("stale"));
        pi_files(&dir, &other_home, false).unwrap();
        assert!(
            read_text(&file)
                .unwrap()
                .contains("const OBOETE_ARGS = [];")
        );
        let backup = file.with_file_name("oboete.ts.oboete.bak");
        assert_eq!(read_text(&backup).unwrap(), original);
        assert_eq!(pi_files(&dir, &cmd, true).unwrap().len(), 1);
        assert!(!file.exists());
        assert_eq!(
            read_text(&other).unwrap(),
            "export default function gitAi() {}\n"
        );
        assert_eq!(read_text(&backup).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_removes_owned_commands_using_the_shared_ownership_rule() {
        let own = json!({"command":"/old/oboete --home /old-home hook claude Stop"});
        assert!(has_ours(&json!({"hooks":[own]})));
        let other = json!({"command":"echo 'oboete hook cursor Stop'"});
        let mut root = json!({"version":1,"hooks":{"afterAgentResponse":[own,other],"custom":[]}});
        cursor_merge_hooks(&mut root, &json!({}), true).unwrap();
        assert_eq!(
            root,
            json!({"version":1,"hooks":{"afterAgentResponse":[other],"custom":[]}})
        );
    }

    #[test]
    fn cursor_empty_config_environment_values_fall_back_like_the_bundle() {
        let home = Path::new("/home/test");
        assert_eq!(
            cursor_dir_from(Some(Path::new("")), Some(Path::new("/xdg")), home),
            Path::new("/xdg/cursor")
        );
        assert_eq!(
            cursor_dir_from(Some(Path::new(" \t")), Some(Path::new("")), home),
            Path::new("/home/test/.cursor")
        );
    }

    #[test]
    fn cursor_round_trip_preserves_other_hooks_and_servers() {
        let dir = std::env::temp_dir().join(format!("oboete-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("hooks.json");
        let mcp = dir.join("mcp.json");
        let original_hooks = json!({"version":2,"other":"keep","hooks":{
            "preToolUse":[{"command":"git-ai pre"}],
            "postToolUse":[{"command":"git-ai post","timeout":3}],
            "sessionEnd":[], "custom":[]
        }});
        let original_mcp = json!({"other":"keep","mcpServers":{
            "other":{"command":"x"},
            "oboete":{"command":"/old/oboete","args":["mcp"],"env":{"KEEP":"1"},"disabled":true}
        }});
        write_json(&hooks, &original_hooks).unwrap();
        write_json(&mcp, &original_mcp).unwrap();
        let original_hook_bytes = std::fs::read(&hooks).unwrap();
        let original_mcp_bytes = std::fs::read(&mcp).unwrap();
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h".into()),
        };
        cursor_files(&dir, &cmd, false, false).unwrap();
        let h = read_json_object(&hooks).unwrap();
        let m = read_json_object(&mcp).unwrap();
        assert_eq!(h["version"], 2);
        assert_eq!(h["other"], "keep");
        assert_eq!(
            h["hooks"]["preToolUse"],
            original_hooks["hooks"]["preToolUse"]
        );
        assert_eq!(
            h["hooks"]["postToolUse"][0],
            original_hooks["hooks"]["postToolUse"][0]
        );
        assert_eq!(h["hooks"]["sessionEnd"].as_array().unwrap().len(), 1);
        assert_eq!(h["hooks"]["custom"], json!([]));
        assert_eq!(cursor_hooks_status(&hooks, &cmd, false), "7 hooks wired");
        assert_eq!(
            m["mcpServers"]["other"],
            original_mcp["mcpServers"]["other"]
        );
        assert_eq!(m["mcpServers"]["oboete"]["env"], json!({"KEEP":"1"}));
        assert_eq!(m["mcpServers"]["oboete"]["disabled"], true);
        assert_eq!(mcp_command(&mcp), Some((cmd.exe.clone(), cmd.mcp_args())));
        // Doctor reads the kept `disabled: true` as turned off, not as registered.
        assert!(mcp_disabled(&mcp));
        let first_h = std::fs::read(&hooks).unwrap();
        let first_m = std::fs::read(&mcp).unwrap();
        cursor_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(std::fs::read(&hooks).unwrap(), first_h);
        assert_eq!(std::fs::read(&mcp).unwrap(), first_m);
        cursor_files(&dir, &cmd, true, false).unwrap();
        let mut expected_hooks = original_hooks;
        expected_hooks["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("sessionEnd");
        assert_eq!(read_json_object(&hooks).unwrap(), expected_hooks);
        let mut expected_mcp = original_mcp;
        expected_mcp["mcpServers"]
            .as_object_mut()
            .unwrap()
            .remove("oboete");
        assert_eq!(read_json_object(&mcp).unwrap(), expected_mcp);
        assert_eq!(
            cursor_hooks_status(&hooks, &cmd, false),
            "not wired (run `oboete setup cursor`)"
        );
        assert_eq!(
            std::fs::read(hooks.with_file_name("hooks.json.oboete.bak")).unwrap(),
            original_hook_bytes
        );
        assert_eq!(
            std::fs::read(mcp.with_file_name("mcp.json.oboete.bak")).unwrap(),
            original_mcp_bytes
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_bad_json_leaves_each_file_untouched_independently() {
        let dir = std::env::temp_dir().join(format!("oboete-cursor-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("hooks.json");
        let mcp = dir.join("mcp.json");
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let comments = b"{\n  // keep this\n  \"hooks\": {}\n}\n";
        std::fs::write(&hooks, comments).unwrap();
        cursor_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(std::fs::read(&hooks).unwrap(), comments);
        assert_eq!(mcp_command(&mcp), Some((cmd.exe.clone(), cmd.mcp_args())));
        std::fs::remove_file(&mcp).unwrap();
        std::fs::write(&hooks, "{}\n").unwrap();
        let mcp_comments = b"{/* keep MCP comment */\"mcpServers\":{}}\n";
        std::fs::write(&mcp, mcp_comments).unwrap();
        cursor_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(cursor_hooks_status(&hooks, &cmd, false), "7 hooks wired");
        assert_eq!(std::fs::read(&mcp).unwrap(), mcp_comments);
        std::fs::write(&hooks, b" \n").unwrap();
        cursor_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(std::fs::read(&hooks).unwrap(), b" \n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_status_and_windows_command() {
        let dir = std::env::temp_dir().join(format!("oboete-cursor-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("hooks.json");
        let cmd = HookCommand {
            exe: r"C:\tools\oboete.exe".into(),
            home: Some(r"C:\data\oboete".into()),
        };
        assert_eq!(
            cmd.cursor_line("SessionStart", true).unwrap(),
            r#"& "C:\tools\oboete.exe" --home "C:\data\oboete" hook cursor SessionStart"#
        );
        assert_eq!(
            cursor_hooks_status(&file, &cmd, true),
            "not wired (run `oboete setup cursor`)"
        );
        let mut root = json!({"version":1,"hooks":{"sessionStart":[{"command":cmd.cursor_line("SessionStart", true).unwrap(),"timeout":10}]}});
        write_json(&file, &root).unwrap();
        assert_eq!(
            cursor_hooks_status(&file, &cmd, true),
            "partly wired: 1/7 hooks"
        );
        root["hooks"]["sessionStart"][0]["command"] =
            json!("& \"C:/old/oboete.exe\" hook cursor SessionStart");
        write_json(&file, &root).unwrap();
        assert!(cursor_hooks_status(&file, &cmd, true).contains("stale"));
        assert!(cmd.cursor_line("Stop", true).is_ok());
        let bad = HookCommand {
            exe: "C:/Program Files/oboete.exe".into(),
            home: None,
        };
        assert!(
            bad.cursor_line("Stop", true)
                .unwrap_err()
                .to_string()
                .contains("space-free")
        );
        let bad = HookCommand {
            exe: "C:/tools/$env:NAME/oboete.exe".into(),
            home: None,
        };
        assert!(bad.cursor_line("Stop", true).is_err());
        // Removal only identifies installed entries; it does not need a runnable current path.
        cursor_files(&dir, &bad, true, true).unwrap();
        assert_eq!(
            read_json_object(&file).unwrap(),
            json!({"version":1,"hooks":{}})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_config_precedence_and_availability() {
        let home = Path::new("/home/test");
        assert_eq!(
            cursor_dir_from(Some(Path::new("/a")), Some(Path::new("/b")), home),
            Path::new("/a")
        );
        assert_eq!(
            cursor_dir_from(None, Some(Path::new("/b")), home),
            Path::new("/b/cursor")
        );
        assert_eq!(
            cursor_dir_from(None, None, home),
            Path::new("/home/test/.cursor")
        );
        assert!(!cursor_available(
            Path::new("/this/does/not/exist"),
            false,
            false
        ));
        assert!(cursor_available(
            Path::new("/this/does/not/exist"),
            true,
            false
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cursor_stages_both_files_before_replacing_either() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("oboete-cursor-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("hooks.json");
        let mcp = dir.join("mcp.json");
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        for locked in [&hooks, &mcp] {
            std::fs::write(&hooks, "{}\n").unwrap();
            std::fs::write(&mcp, "{}\n").unwrap();
            std::fs::set_permissions(locked, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(cursor_files(&dir, &cmd, false, false).is_err());
            assert_eq!(std::fs::read(&hooks).unwrap(), b"{}\n");
            assert_eq!(std::fs::read(&mcp).unwrap(), b"{}\n");
            std::fs::set_permissions(locked, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_setup_round_trip_preserves_other_entries() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("config/hooks.json");
        let mcp = dir.join("config/mcp_config.json");
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(&hooks, "{\"other\":{\"Stop\":[]}}\n").unwrap();
        std::fs::write(&mcp, "{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}\n").unwrap();
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h".into()),
        };
        agy_files(&dir, &cmd, false, false).unwrap();
        let h = read_json_object(&hooks).unwrap();
        assert_eq!(h["other"], json!({"Stop": []}));
        assert!(h["oboete"].get("PreToolUse").is_none());
        assert_eq!(
            h["oboete"]["SessionStart"][0],
            json!({"type":"command","command":"/x/oboete --home /h hook agy SessionStart","timeout":10})
        );
        assert_eq!(h["oboete"]["PreInvocation"][0]["timeout"], 10);
        assert_eq!(h["oboete"]["Stop"][0]["timeout"], 10);
        assert_eq!(h["oboete"]["PostToolUse"][0]["matcher"], "*");
        assert_eq!(
            h["oboete"]["PostToolUse"][0]["hooks"][0]["command"],
            "/x/oboete --home /h hook agy PostToolUse"
        );
        let m = read_json_object(&mcp).unwrap();
        assert_eq!(m["mcpServers"]["other"], json!({"command":"x"}));
        assert_eq!(
            m["mcpServers"]["oboete"],
            json!({"command":"/x/oboete","args":["--home","/h","mcp"]})
        );
        let first_hooks = std::fs::read(&hooks).unwrap();
        let first_mcp = std::fs::read(&mcp).unwrap();
        agy_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(std::fs::read(&hooks).unwrap(), first_hooks);
        assert_eq!(std::fs::read(&mcp).unwrap(), first_mcp);
        assert_eq!(
            std::fs::read_to_string(hooks.with_file_name("hooks.json.oboete.bak")).unwrap(),
            "{\"other\":{\"Stop\":[]}}\n"
        );
        assert_eq!(
            std::fs::read_to_string(mcp.with_file_name("mcp_config.json.oboete.bak")).unwrap(),
            "{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}\n"
        );
        agy_files(&dir, &cmd, true, false).unwrap();
        assert_eq!(
            read_json_object(&hooks).unwrap(),
            json!({"other":{"Stop":[]}})
        );
        assert_eq!(
            read_json_object(&mcp).unwrap(),
            json!({"mcpServers":{"other":{"command":"x"}}})
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agy_remove_refuses_read_only_owned_files_before_touching_either_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("oboete-agy-locked-{}", std::process::id()));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        for locked in ["hooks.json", "mcp_config.json"] {
            let _ = std::fs::remove_dir_all(&dir);
            agy_files(&dir, &cmd, false, false).unwrap();
            let hooks = dir.join("config/hooks.json");
            let mcp = dir.join("config/mcp_config.json");
            let before_hooks = std::fs::read(&hooks).unwrap();
            let before_mcp = std::fs::read(&mcp).unwrap();
            let target = if locked == "hooks.json" { &hooks } else { &mcp };
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(agy_files(&dir, &cmd, true, false).is_err());
            assert_eq!(std::fs::read(&hooks).unwrap(), before_hooks);
            assert_eq!(std::fs::read(&mcp).unwrap(), before_mcp);
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_fresh_install_removes_to_empty_objects_and_missing_install_skips() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!agy_available(&dir, false));
        assert!(agy_available(&dir, true));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let written = agy_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(written.len(), 2);
        agy_files(&dir, &cmd, true, false).unwrap();
        assert_eq!(
            read_json_object(&dir.join("config/hooks.json")).unwrap(),
            json!({})
        );
        assert_eq!(
            read_json_object(&dir.join("config/mcp_config.json")).unwrap(),
            json!({"mcpServers": {}})
        );
        assert!(agy_files(&dir, &cmd, true, false).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_windows_command_is_raw_and_rejects_spaces_before_writing() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cmd = HookCommand {
            exe: "C:/tools/oboete.exe".into(),
            home: Some("C:/data/oboete".into()),
        };
        agy_files(&dir, &cmd, false, true).unwrap();
        let hooks = read_json_object(&dir.join("config/hooks.json")).unwrap();
        assert_eq!(
            hooks["oboete"]["Stop"][0]["command"],
            "C:/tools/oboete.exe --home C:/data/oboete hook agy Stop"
        );
        let bad = HookCommand {
            exe: "C:/Program Files/oboete.exe".into(),
            home: None,
        };
        assert!(
            agy_files(&dir, &bad, false, true)
                .unwrap_err()
                .to_string()
                .contains("space-free")
        );
        assert_eq!(
            read_json_object(&dir.join("config/hooks.json")).unwrap(),
            hooks
        );
        let bad_home = HookCommand {
            exe: "C:/tools/oboete.exe".into(),
            home: Some("C:/My Data".into()),
        };
        assert!(agy_files(&dir, &bad_home, false, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_doctor_checks_expected_hooks_and_disabled_mcp() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        agy_files(&dir, &cmd, false, false).unwrap();
        let hooks = dir.join("config/hooks.json");
        let mcp = dir.join("config/mcp_config.json");
        assert_eq!(agy_hooks_status(&hooks, &cmd, false), "4 hooks wired");
        assert!(!mcp_disabled(&mcp));
        let mut root = read_json_object(&hooks).unwrap();
        root["oboete"]["PreInvocation"][0]["timeout"] = json!(10000);
        write_json(&hooks, &root).unwrap();
        assert!(agy_hooks_status(&hooks, &cmd, false).contains("rerun"));
        let mut root = read_json_object(&mcp).unwrap();
        root["mcpServers"]["oboete"]["disabled"] = json!(true);
        write_json(&mcp, &root).unwrap();
        assert!(mcp_disabled(&mcp));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rows Codex 0.155.1 itself wrote into the owner's config.toml for these two handlers.
    #[test]
    fn codex_trust_hash_matches_rows_codex_wrote() {
        let h = json!({"type": "command", "command": "node /home/jura/.codex/hooks/caveman.cjs", "timeout": 5, "statusMessage": "Loading caveman mode"});
        assert_eq!(
            codex_trust_hash("SessionStart", Some("startup|resume|clear|compact"), &h),
            "sha256:85139b0f4a7d3c7548afa35ac8377be0343cfb9b8657fe3854f5fd62d8241972"
        );
        let h = json!({"type": "command", "command": "node /home/jura/.codex/hooks/caveman.cjs", "timeout": 5, "statusMessage": "Tracking caveman mode"});
        assert_eq!(
            codex_trust_hash("UserPromptSubmit", None, &h),
            "sha256:d3b11f4d6ea1ff2411c3bc9953d9cac72e4f0cde061a95ffadb488139516289e"
        );
    }

    #[test]
    fn ownership_is_the_exact_hook_command_shape() {
        let ours = |c: &str| is_our_handler(&json!({"type": "command", "command": c}));
        assert!(ours("/x/oboete hook claude Stop"));
        assert!(ours("/x/oboete --home /h hook codex SessionStart"));
        assert!(ours(
            "'/my dir/it'\\''s/oboete' --home '/h o/me' hook grok PostToolUse"
        ));
        assert!(ours("C:/Users/x/.cargo/bin/oboete.exe hook claude Stop"));
        assert!(ours("/opt/oboete-0.1 hook claude Stop"));
        assert!(!ours("python /tools/oboete_report.py hook audit"));
        assert!(!ours("/x/oboete hook audit Stop"));
        assert!(!ours("/x/oboete observe"));
        assert!(!ours("/x/oboete hook claude"));
        assert!(!ours("/x/my-oboete hook claude Stop"));
        assert_eq!(
            shell_words("a 'b c' 'd'\\''e'  f"),
            vec!["a", "b c", "d'e", "f"]
        );
        let cmd = HookCommand {
            exe: "/my dir/oboete".into(),
            home: Some("/h o/me".into()),
        };
        assert!(ours(&cmd.line("codex", "Stop")));
    }

    #[test]
    fn mcp_registrations_round_trip_and_leave_other_servers() {
        let dir = std::env::temp_dir().join(format!("oboete-mcp-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h".into()),
        };
        let registered = |f: &Path| mcp_command(f).map(|(c, _)| c);
        assert_eq!(
            {
                let f = dir.join("args.toml");
                std::fs::write(
                    &f,
                    "[mcp_servers.oboete]\ncommand = \"/x/oboete\"\nargs = [\"mcp\"]\n",
                )
                .unwrap();
                mcp_command(&f)
            },
            Some(("/x/oboete".to_string(), vec!["mcp".to_string()]))
        );
        // TOML (Codex, Grok): other servers and sections survive, ours comes and goes.
        let toml = dir.join("config.toml");
        std::fs::write(&toml, "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n\n[hooks.state.\"k\"]\ntrusted_hash = \"h\"\n").unwrap();
        toml_mcp(&toml, &cmd, false).unwrap();
        let text = std::fs::read_to_string(&toml).unwrap();
        assert!(text.contains("[mcp_servers.oboete]\ncommand = \"/x/oboete\"\nargs = [\"--home\", \"/h\", \"mcp\"]"), "{text}");
        assert!(
            text.contains("[mcp_servers.other]")
                && text.contains("model = \"x\"")
                && text.contains("[hooks.state.\"k\"]")
        );
        assert!(!text.contains("\n[mcp_servers]\n"), "{text}");
        assert_eq!(registered(&toml).as_deref(), Some("/x/oboete"));
        toml_mcp(&toml, &cmd, true).unwrap();
        let text = std::fs::read_to_string(&toml).unwrap();
        assert!(
            !text.contains("oboete") && text.contains("[mcp_servers.other]"),
            "{text}"
        );
        assert!(mcp_command(&toml).is_none());
        // An empty file gains only our table; removing it leaves the file without mcp_servers.
        let fresh = dir.join("fresh.toml");
        toml_mcp(&fresh, &cmd, false).unwrap();
        assert_eq!(registered(&fresh).as_deref(), Some("/x/oboete"));
        toml_mcp(&fresh, &cmd, true).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap().trim(), "");
        // A rerun sets only command/args: keys the developer put on our entry stay.
        let own = dir.join("own.toml");
        std::fs::write(&own, "[mcp_servers.oboete]\ncommand = \"/old/oboete\"\nargs = [\"mcp\"]\nenabled = false\nstartup_timeout_sec = 60\n").unwrap();
        toml_mcp(&own, &cmd, false).unwrap();
        assert!(mcp_disabled(&own) && !mcp_disabled(&toml));
        // Removing from a config that does not exist creates nothing.
        let absent = dir.join("absent").join("config.toml");
        toml_mcp(&absent, &cmd, true).unwrap();
        assert!(!absent.exists() && !dir.join("absent").exists());
        let text = std::fs::read_to_string(&own).unwrap();
        assert!(
            text.contains("command = \"/x/oboete\"")
                && text.contains("enabled = false")
                && text.contains("startup_timeout_sec = 60"),
            "{text}"
        );
        // An explicit [mcp_servers] header keeps its comment; the inline form is edited in place.
        let header = dir.join("header.toml");
        std::fs::write(
            &header,
            "# my servers\n[mcp_servers]\n\n[mcp_servers.a]\ncommand = \"x\"\n",
        )
        .unwrap();
        toml_mcp(&header, &cmd, false).unwrap();
        let text = std::fs::read_to_string(&header).unwrap();
        assert!(text.starts_with("# my servers\n[mcp_servers]\n"), "{text}");
        let inline = dir.join("inline.toml");
        std::fs::write(&inline, "mcp_servers = { other = { command = \"o\" } }\n").unwrap();
        toml_mcp(&inline, &cmd, false).unwrap();
        assert_eq!(registered(&inline).as_deref(), Some("/x/oboete"));
        assert!(std::fs::read_to_string(&inline).unwrap().contains("other"));
        toml_mcp(&inline, &cmd, true).unwrap();
        assert!(mcp_command(&inline).is_none());
        // Remove keeps an explicit [mcp_servers] header; a dotted prefix goes with our entry.
        std::fs::write(&header, "# my servers\n[mcp_servers]\n").unwrap();
        toml_mcp(&header, &cmd, false).unwrap();
        toml_mcp(&header, &cmd, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&header).unwrap(),
            "# my servers\n[mcp_servers]\n"
        );
        let dotted = dir.join("dotted.toml");
        std::fs::write(&dotted, "a = 1\nmcp_servers.oboete.command = \"/old\"\n").unwrap();
        toml_mcp(&dotted, &cmd, true).unwrap();
        assert_eq!(std::fs::read_to_string(&dotted).unwrap(), "a = 1\n");
        // A file that cannot be read stops setup and stays as it was.
        let latin = dir.join("latin.toml");
        std::fs::write(&latin, b"# caf\xe9\n[cli]\nfoo = 1\n").unwrap();
        assert!(toml_mcp(&latin, &cmd, false).is_err());
        assert_eq!(
            std::fs::read(&latin).unwrap(),
            b"# caf\xe9\n[cli]\nfoo = 1\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("oboete-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "{\"a\": 1}\n").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\": 1}\n");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&real), 0o640);
        let new = dir.join("sub").join("new.json");
        write_atomic(&new, "{}\n").unwrap();
        assert_eq!(mode(&new), 0o600);
        assert_eq!(std::fs::read_dir(dir.join("sub")).unwrap().count(), 1);
        let chain = dir.join("chain.json");
        std::os::unix::fs::symlink("dangling.json", &chain).unwrap();
        // A link whose target does not exist yet: the target is created, the link stays.
        let dangling = dir.join("dangling.json");
        std::os::unix::fs::symlink("sub/later.json", &dangling).unwrap();
        write_atomic(&dangling, "{}\n").unwrap();
        assert!(
            dangling
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sub/later.json")).unwrap(),
            "{}\n"
        );
        // Through a chain of links, too.
        std::fs::remove_file(dir.join("sub/later.json")).unwrap();
        write_atomic(&chain, "[]\n").unwrap();
        assert!(
            dangling
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sub/later.json")).unwrap(),
            "[]\n"
        );
        // A staged file that is never committed leaves nothing behind.
        drop(stage(&dir.join("sub/never.json"), "{}").unwrap());
        assert_eq!(std::fs::read_dir(dir.join("sub")).unwrap().count(), 2);
        // A file locked read-only is not replaced.
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(write_atomic(&real, "{}").is_err());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\": 1}\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// X5 F1: Claude Code's reads get oboete's `PreToolUse` hook, beside the user's own.
    #[test]
    fn claude_gets_a_pre_tool_use_hook_for_its_reads_beside_the_users() {
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let user = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]});
        let mut root = json!({"hooks": {"PreToolUse": [user.clone()]}});
        merge_groups(&mut root, claude_groups(&cmd));
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!((pre.len(), &pre[0]), (2, &user));
        assert_eq!(pre[1]["matcher"], CLAUDE_READS);
        let command = pre[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.ends_with("hook claude PreToolUse"), "{command}");
        let others = claude_groups(&cmd);
        assert!(
            others
                .iter()
                .all(|(e, g)| e == "PreToolUse" || g.get("matcher").is_none())
        );
    }

    /// X5 F1 (G25): Codex's reads get oboete's `PreToolUse` hook, its shell and MCP read tools
    /// matched as claude-mem matches them, after the user's own, with the row that trusts it.
    #[test]
    fn codex_gets_a_pre_tool_use_hook_for_its_reads_beside_the_users() {
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let user =
            json!({"matcher": "^Bash$", "hooks": [{"type": "command", "command": "echo hi"}]});
        let mut root = json!({"hooks": {"PreToolUse": [user.clone()]}});
        let file = Path::new("/h/hooks.json");
        let before = codex_trust_keys(file, &root);
        merge_groups(&mut root, codex_groups(&cmd));
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!((pre.len(), &pre[0]), (2, &user));
        assert_eq!(pre[1]["matcher"], CODEX_READS);
        let handler = &pre[1]["hooks"][0];
        let command = handler["command"].as_str().unwrap();
        assert!(command.ends_with("hook codex PreToolUse"), "{command}");
        assert_eq!(handler["additionalContextLimit"], 0);
        let delta = codex_trust_delta(&before, &codex_trust_keys(file, &root));
        assert!(delta.moved.is_empty(), "{delta:?}");
        let row = (
            "/h/hooks.json:pre_tool_use:1:0".to_owned(),
            codex_trust_hash("PreToolUse", Some(CODEX_READS), handler),
        );
        assert!(delta.fresh.contains(&row), "{delta:?}");
        // claude-mem 13.39: a subagent thread's next tool call after a compaction takes the
        // manifest back, whole.
        let post = &root["hooks"]["PostToolUse"][0]["hooks"][0];
        assert_eq!(post["additionalContextLimit"], 0);
    }

    /// X17, X26 (docs/subagents.md): Claude Code's SubagentStop and Codex's SubagentStart are
    /// wired as claude-mem wires them, with no matcher; Codex's gives its context whole, with the
    /// row that trusts it.
    #[test]
    fn the_subagent_hooks_are_wired() {
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let mut root = json!({});
        merge_groups(&mut root, claude_groups(&cmd));
        let stop = root["hooks"]["SubagentStop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert!(stop[0].get("matcher").is_none());
        let command = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.ends_with("hook claude SubagentStop"), "{command}");
        let mut root = json!({});
        merge_groups(&mut root, codex_groups(&cmd));
        let start = root["hooks"]["SubagentStart"].as_array().unwrap();
        assert_eq!(start.len(), 1);
        assert!(start[0].get("matcher").is_none());
        let handler = &start[0]["hooks"][0];
        let command = handler["command"].as_str().unwrap();
        assert!(command.ends_with("hook codex SubagentStart"), "{command}");
        assert_eq!(handler["additionalContextLimit"], 0);
        let rows = codex_trust_keys(Path::new("/h/hooks.json"), &root);
        let row = rows
            .iter()
            .find(|r| r.key == "/h/hooks.json:subagent_start:0:0")
            .unwrap();
        assert!(row.ours);
        assert_eq!(row.hash, codex_trust_hash("SubagentStart", None, handler));
    }

    /// Decision 108 (docs/hook-time.md): `setup` gives the hooks an older one wrote their new
    /// timeouts and changes nothing else, the developer's own handler keeps its timeout, and
    /// Codex's trust rows are written for the handlers as they now are.
    #[test]
    fn an_older_setups_hooks_get_the_new_timeouts() {
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let user = json!({"hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]});
        // A file as `setup` wrote it until decision 108: 5 s, a session's start 10 s, its end 3 s.
        let older = |groups: Vec<(String, Value)>| {
            let mut root = json!({"hooks": {"Stop": [user.clone()]}});
            merge_groups(&mut root, groups);
            for (event, groups) in root["hooks"].as_object_mut().unwrap() {
                let secs = match event.as_str() {
                    "SessionStart" | "SubagentStart" => 10,
                    "SessionEnd" => 3,
                    _ => 5,
                };
                for group in groups.as_array_mut().unwrap() {
                    if has_ours(group) {
                        group["hooks"][0]["timeout"] = json!(secs);
                    }
                }
            }
            root
        };
        fn without_our_timeouts(mut root: Value) -> Value {
            for groups in root["hooks"].as_object_mut().unwrap().values_mut() {
                for group in groups.as_array_mut().unwrap() {
                    for handler in group["hooks"].as_array_mut().unwrap() {
                        if is_our_handler(handler) {
                            handler.as_object_mut().unwrap().remove("timeout");
                        }
                    }
                }
            }
            root
        }

        let mut root = older(claude_groups(&cmd));
        let before = root.clone();
        merge_groups(&mut root, claude_groups(&cmd));
        let hooks = &root["hooks"];
        assert_eq!(hooks["Stop"][0], user);
        assert_eq!(hooks["Stop"][1]["hooks"][0]["timeout"], 15);
        assert_eq!(hooks["UserPromptSubmit"][0]["hooks"][0]["timeout"], 15);
        assert_eq!(hooks["SessionStart"][0]["hooks"][0]["timeout"], 20);
        assert_eq!(hooks["SessionEnd"][0]["hooks"][0]["timeout"], 3);
        assert_ne!(root, before);
        assert_eq!(without_our_timeouts(root), without_our_timeouts(before));

        let file = Path::new("/h/hooks.json");
        let mut root = older(codex_groups(&cmd));
        let before = codex_trust_keys(file, &root);
        merge_groups(&mut root, codex_groups(&cmd));
        let delta = codex_trust_delta(&before, &codex_trust_keys(file, &root));
        assert!(
            delta.stale.is_empty() && delta.moved.is_empty(),
            "{delta:?}"
        );
        // A row for each of ours and none for the developer's: a new hash where the timeout is
        // new, the session's end's as it was.
        assert_eq!(delta.fresh.len(), CODEX_EVENTS.len());
        for (key, hash) in &delta.fresh {
            let old = before.iter().find(|row| row.key == *key).unwrap();
            assert!(old.ours, "{key}");
            assert_eq!(*hash == old.hash, key.contains(":session_end:"), "{key}");
        }
    }

    /// Decision 108: doctor says which of an agent's hooks are not as this binary's `setup`
    /// writes them (an older one's timeouts), and how to put that right; hooks that name another
    /// binary are said as before, and first.
    #[test]
    fn doctor_names_the_hooks_an_older_setup_wrote() {
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        type Groups = fn(&HookCommand) -> Vec<(String, Value)>;
        let agents: [(&str, Groups); 3] = [
            ("claude", claude_groups),
            ("codex", codex_groups),
            ("grok", grok_groups),
        ];
        // The same binary at a home that is not the default one, and another binary.
        let homed = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h o".into()),
        };
        let other = HookCommand {
            exe: "/y/oboete".into(),
            home: None,
        };
        for (agent, groups) in agents {
            let wanted = groups(&cmd);
            let n = wanted.len();
            let status =
                |root: &Value, by: &HookCommand| hooks_status(root, agent, by, &groups(by));
            let user = json!({"hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]});
            let mut root = json!({"hooks": {"Stop": [user]}});
            assert_eq!(status(&root, &cmd), "not wired (run `oboete setup`)");
            merge_groups(&mut root, wanted.clone());
            assert_eq!(status(&root, &cmd), format!("{n} hooks wired"));
            // Two of them with the timeout `setup` wrote until decision 108.
            let older = |root: &mut Value| {
                root["hooks"]["UserPromptSubmit"][0]["hooks"][0]["timeout"] = json!(5);
                root["hooks"]["PostToolUse"][0]["hooks"][0]["timeout"] = json!(5);
            };
            older(&mut root);
            assert_eq!(
                status(&root, &cmd),
                format!(
                    "{n} hooks wired, 2 not as this version writes them (rerun `oboete setup {agent}`)"
                )
            );
            assert_eq!(
                status(&root, &other),
                format!("{n} hooks, {n} point at another binary (rerun `oboete setup`)")
            );
            // Hooks wired for another home are not this home's to judge: wired, as they were.
            assert_eq!(status(&root, &homed), format!("{n} hooks wired"));
            merge_groups(&mut root, wanted.clone());
            assert_eq!(status(&root, &cmd), format!("{n} hooks wired"));
            assert_eq!(status(&root, &homed), format!("{n} hooks wired"));
            // A group the developer's own handler shares with ours is ours as written, whichever
            // of the two comes first: the settings page reads it so (`readiness`).
            for first in [true, false] {
                let mut shared = root.clone();
                let handlers = shared["hooks"]["UserPromptSubmit"][0]["hooks"]
                    .as_array_mut()
                    .unwrap();
                let theirs = json!({"type": "command", "command": "echo hi"});
                handlers.insert(if first { 0 } else { 1 }, theirs);
                assert_eq!(status(&shared, &cmd), format!("{n} hooks wired"), "{first}");
                older(&mut shared);
                let ours = usize::from(first);
                shared["hooks"]["UserPromptSubmit"][0]["hooks"][ours]["timeout"] = json!(5);
                assert_eq!(
                    status(&shared, &cmd),
                    format!(
                        "{n} hooks wired, 2 not as this version writes them (rerun `oboete setup {agent}`)"
                    ),
                    "{first}"
                );
            }
            // A home that is not the default one is named in the command: without it `setup`
            // would wire the agent to the default home.
            let mut root = json!({});
            merge_groups(&mut root, groups(&homed));
            assert_eq!(status(&root, &homed), format!("{n} hooks wired"));
            older(&mut root);
            assert_eq!(
                status(&root, &homed),
                format!(
                    "{n} hooks wired, 2 not as this version writes them (rerun `oboete setup {agent} --home '/h o'`)"
                )
            );
            assert_eq!(status(&root, &cmd), format!("{n} hooks wired"));
        }
    }

    #[test]
    fn merge_keeps_user_groups_and_is_idempotent() {
        let user = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]});
        let mut root = json!({"env": {"A": "1"}, "hooks": {"PreToolUse": [user.clone()], "Stop": [user.clone()]}});
        let ours = || {
            vec![(
                "Stop".to_string(),
                json!({"hooks": [{"type": "command", "command": "/x/oboete hook claude Stop"}]}),
            )]
        };
        merge_groups(&mut root, ours());
        merge_groups(&mut root, ours());
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(root["hooks"]["Stop"][0], user);
        assert_eq!(root["hooks"]["PreToolUse"][0], user);
        merge_groups(&mut root, vec![]);
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["env"]["A"], "1");

        // A group shared with the developer's handler keeps that handler in place.
        let mixed = json!({"matcher": "Bash", "hooks": [
            {"type": "command", "command": "/x/oboete hook claude Stop"},
            {"type": "command", "command": "echo hi"}
        ]});
        let mut root = json!({"hooks": {"Stop": [mixed]}});
        merge_groups(&mut root, ours());
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(
            stop[0]["hooks"],
            json!([{"type": "command", "command": "echo hi"}])
        );
        assert_eq!(stop[0]["matcher"], "Bash");
        assert!(has_ours(&stop[1]));
        merge_groups(&mut root, vec![]);
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(!has_ours(&root["hooks"]["Stop"][0]));
    }

    #[test]
    fn codex_trust_rows_are_written_as_tables_and_removed() {
        let text = "[features]\nmemories = true\n\n[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n\n[mcp_servers.x]\ncommand = \"y\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        let fresh = vec![(
            "/h/hooks.json:session_start:1:0".to_string(),
            "sha256:bb".to_string(),
        )];
        codex_write_trust(
            &mut doc,
            &TrustDelta {
                fresh: fresh.clone(),
                ..Default::default()
            },
        );
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:session_start:1:0\"]\ntrusted_hash = \"sha256:bb\""
            ),
            "{out}"
        );
        assert!(out.contains("[hooks.state.\"/h/hooks.json:stop:0:0\"]"));
        assert!(out.contains("[mcp_servers.x]"));
        codex_write_trust(
            &mut doc,
            &TrustDelta {
                stale: vec![fresh[0].0.clone()],
                ..Default::default()
            },
        );
        let out = doc.to_string();
        assert!(!out.contains("session_start:1:0"), "{out}");
        assert!(out.contains("[features]\nmemories = true"));
    }

    #[test]
    fn trust_rows_follow_user_groups_that_shift() {
        let file = Path::new("/h/hooks.json");
        let ours = json!({"hooks": [{"type": "command", "command": "/x/oboete hook codex Stop", "timeout": 5}]});
        let user = json!({"hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]});
        let keys = |groups: Vec<Value>| codex_trust_keys(file, &json!({"hooks": {"Stop": groups}}));
        let before = keys(vec![ours.clone(), user.clone()]);
        let toml = "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:ours-old\"\n\n[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"sha256:user\"\n";
        let k = |g: usize| format!("/h/hooks.json:stop:{g}:0");
        let kh = |g: usize, h: usize| format!("/h/hooks.json:stop:{g}:{h}");

        // Rerun: ours is re-appended after the user's group, which moves from 1 to 0.
        let d = codex_trust_delta(&before, &keys(vec![user.clone(), ours.clone()]));
        assert_eq!(d.stale, vec![k(0)]);
        assert_eq!(d.moved, vec![(k(1), k(0))]);
        assert_eq!(d.fresh.len(), 1);
        assert_eq!(d.fresh[0].0, k(1));
        let mut doc: toml_edit::DocumentMut = toml.parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:user\""
            ),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"{}\"",
                d.fresh[0].1
            )),
            "{out}"
        );
        assert!(!out.contains("ours-old"));

        // Remove: the user's group moves from 1 to 0 and keeps its row; ours is gone.
        let d = codex_trust_delta(&before, &keys(vec![user.clone()]));
        assert_eq!(d.stale, vec![k(0)]);
        assert_eq!(d.moved, vec![(k(1), k(0))]);
        assert!(d.fresh.is_empty());
        let mut doc: toml_edit::DocumentMut = toml.parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:user\""
            ),
            "{out}"
        );
        assert!(
            !out.contains("stop:1:0") && !out.contains("ours-old"),
            "{out}"
        );

        // Nothing shifts when ours already sits last.
        let last = keys(vec![user.clone(), ours.clone()]);
        let d = codex_trust_delta(&last, &last);
        assert!(d.stale.is_empty() && d.moved.is_empty() && d.fresh.len() == 1);

        // A handler of the developer's inside our group moves from 0:1 to 0:0 when ours leaves.
        let shared = json!({"hooks": [
            {"type": "command", "command": "/x/oboete hook codex Stop", "timeout": 5},
            {"type": "command", "command": "echo hi", "timeout": 5}
        ]});
        let d = codex_trust_delta(&keys(vec![shared]), &keys(vec![user.clone(), ours.clone()]));
        assert_eq!(d.stale, vec![kh(0, 0)]);
        assert_eq!(d.moved, vec![(kh(0, 1), kh(0, 0))]);
        assert_eq!(d.fresh[0].0, kh(1, 0));

        // Two identical user handlers (same hash) each keep their own row, by position.
        let twins = json!({"hooks": [
            {"type": "command", "command": "echo hi", "timeout": 5},
            {"type": "command", "command": "echo hi", "timeout": 5}
        ]});
        let same = keys(vec![twins.clone()]);
        assert_eq!(codex_trust_delta(&same, &same), TrustDelta::default());
        let d = codex_trust_delta(&keys(vec![ours.clone(), twins.clone()]), &same);
        assert_eq!(d.moved, vec![(kh(1, 0), kh(0, 0)), (kh(1, 1), kh(0, 1))]);
        let mut doc: toml_edit::DocumentMut = "[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"sha256:t0\"\n\n[hooks.state.\"/h/hooks.json:stop:1:1\"]\ntrusted_hash = \"sha256:t1\"\n".parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains("stop:0:0\"]\ntrusted_hash = \"sha256:t0\"")
                && out.contains("stop:0:1\"]\ntrusted_hash = \"sha256:t1\"")
                && !out.contains("stop:1:"),
            "{out}"
        );
    }

    /// Spec 7.2 (milestone 4 Task 10): each choice says what leaves the machine and is answered
    /// before anything changes; then only `[embedding] provider` changes, once the choice is
    /// ready. A refusal, a missing account or token, a file off its pin, or a build without the
    /// model leaves config.toml as it was.
    #[test]
    fn setup_embeddings() {
        use crate::model_fetch::{Artifact, Runtime};
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        let pin = |b: &[u8]| leak(format!("{:x}", Sha256::digest(b)));
        // Nobody listens there: a file still to download fails.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            leak(format!("http://{}/model", listener.local_addr().unwrap()))
        };
        let model = [Artifact {
            name: "tokenizer.json",
            url: closed,
            size: 5,
            sha256: pin(b"model"),
        }];
        let runtime = Runtime {
            archive: Artifact {
                name: "onnxruntime/rt.tgz",
                url: closed,
                size: 1,
                sha256: pin(b"a"),
            },
            member: "rt/lib",
            library: Artifact {
                name: "onnxruntime/librt",
                url: "",
                size: 7,
                sha256: pin(b"runtime"),
            },
        };
        let local = crate::embed::local_unavailable().is_none();
        // (choice, what the home holds, the answer, what the line says, written or the error)
        type Row<'a> = (&'a str, &'a str, bool, &'a str, Result<(), &'a str>);
        let rows: &[Row] = &[
            ("none", "account", true, "no text or query goes", Ok(())),
            ("none", "account files", true, "1 MB); delete", Ok(())),
            ("none", "account part", true, "2 MB); delete", Ok(())),
            ("workers-ai", "key", true, "", Err("account_id")),
            ("workers-ai", "account", true, "", Err("key file")),
            ("workers-ai", "account key", true, "Cloudflare", Ok(())),
            ("workers-ai", "account key", false, "acct", Err("nothing")),
            ("local", "", false, "1 MB from 127.0.0.1", Err("nothing")),
            ("local", "files", true, "once each is held", Ok(())),
            ("local", "verified", true, "checked when they were", Ok(())),
            // A byte off its pin: downloaded again, which fails here.
            ("local", "changed", true, "once each is held", Err("")),
        ];
        for &(choice, holds, answer, said, outcome) in rows {
            let home = tempfile::tempdir().unwrap();
            let key_file = home.path().join("key.md");
            if holds.contains("key") {
                std::fs::write(&key_file, "workers ai\ntoken\n").unwrap();
            }
            let account = holds.contains("account");
            let was = if account { "workers-ai" } else { "none" };
            let before = format!(
                "# the owner's notes\n[summary]\ncurate = true\n\n[embedding]\n# by hand\n\
                 provider = \"{was}\" # first\n{}key_file = '{}'\ndaily_requests = 150\n",
                if account {
                    "account_id = \"acct\"\n"
                } else {
                    ""
                },
                key_file.display()
            );
            std::fs::write(home.path().join("config.toml"), &before).unwrap();
            let dir = crate::embed::local_dir(home.path());
            let tokenizer: &[u8] = if holds == "changed" {
                b"modem"
            } else {
                b"model"
            };
            if holds.contains("files") || holds == "changed" || holds == "verified" {
                std::fs::create_dir_all(dir.join("onnxruntime")).unwrap();
                std::fs::write(dir.join("tokenizer.json"), tokenizer).unwrap();
                std::fs::write(dir.join("onnxruntime/librt"), "runtime").unwrap();
            }
            if holds.contains("part") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("tokenizer.json.part"), vec![0u8; 2 << 20]).unwrap();
            }
            if holds == "verified" {
                let files: Vec<_> = model.iter().copied().chain([runtime.library]).collect();
                crate::model_fetch::verify(&dir, &files).unwrap();
            }
            let mut line = None;
            let got = embeddings_with(home.path(), choice, &model, Some(&runtime), |l| {
                line = Some(l.to_owned());
                anyhow::ensure!(answer, "nothing changed");
                Ok(())
            });
            let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
            let row = format!("{choice} {holds:?} {answer}: {got:?} {line:?}");
            if choice == "local" && !local {
                // A build without the model is refused before the question.
                let error = format!("{:#}", got.unwrap_err());
                assert!(
                    error.contains("local embeddings") || error.contains("ONNX"),
                    "{row}"
                );
                assert_eq!((line, after), (None, before), "{row}");
                continue;
            }
            match outcome {
                Ok(()) => {
                    got.unwrap();
                    let now = before.replace(
                        &format!("provider = \"{was}\""),
                        &format!("provider = \"{choice}\""),
                    );
                    assert_eq!(after, now, "{row}");
                }
                Err(error) => {
                    assert!(format!("{:#}", got.unwrap_err()).contains(error), "{row}");
                    assert_eq!(after, before, "{row}");
                }
            }
            match said {
                "" => assert_eq!(line, None, "{row}"),
                said => assert!(line.as_deref().is_some_and(|l| l.contains(said)), "{row}"),
            }
            if choice == "local" {
                let files: Vec<_> = model.iter().copied().chain([runtime.library]).collect();
                let verified = crate::model_fetch::marker_ok(&dir, &files);
                assert_eq!(verified, outcome.is_ok(), "{row}");
            }
        }
    }

    /// Spec 7.2: a choice needs a terminal's "y" or `--yes`.
    #[test]
    fn setup_asks_in_a_terminal_or_takes_yes() {
        let answer = |yes, terminal, typed: &str| {
            confirm(yes, terminal, &mut std::io::Cursor::new(typed.as_bytes())).is_ok()
        };
        assert!(answer(true, false, ""));
        assert!(!answer(false, false, "y\n"));
        assert!(answer(false, true, "y\n"));
        assert!(!answer(false, true, "n\n"));
        assert!(!answer(false, true, ""));
    }
