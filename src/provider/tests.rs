    use super::*;

    #[test]
    fn answers_of_another_shape_do_not_count_as_success() {
        let schema = crate::curate::schema();
        let claim = |kind: &str| {
            serde_json::json!({"id": "c1", "kind": kind, "status": "decided", "speaker": "user",
                "scope": "repo", "body": "b", "quote": "q", "line": "L1", "supersedes": [],
                "why": ""})
        };
        let ok = serde_json::json!({"claims": [claim("decision")], "summary": "s",
            "observations": []});
        assert!(fits(&ok, &schema));
        // Without its cards an answer is not of the shape; the curator's own check takes it
        // (`curate::check`), as it takes a line id written as a number.
        let no_cards = serde_json::json!({"claims": [claim("decision")], "summary": "s"});
        assert!(!fits(&no_cards, &schema));
        // Valid JSON, wrong keys: what a free model without strict schema support returned.
        let other = serde_json::json!({"issue": "x", "resolution": "y", "decision": "z"});
        assert!(!fits(&other, &schema));
        let mut item_missing_body = claim("decision");
        item_missing_body.as_object_mut().unwrap().remove("body");
        let item_missing_body = serde_json::json!({"claims": [item_missing_body], "summary": "s"});
        assert!(!fits(&item_missing_body, &schema));
        let summary_not_text = serde_json::json!({"claims": [], "summary": 3});
        assert!(!fits(&summary_not_text, &schema));
        // Kinds outside the enum are mapped later (the claims consumer), not refused here.
        let odd_kind = serde_json::json!({"claims": [claim("Decision")], "summary": "",
            "observations": []});
        assert!(fits(&odd_kind, &schema));
    }

    #[test]
    fn the_codex_curator_gets_no_user_config_mcp_or_home() {
        let scratch = scratch_dir().unwrap();
        let (cmd, _) =
            headless_command("codex", None, &scratch.0, CURATOR_SYSTEM, "p", "{}").unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let web_search_off = r#"web_search="disabled""#;
        for flag in [
            "--ignore-user-config",
            "--ignore-rules",
            CODEX_PROFILE,
            web_search_off,
        ]
        .into_iter()
        .chain(CODEX_OFF)
        {
            assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        // --sandbox would switch codex back to its older settings and ignore the profile.
        assert!(!args.iter().any(|a| a == "--sandbox"), "{args:?}");
    }

    /// Decision 86: a fenced prompt's instructions go into the CLI's own system prompt (claude's
    /// `--system-prompt-file`, codex's `model_instructions_file`), which the CLI's prompt cache
    /// holds from call to call; the turn on stdin starts at the sentence that names the fence, its
    /// `@` read as before. An unfenced prompt goes whole into the turn, the system prompt as it was.
    #[test]
    fn a_fenced_prompts_instructions_go_into_the_cli_system_prompt() {
        let scratch = scratch_dir().unwrap();
        let sentence = crate::curate::FENCE_SENTENCE;
        let fenced = format!(
            "Extract the claims.\n{sentence}=== RECORD a ===`.\n\n=== RECORD a ===\nL1 a@b\n=== RECORD a ==="
        );
        let at = fenced.find(sentence).unwrap();
        // Decision 107: a three-part prompt's instructions end at its stretch's fence sentence too,
        // so they are the system prompt, the same for every window, and the stretch and the tail
        // are the turn.
        let window = "## claude session s\nL1 [user] a@b\n";
        let parts = crate::curate::prompt_parts("English", window, "", "", 5, 1);
        let parted = parts.find(sentence).unwrap();
        let other = "## codex session t\nL1 [user] Other.\nL2 [assistant] Yes.\n";
        let other = crate::curate::prompt_parts("English", other, "u1: kept", "", 15, 2);
        assert_eq!(parts[..parted], other[..other.find(sentence).unwrap()]);
        assert!(!parts[..parted].contains("=== ") && !parts[..parted].contains("a@b"));
        assert!(parts[parted..].contains(window) && parts[parted..].contains("=== KNOWN "));
        let file_after = |cmd: &Command, flag: &str| -> String {
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let path = if flag == "-c" {
                let value = args
                    .iter()
                    .find(|a| a.starts_with("model_instructions_file="))
                    .unwrap();
                let table: toml::Table = toml::from_str(value).unwrap();
                table["model_instructions_file"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            } else {
                let i = args.iter().position(|a| a.as_str() == flag).unwrap();
                args[i + 1].clone()
            };
            std::fs::read_to_string(path).unwrap()
        };
        for (n, (prompt, instructions, turn)) in [
            (fenced.as_str(), Some(&fenced[..at]), &fenced[at..]),
            (parts.as_str(), Some(&parts[..parted]), &parts[parted..]),
            ("p", None, "p"),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = scratch.0.join(format!("claude {n}"));
            std::fs::create_dir(&dir).unwrap();
            let (cmd, stdin) =
                headless_command("claude", None, &dir, CURATOR_SYSTEM, prompt, "{}").unwrap();
            let system = format!("{CURATOR_SYSTEM} The JSON schema: {{}}");
            let system = match instructions {
                Some(i) => format!("{system}\n\n{i}"),
                None => system,
            };
            assert_eq!(file_after(&cmd, "--system-prompt-file"), system);
            assert_eq!(
                stdin.as_deref(),
                Some(turn.replace('@', "\u{FF20}").as_str())
            );
            let dir = scratch.0.join(format!("codex {n}"));
            std::fs::create_dir(&dir).unwrap();
            let (cmd, stdin) =
                headless_command("codex", None, &dir, CURATOR_SYSTEM, prompt, "{}").unwrap();
            let system = match instructions {
                Some(i) => format!("{CURATOR_SYSTEM}\n\n{i}"),
                None => CURATOR_SYSTEM.to_owned(),
            };
            assert_eq!(file_after(&cmd, "-c"), system);
            assert_eq!(stdin.as_deref(), Some(turn));
        }
    }

    /// Decision 86: when codex's instructions file cannot be named (a path that is not UTF-8), codex
    /// keeps its own instructions and the fenced prompt's stay in the turn, so it still reads them.
    #[cfg(target_os = "linux")]
    #[test]
    fn codex_keeps_the_instructions_in_the_turn_when_its_file_cannot_be_named() {
        use std::os::unix::ffi::OsStrExt;
        let scratch = scratch_dir().unwrap();
        let dir = scratch
            .0
            .join(std::ffi::OsStr::from_bytes(b"not utf-8 \xff"));
        std::fs::create_dir(&dir).unwrap();
        let fenced = format!(
            "Extract the claims.\n{}=== RECORD a ===`.\n\n=== RECORD a ===\nL1\n=== RECORD a ===",
            crate::curate::FENCE_SENTENCE
        );
        let (cmd, stdin) =
            headless_command("codex", None, &dir, CURATOR_SYSTEM, &fenced, "{}").unwrap();
        assert!(
            !cmd.get_args()
                .any(|a| a.to_string_lossy().starts_with("model_instructions_file=")),
            "{cmd:?}"
        );
        assert_eq!(stdin.as_deref(), Some(fenced.as_str()));
    }

    /// Each role's CLI entry is told what it is: a transcript's curator, or My Work's jobs, which
    /// read requests, summaries and work state (docs/my-work-model.md A10).
    #[test]
    fn the_subscription_curators_replace_the_cli_system_prompt() {
        let scratch = scratch_dir().unwrap();
        for (role, expected) in [
            ("curator", CURATOR_SYSTEM),
            ("summary", CURATOR_SYSTEM),
            (crate::my_work::ROLE, MY_WORK_SYSTEM),
        ] {
            // A path TOML must escape: the codex -c value has to stay one valid key = string.
            let name = if cfg!(unix) { "a\"b\\c" } else { "a b" };
            let dir = scratch.0.join(format!("{role} {name}"));
            std::fs::create_dir(&dir).unwrap();
            let args = |cli| -> Vec<String> {
                let (cmd, _) =
                    headless_command(cli, None, &dir, system_for(role), "p", "{}").unwrap();
                cmd.get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect()
            };
            let claude = args("claude");
            let at = claude
                .iter()
                .position(|a| a == "--system-prompt-file")
                .unwrap();
            assert_eq!(Path::new(&claude[at + 1]), dir.join("system.md"));
            let system = std::fs::read_to_string(&claude[at + 1]).unwrap();
            assert_eq!(
                system,
                format!("{expected} The JSON schema: {{}}"),
                "{role}"
            );
            let codex = args("codex");
            let value = codex
                .iter()
                .find(|a| a.starts_with("model_instructions_file="))
                .unwrap();
            let table: toml::Table = toml::from_str(value).unwrap();
            let file = table["model_instructions_file"].as_str().unwrap();
            assert_eq!(Path::new(file), dir.join("instructions.md"));
            assert_eq!(std::fs::read_to_string(file).unwrap(), expected, "{role}");
        }
    }

    /// docs/my-work-model.md A9: an answer a My Work check refuses rests no entry and counts toward
    /// no breaker; the same refusal for windows counts (D11).
    #[test]
    fn an_answer_my_works_check_refuses_counts_toward_no_breaker() {
        let content = json!({"choices": [{"message": {"content": "{\"groups\": 1}"}}]});
        let check = |_: &Value| Some("shape");
        for (role, skip, fails) in [
            (crate::my_work::ROLE, Skip::Refused, 0),
            ("curator", Skip::Failed, 1),
        ] {
            let home = tempfile::tempdir().unwrap();
            let conn = crate::providers_db::open(home.path()).unwrap();
            let (url, _) = serve_once(content.to_string().into_bytes(), "");
            let Err(err) = Chain::new(&[stub(url)], &conn).check(&check).run(
                role,
                "s",
                "p",
                &json!({"type": "object"}),
            ) else {
                panic!("{role}: a refused answer was accepted");
            };
            let ChainFailed(fallbacks) = err.downcast::<ChainFailed>().unwrap();
            assert_eq!(fallbacks[0].skip, skip, "{role}");
            let state = crate::providers_db::state(&conn, "stub").unwrap();
            assert_eq!(state.fails, fails, "{role}");
        }
    }

    #[test]
    fn claude_runs_with_spec_6_5s_layers_and_no_schema_tool() {
        let scratch = scratch_dir().unwrap();
        let (cmd, _) = headless_command(
            "claude",
            Some("haiku"),
            &scratch.0,
            CURATOR_SYSTEM,
            "p",
            "{}",
        )
        .unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let has = |pair: [&str; 2]| args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        for pair in [
            ["--output-format", "stream-json"],
            ["--permission-mode", "dontAsk"],
            ["--permission-prompts", "none"],
            ["--tools", ""],
            ["--max-turns", "1"],
            ["--settings", CLAUDE_SETTINGS],
        ] {
            assert!(has(pair), "{pair:?}: {args:?}");
        }
        let settings: Value = serde_json::from_str(CLAUDE_SETTINGS).unwrap();
        assert_eq!(settings["disableAllHooks"], true);
        assert_eq!(settings["alwaysThinkingEnabled"], false);
        for builtin in [
            "agents-md@builtin",
            "telemetry@builtin",
            "cc-plugin-plugin-authoring@builtin",
            "cc-plugin-privacy@builtin",
        ] {
            assert_eq!(settings["enabledPlugins"][builtin], false, "{builtin}");
        }
        for flag in [
            "--verbose",
            "--disable-slash-commands",
            "--strict-mcp-config",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        let at = args.iter().position(|a| a == "--disallowedTools").unwrap();
        assert_eq!(args[at + 1..], ["Agent", "Task", "Monitor", "mcp__*"]);
        // --json-schema would hand claude a StructuredOutput tool.
        assert!(!args.iter().any(|a| a == "--json-schema"), "{args:?}");
        let (_, stdin) = headless_command(
            "claude",
            None,
            &scratch.0,
            CURATOR_SYSTEM,
            "look at @/etc/hostname",
            "{}",
        )
        .unwrap();
        assert_eq!(stdin.unwrap(), "look at \u{FF20}/etc/hostname");
    }

    #[test]
    fn the_curator_environment_is_the_allow_list() {
        let os = |k: &str, v: &str| (std::ffi::OsString::from(k), std::ffi::OsString::from(v));
        let parent = [
            os("PATH", "/bin"),
            os("HOME", "/h"),
            os("LANG", "ja_JP.UTF-8"),
            os("LC_ALL", "C"),
            os("XDG_CONFIG_HOME", "/h/.config"),
            os("https_proxy", "http://p:1"),
            os("CODEX_HOME", "/h/.codex"),
            os("ANTHROPIC_BASE_URL", "http://evil"),
            os("ANTHROPIC_API_KEY", "k"),
            os("CLAUDE_CODE_EFFORT_LEVEL", "max"),
            os("AUTHORIZATION", "Bearer x"),
            os("google_application_credentials", "/h/k.json"),
            os("SSH_AUTH_SOCK", "/tmp/agent"),
            os("GROQ_TOKEN", "t"),
            os("Path", "C:\\bin"),
        ];
        let names = |windows| -> Vec<String> {
            curator_env(parent.clone().into_iter(), windows)
                .into_iter()
                .map(|(k, _)| k.into_string().unwrap())
                .collect()
        };
        let want = [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "XDG_CONFIG_HOME",
            "https_proxy",
            "CODEX_HOME",
        ];
        assert_eq!(names(false), want);
        // Windows compares names without case: its `Path` is PATH.
        assert_eq!(names(true), [&want[..], &["Path"]].concat());
    }

    /// claude's stream: the init event, an answer, the rate-limit event and the result.
    fn claude_events(init: Value, rate: &str, result: Value) -> String {
        [
            init,
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "{}"}]}}),
            json!({"type": "rate_limit_event", "rate_limit_info":
                {"status": rate, "resetsAt": 1_790_744_400_i64, "rateLimitType": "seven_day"}}),
            result,
        ]
        .map(|v| v.to_string())
        .join("\n")
    }

    fn clean_init() -> Value {
        json!({"type": "system", "subtype": "init", "tools": [], "mcp_servers": [],
            "plugins": [], "permissionMode": "dontAsk", "apiKeySource": "none"})
    }

    fn result_of(text: &str) -> Value {
        json!({"type": "result", "subtype": "success", "is_error": false, "result": text,
            "usage": {"input_tokens": 9, "cache_creation_input_tokens": 100, "output_tokens": 50}})
    }

    #[test]
    fn claudes_answer_is_read_from_its_stream() {
        let out = claude_events(
            clean_init(),
            "allowed",
            result_of("```json\n{\"summary\": \"s\"}\n```"),
        );
        let result = claude_stream(&out).unwrap();
        assert_eq!(claude_rest(&out), None);
        assert_eq!(
            extract_structured("claude", &result).unwrap(),
            json!({"summary": "s"})
        );
        assert_eq!(usage_cli("claude", &out).prompt, Some(109));
    }

    #[test]
    fn a_tool_in_system_init_discards_the_answer() {
        for (k, v) in [
            ("tools", json!(["Bash"])),
            ("mcp_servers", json!([{"name": "github"}])),
            ("plugins", json!([{"name": "agents-md"}])),
            ("permissionMode", json!("bypassPermissions")),
            ("apiKeySource", json!("ANTHROPIC_API_KEY")),
        ] {
            let mut init = clean_init();
            init[k] = v;
            let out = claude_events(init, "allowed", result_of("{}"));
            let e = claude_stream(&out).expect_err(k);
            assert!(e.message.contains("isolation"), "{k}: {}", e.message);
            assert!(!e.invalid(), "an isolation failure rests claude: {k}");
        }
        // A turn that used a tool, whatever init said.
        let used = [
            clean_init(),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash"}]}}),
            result_of("{}"),
        ]
        .map(|v| v.to_string())
        .join("\n");
        assert!(claude_stream(&used).is_err());
    }

    /// Issue #166: codex rests at claude's lines, read from its app server's windows; a reached
    /// limit or no included usage rests it until the latest reset, or until the owner acts.
    #[test]
    fn a_reset_is_read_in_seconds_or_milliseconds() {
        let ms = 1_790_744_400_000_i64;
        assert_eq!(reset_ms(&json!(1_790_744_400_i64)), Some(ms));
        assert_eq!(reset_ms(&json!(ms)), Some(ms));
        assert_eq!(reset_ms(&json!(1_790_744_400.5)), Some(ms + 500));
        assert_eq!(reset_ms(&json!(null)), None);
        assert_eq!(reset_ms(&json!("1790744400")), None);
    }

    #[test]
    fn codex_rests_at_the_lines_its_app_server_reports() {
        let now = db::now_ms();
        let (later, soon) = (now / 1000 + 86_400, now / 1000 + 600);
        let read = |windows: Value| json!({"rateLimits": windows});
        let window = |used: i64, mins: i64, reset: i64| json!({"usedPercent": used, "windowDurationMins": mins, "resetsAt": reset});
        let week = |used| read(json!({"primary": window(used, 10080, later)}));
        // The owner's week on 2026-09-28: 47%.
        assert_eq!(codex_rest(&week(47), now, false), None);
        assert_eq!(codex_rest(&week(92), now, false), None);
        assert_eq!(codex_rest(&week(93), now, false), Some(later * 1000));
        let hours = |used, reset| read(json!({"primary": window(used, 300, reset)}));
        assert_eq!(codex_rest(&hours(94, later), now, false), None);
        assert_eq!(
            codex_rest(&hours(95, later), now, false),
            Some(later * 1000)
        );
        assert_eq!(codex_rest(&hours(85, soon), now, false), Some(soon * 1000));
        assert_eq!(codex_rest(&hours(84, soon), now, false), None);
        // Every limit counts, and the later reset holds.
        let both = json!({"rateLimitsByLimitId": {
            "codex": {"primary": window(10, 10080, later)},
            "other": {"primary": window(10, 300, soon), "secondary": window(96, 10080, later + 60)},
        }});
        assert_eq!(codex_rest(&both, now, false), Some((later + 60) * 1000));
        let reached = json!({"rateLimits": {"primary": window(40, 10080, later),
            "rateLimitReachedType": "rate_limit_reached"}});
        assert_eq!(codex_rest(&reached, now, false), Some(later * 1000));
        let workspace = json!({"rateLimits": {"rateLimitReachedType": "workspace_member_usage_limit_reached",
            "individualLimit": {"limit": "10", "used": "10", "remainingPercent": 0, "resetsAt": later}}});
        assert_eq!(codex_rest(&workspace, now, false), Some(later * 1000));
        let none_left = json!({"ordinaryUsageAllowed": false, "rateLimits": {}});
        assert_eq!(
            codex_rest(&none_left, now, false),
            Some(providers_db::OWNER_HOLD)
        );
        // A reached limit with no reset holds codex, whatever reset another limit gives.
        let unknown = json!({"rateLimitsByLimitId": {
            "codex": {"primary": {"usedPercent": 100}, "rateLimitReachedType": "rate_limit_reached"},
            "other": {"primary": window(10, 300, soon)},
        }});
        assert_eq!(
            codex_rest(&unknown, now, false),
            Some(providers_db::OWNER_HOLD)
        );
        assert_eq!(codex_rest(&json!({}), now, false), None);
        // At its line with no reset: an hour, then codex is read again.
        let unset = read(json!({"primary": {"usedPercent": 97, "windowDurationMins": 10080}}));
        let hour = REST_WITHOUT_RESET.as_millis() as i64;
        assert_eq!(codex_rest(&unset, now, false), Some(now + hour));
    }

    /// Owner decision 41 (#164): with `credits`, a limit whose account has credits to draw on
    /// rests nothing, at its line or past it, as codex's own client reads the credits; a
    /// workspace's spend control, a workspace limit and an account without credits still rest.
    #[test]
    fn codex_draws_on_credits_past_its_lines_when_allowed() {
        let now = db::now_ms();
        let later = now / 1000 + 86_400;
        let limit = |more: Value| {
            let mut s = json!({"primary": {"usedPercent": 100, "windowDurationMins": 10080,
                "resetsAt": later}, "rateLimitReachedType": "rate_limit_reached",
                "credits": {"hasCredits": true, "unlimited": false, "balance": "4"}});
            s.as_object_mut()
                .unwrap()
                .extend(more.as_object().unwrap().clone());
            json!({"rateLimits": s})
        };
        let reached = limit(json!({}));
        assert_eq!(codex_rest(&reached, now, false), Some(later * 1000));
        assert_eq!(codex_rest(&reached, now, true), None);
        let line = limit(
            json!({"primary": {"usedPercent": 96, "windowDurationMins": 10080,
            "resetsAt": later}, "rateLimitReachedType": null}),
        );
        assert_eq!(codex_rest(&line, now, true), None);
        let unlimited = limit(json!({"credits": {"hasCredits": false, "unlimited": true}}));
        assert_eq!(codex_rest(&unlimited, now, true), None);
        for still in [
            json!({"credits": {"hasCredits": false, "unlimited": false, "balance": "0"}}),
            json!({"credits": null}),
            json!({"spendControlReached": true}),
            json!({"rateLimitReachedType": "workspace_member_credits_depleted"}),
            json!({"rateLimitReachedType": "workspace_owner_usage_limit_reached"}),
        ] {
            assert_eq!(
                codex_rest(&limit(still.clone()), now, true),
                Some(later * 1000),
                "{still}"
            );
        }
        // No included usage left: credits carry the call when every limit has them.
        let mut none_left = limit(json!({}));
        none_left["ordinaryUsageAllowed"] = json!(false);
        assert_eq!(codex_rest(&none_left, now, true), None);
        // One limit without credits is enough to rest.
        let both = json!({"rateLimitsByLimitId": {
            "codex": reached["rateLimits"].clone(),
            "other": limit(json!({"credits": {"hasCredits": false}}))["rateLimits"].clone(),
        }});
        assert_eq!(codex_rest(&both, now, true), Some(later * 1000));
        // An additional limit, with no credits and no reached type as codex reads it, does not
        // take over the main limit's `ordinaryUsageAllowed` (Codex on #418).
        let additional = json!({
            "ordinaryUsageAllowed": false,
            "rateLimits": reached["rateLimits"].clone(),
            "rateLimitsByLimitId": {
                "codex": reached["rateLimits"].clone(),
                "other": {"primary": {"usedPercent": 10, "windowDurationMins": 10080,
                    "resetsAt": later}, "rateLimitReachedType": null, "credits": null},
            },
        });
        assert_eq!(codex_rest(&additional, now, true), None);
        assert_eq!(codex_rest(&additional, now, false), Some(later * 1000));
    }

    /// A rest codex's allowance set before a call is its own cooldown, to the millisecond: no
    /// outage cooldown on top, and no failure counted toward the breaker.
    #[test]
    fn a_rest_before_the_call_is_only_the_rest() {
        let soon = db::now_ms() + 120_000;
        let e = CallError::other("codex is at its plan's usage line")
            .unsent()
            .resting(Some(soon));
        let was = providers_db::State {
            fails: 1,
            ..Default::default()
        };
        let s = next_state(was, &e);
        assert_eq!((s.down_until, s.fails), (soon, 1));
    }

    /// #164: an entry's `credits` reaches codex's allowance read, which draws on them only then.
    #[test]
    fn a_codex_entrys_credits_reach_its_allowance_read() {
        use std::cell::Cell;
        use std::rc::Rc;
        for credits in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let until = db::now_ms() + 60_000;
            let seen = Rc::new(Cell::new(None));
            let saw = Rc::clone(&seen);
            CODEX_REST_TEST.with(|probe| {
                *probe.borrow_mut() = Some(Box::new(move |c| {
                    saw.set(Some(c));
                    Some(until)
                }))
            });
            let providers = [Provider::Cli {
                credits,
                enabled: true,
                name: "codex".into(),
                cli: "codex".into(),
                model: None,
                daily_budget: 10,
                timeout_s: 1,
                limits: Default::default(),
            }];
            let gate = || Ok(Some(crate::dispatch::Admission::shared(home.path())?));
            let isolation = || Ok(crate::isolation::Gate::Passed);
            let mut chain = Chain::new(&providers, &conn).gate(&gate);
            chain.isolation = Some(&isolation);
            chain.forced_fail = None;
            OFFLINE_CALL_TEST.with(|call| {
                *call.borrow_mut() =
                    Some(CallError::other("synthetic offline dispatch refusal").unsent())
            });
            let result = chain.run("curator", "preflight", "synthetic prompt", &json!({}));
            CODEX_REST_TEST.with(|probe| probe.borrow_mut().take());
            OFFLINE_CALL_TEST.with(|call| call.borrow_mut().take());
            assert!(result.is_err());
            assert_eq!(seen.get(), Some(credits));
        }
    }

    /// A rest codex's allowance read set without counting credits is marked so, and does not
    /// hold an entry that may draw on them, however it came to (the page, config.toml, another
    /// entry of the name): it reads the allowance again. A rest read with them holds (Codex on
    /// #418).
    /// Only codex draws on credits: another CLI's entry that says `credits` in config.toml is held
    /// by an uncredited rest all the same (Codex on #418).
    #[test]
    fn only_a_codex_entry_passes_an_uncredited_rest() {
        let rest = providers_db::State {
            down_until: db::now_ms() + 60_000,
            uncredited: true,
            ..Default::default()
        };
        let entry = |cli: &str| Provider::Cli {
            credits: true,
            enabled: true,
            name: "codex".into(),
            cli: cli.into(),
            model: None,
            daily_budget: 10,
            timeout_s: 1,
            limits: Default::default(),
        };
        assert!(!rest_holds(&rest, &entry("codex")));
        for cli in ["claude", "agy", "grok"] {
            assert!(rest_holds(&rest, &entry(cli)), "{cli}");
        }
    }

    #[test]
    fn a_rest_read_without_credits_does_not_hold_an_entry_that_draws_on_them() {
        use std::cell::Cell;
        use std::rc::Rc;
        // (the entry's credits, a stored rest's mark, whether the allowance is read, the mark after)
        for (credits, stored, read, marked) in [
            (true, Some(true), true, false),
            (true, Some(false), false, false),
            (false, Some(true), false, true),
            (false, None, true, true),
            (true, None, true, false),
        ] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let until = db::now_ms() + 60_000;
            if let Some(uncredited) = stored {
                let rest = providers_db::State {
                    down_until: until,
                    uncredited,
                    ..Default::default()
                };
                providers_db::set_state(&conn, "codex", rest).unwrap();
            }
            let seen = Rc::new(Cell::new(false));
            let saw = Rc::clone(&seen);
            CODEX_REST_TEST.with(|probe| {
                *probe.borrow_mut() = Some(Box::new(move |_| {
                    saw.set(true);
                    Some(until)
                }))
            });
            let providers = [Provider::Cli {
                credits,
                enabled: true,
                name: "codex".into(),
                cli: "codex".into(),
                model: None,
                daily_budget: 10,
                timeout_s: 1,
                limits: Default::default(),
            }];
            let gate = || Ok(Some(crate::dispatch::Admission::shared(home.path())?));
            let isolation = || Ok(crate::isolation::Gate::Passed);
            let mut chain = Chain::new(&providers, &conn).gate(&gate);
            chain.isolation = Some(&isolation);
            chain.forced_fail = None;
            let result = chain.run("curator", "preflight", "synthetic prompt", &json!({}));
            CODEX_REST_TEST.with(|probe| probe.borrow_mut().take());
            let case = format!("credits {credits}, stored {stored:?}");
            assert!(result.is_err(), "{case}");
            assert_eq!(seen.get(), read, "{case}");
            let after = providers_db::state(&conn, "codex").unwrap();
            assert_eq!(
                (after.down_until, after.uncredited),
                (until, marked),
                "{case}"
            );
        }
    }

    /// Allowance reads carry no prompt. They must leave registration free, and a rest must
    /// preserve the unsent accounting without asking the final raw egress gate.
    #[test]
    fn codex_allowance_preflight_leaves_registration_free_preserving_rest_accounting() {
        use std::cell::Cell;
        use std::rc::Rc;
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let until = db::now_ms() + 60_000;
        let was = providers_db::State {
            fails: 1,
            backoff: 2,
            ..Default::default()
        };
        providers_db::set_state(&conn, "codex", was).unwrap();
        let free = Rc::new(Cell::new(false));
        let reads = Rc::new(Cell::new(0));
        let path = home.path().join("dispatch.lock");
        let observed = Rc::clone(&free);
        let observed_reads = Rc::clone(&reads);
        CODEX_REST_TEST.with(|probe| {
            *probe.borrow_mut() = Some(Box::new(move |_| {
                observed_reads.set(observed_reads.get() + 1);
                let registration = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&path)
                    .unwrap();
                observed.set(registration.try_lock().is_ok());
                Some(until)
            }))
        });
        let providers = [Provider::Cli {
            credits: false,
            enabled: true,
            name: "codex".into(),
            cli: "codex".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 1,
            limits: Default::default(),
        }];
        let gates = Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            Ok(Some(crate::dispatch::Admission::shared(home.path())?))
        };
        let isolation = || Ok(crate::isolation::Gate::Passed);
        let mut chain = Chain::new(&providers, &conn).gate(&gate);
        chain.isolation = Some(&isolation);
        chain.forced_fail = None;
        OFFLINE_CALL_TEST.with(|call| {
            *call.borrow_mut() =
                Some(CallError::other("synthetic offline dispatch refusal").unsent())
        });
        let result = chain.run("curator", "preflight", "synthetic prompt", &json!({}));
        CODEX_REST_TEST.with(|probe| probe.borrow_mut().take());
        let not_called = OFFLINE_CALL_TEST.with(|call| call.borrow_mut().take().is_some());
        let error = result.unwrap_err();
        let failed = error.downcast_ref::<ChainFailed>().unwrap();
        assert_eq!(failed.0[0].skip, Skip::Wait(until));
        // Read without credits (the entry draws on none), so marked so.
        assert_eq!(
            providers_db::state(&conn, "codex").unwrap(),
            providers_db::State {
                down_until: until,
                uncredited: true,
                ..was
            }
        );
        // Existing error accounting counts the unsent attempt, even with no prompt bytes.
        assert_eq!(providers_db::calls_in_a_day(&conn, "codex").unwrap().0, 1);
        let sent: i64 = conn
            .query_row("SELECT bytes_out FROM provider_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sent, 0);
        assert_eq!(outcomes(&conn), ["error"]);
        assert_eq!(reads.get(), 1);
        assert!(not_called, "an allowance rest attempted prompt dispatch");
        eprintln!(
            "allowance boundary: exclusive_free={}, dispatch_gates={}, reads={}, attempts=1, bytes_out={sent}, skip={:?}, state={:?}",
            free.get(),
            gates.get(),
            reads.get(),
            failed.0[0].skip,
            providers_db::state(&conn, "codex").unwrap()
        );
        assert!(
            free.get(),
            "an allowance read held shared dispatch admission and blocked registration"
        );
        assert_eq!(
            gates.get(),
            0,
            "an unsent allowance rest acquired the dispatch gate"
        );
    }

    #[test]
    fn codex_allowance_preflight_forget_is_seen_by_the_fresh_dispatch_gate() {
        use std::cell::Cell;
        use std::rc::Rc;
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[summary]\ncurate = false\n",
        )
        .unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let mut event = crate::raw::test_event(r#"{"prompt":"synthetic-preflight-forget-971"}"#);
        event.source = "transcript".into();
        let identity = crate::raw::ImportIdentity {
            origin: crate::forget::origin("synthetic-preflight", "971"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        let preview = crate::forget::preview(
            home.path(),
            crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            },
        )
        .unwrap();
        let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live).unwrap();
        let registered = Rc::new(Cell::new(false));
        let observed = Rc::clone(&registered);
        let path = home.path().to_owned();
        CODEX_REST_TEST.with(|probe| {
            *probe.borrow_mut() = Some(Box::new(move |_| {
                let (status, _) = crate::forget::start(&path, &preview).unwrap();
                observed.set(status.records == 1);
                None
            }))
        });
        let conn = providers_db::open(home.path()).unwrap();
        let providers = [Provider::Cli {
            credits: false,
            enabled: true,
            name: "codex".into(),
            cli: "codex".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 1,
            limits: Default::default(),
        }];
        let gates = Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            let admission = raw.dispatch()?;
            reading.still(&raw)?;
            Ok(Some(admission))
        };
        let isolation = || Ok(crate::isolation::Gate::Passed);
        let mut chain = Chain::new(&providers, &conn).gate(&gate);
        chain.isolation = Some(&isolation);
        chain.forced_fail = None;
        OFFLINE_CALL_TEST.with(|call| {
            *call.borrow_mut() =
                Some(CallError::other("synthetic offline dispatch refusal").unsent())
        });
        let result = chain.run("curator", "preflight", "synthetic prompt", &json!({}));
        CODEX_REST_TEST.with(|probe| probe.borrow_mut().take());
        let not_called = OFFLINE_CALL_TEST.with(|call| call.borrow_mut().take().is_some());
        assert!(
            registered.get(),
            "forget could not commit during allowance discovery"
        );
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<crate::curate::ListChanged>()
                .is_some()
        );
        assert_eq!(gates.get(), 1);
        assert!(not_called, "the stale prompt passed its dispatch gate");
        assert!(
            outcomes(&conn).is_empty(),
            "a stale prompt attempted a provider call"
        );
    }

    #[test]
    fn a_forced_codex_failure_does_not_read_its_allowance() {
        use std::cell::Cell;
        use std::rc::Rc;
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let was = providers_db::State {
            fails: 1,
            backoff: 2,
            ..Default::default()
        };
        providers_db::set_state(&conn, "codex", was).unwrap();
        let reads = Rc::new(Cell::new(0));
        let observed = Rc::clone(&reads);
        CODEX_REST_TEST.with(|probe| {
            *probe.borrow_mut() = Some(Box::new(move |_| {
                observed.set(observed.get() + 1);
                Some(db::now_ms() + 60_000)
            }))
        });
        let providers = [Provider::Cli {
            credits: false,
            enabled: true,
            name: "codex".into(),
            cli: "codex".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 1,
            limits: Default::default(),
        }];
        let gates = Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            Ok(None)
        };
        let isolation = || Ok(crate::isolation::Gate::Passed);
        let mut chain = Chain::new(&providers, &conn).gate(&gate);
        chain.isolation = Some(&isolation);
        chain.forced_fail = Some("codex".into());
        OFFLINE_CALL_TEST.with(|call| {
            *call.borrow_mut() =
                Some(CallError::other("synthetic offline dispatch refusal").unsent())
        });
        let result = chain.run("curator", "forced", "synthetic prompt", &json!({}));
        CODEX_REST_TEST.with(|probe| probe.borrow_mut().take());
        let not_called = OFFLINE_CALL_TEST.with(|call| call.borrow_mut().take().is_some());
        assert_eq!(
            result.unwrap_err().downcast_ref::<ChainFailed>().unwrap().0[0].skip,
            Skip::Failed
        );
        assert_eq!(reads.get(), 0);
        assert!(not_called, "a forced failure dispatched a prompt");
        assert_eq!(gates.get(), 1);
        assert_eq!(providers_db::state(&conn, "codex").unwrap(), was);
        let sent: i64 = conn
            .query_row("SELECT bytes_out FROM provider_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sent, 0);
    }

    /// The app server answers only while its stdin is open: the read keeps it open until the
    /// answer, and a server that never answers is given up on and killed.
    #[cfg(unix)]
    #[test]
    fn codex_limits_are_read_from_a_server_that_waits_for_its_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        // As codex does: the answer comes a moment after the request, and the end of stdin ends
        // the server before it.
        let answering = fake(
            "answering",
            r#"while read -r line; do case "$line" in *'"id":1'*) (sleep 0.3; echo '{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":47}}}}') & pid=$!;; esac; done; kill $pid 2>/dev/null"#,
        );
        let second = Duration::from_secs(1);
        let got = codex_limits(answering.as_os_str(), second * 5).expect("an answer");
        assert_eq!(got["rateLimits"]["primary"]["usedPercent"], 47);
        let silent = fake("silent", "while read -r line; do :; done");
        let started = Instant::now();
        assert!(codex_limits(silent.as_os_str(), second).is_none());
        assert!(started.elapsed() < second * 3);
        let missing = dir.path().join("missing");
        assert!(codex_limits(missing.as_os_str(), second).is_none());
    }

    /// Claude decision C1 at claude-mem's lines (owner delegated, 2026-09-28): a window used to
    /// its line (five hours 95%, a week 93%, the Sonnet week 92%), the last quarter hour of a
    /// five-hour window used to 85%, a rejection, or paid overage rests claude until the reset; a
    /// warning under the line does not. An event with no utilization, or of a window with no line, rests it on a warning.
    #[test]
    fn claude_rests_at_claude_mems_usage_lines() {
        let now_s = db::now_ms() / 1000;
        let later = now_s + 86_400;
        let rest = |info: Value| {
            let rate = json!({"type": "rate_limit_event", "rate_limit_info": info});
            claude_rest(
                &[clean_init(), rate, result_of("{}")]
                    .map(|v| v.to_string())
                    .join("\n"),
            )
        };
        let at = |status: &str, window: &str, used: f64| {
            json!({"status": status, "resetsAt": later, "rateLimitType": window,
                "utilization": used})
        };
        // The owner's week on 2026-09-27: a warning at 87%, under the week's line.
        assert_eq!(rest(at("allowed_warning", "seven_day", 0.87)), None);
        assert_eq!(
            rest(at("allowed_warning", "seven_day", 0.93)),
            Some(later * 1000)
        );
        assert_eq!(
            rest(at("allowed", "seven_day_sonnet", 0.92)),
            Some(later * 1000)
        );
        assert_eq!(rest(at("allowed", "five_hour", 0.94)), None);
        assert_eq!(rest(at("allowed", "five_hour", 0.95)), Some(later * 1000));
        assert_eq!(rest(at("rejected", "seven_day", 0.10)), Some(later * 1000));
        let soon = now_s + 600;
        let ending = |used: f64| {
            json!({"status": "allowed", "resetsAt": soon, "rateLimitType": "five_hour",
                "utilization": used})
        };
        assert_eq!(rest(ending(0.85)), Some(soon * 1000));
        assert_eq!(rest(ending(0.84)), None);
        let paid = json!({"status": "allowed", "resetsAt": later, "rateLimitType": "overage",
            "utilization": 0.01, "isUsingOverage": true});
        assert_eq!(rest(paid), Some(later * 1000));
        let paid = json!({"status": "allowed", "isUsingOverage": true});
        assert_eq!(rest(paid), Some(providers_db::OWNER_HOLD));
        let bare = |status: &str| json!({"status": status, "resetsAt": later});
        assert_eq!(rest(bare("allowed_warning")), Some(later * 1000));
        assert_eq!(rest(bare("allowed")), None);
        // A window claude-mem does not name, or none, has no line: its status decides.
        for window in ["", "seven_day_haiku"] {
            assert_eq!(rest(at("allowed", window, 0.99)), None, "{window}");
            let warned = rest(at("allowed_warning", window, 0.5));
            assert_eq!(warned, Some(later * 1000), "{window}");
        }
        let untyped = json!({"status": "allowed", "resetsAt": later, "utilization": 0.99});
        assert_eq!(rest(untyped), None);
        // At its line with no reset: an hour, then claude is asked again.
        let hour = REST_WITHOUT_RESET.as_millis() as i64;
        let unset = rest(json!({"status": "rejected", "rateLimitType": "five_hour"})).unwrap();
        assert!(
            (now_s * 1000 + hour..=db::now_ms() + hour).contains(&unset),
            "{unset}"
        );
    }

    #[test]
    fn a_rate_limit_warning_cools_claude_down_until_its_reset() {
        for status in ["allowed_warning", "rejected"] {
            let out = claude_events(clean_init(), status, result_of("{}"));
            assert!(claude_stream(&out).is_ok());
            // Until the weekly window's reset, days away: not capped at a day.
            assert_eq!(claude_rest(&out), Some(1_790_744_400_000), "{status}");
        }
    }

    #[test]
    fn claudes_reset_holds_when_its_run_fails_or_its_answer_is_unusable() {
        let reset_s = db::now_ms() / 1000 + 5 * 86_400; // a weekly window, 5 days away
        let rejected = [
            clean_init(),
            json!({"type": "rate_limit_event", "rate_limit_info":
                {"status": "rejected", "resetsAt": reset_s, "rateLimitType": "seven_day"}}),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let rest = claude_rest(&rejected);
        assert_eq!(rest, Some(reset_s * 1000));
        // The failed run, as cli_headless passes it on: not a 10-minute outage.
        let e = claude_stream(&rejected).unwrap_err().resting(rest);
        let s = next_state(providers_db::State::default(), &e);
        assert_eq!(s.down_until, reset_s * 1000);
        // Doctor lists it with its reset.
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        providers_db::set_state(&conn, "claude", s).unwrap();
        assert_eq!(
            providers_db::stopped(&conn).unwrap(),
            [("claude".to_owned(), reset_s * 1000)]
        );
        // An answer of the wrong shape under a warning, as the chain passes it on.
        let e =
            CallError::other("invalid output: the answer does not match the schema").resting(rest);
        assert_eq!(
            next_state(providers_db::State::default(), &e).down_until,
            reset_s * 1000
        );
    }

    #[test]
    fn credits_required_stops_claude_until_the_owner_acts() {
        let out = [
            clean_init(),
            // claude 2.1.283 folds the API's error_code into the rate-limit info (`Kbe`).
            json!({"type": "rate_limit_event", "rate_limit_info":
                {"status": "rejected", "errorCode": "credits_required"}}),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        // Read from the stream on its own too, as a failed or timed-out run passes it on.
        assert_eq!(claude_rest(&out), Some(providers_db::OWNER_HOLD));
        let e = claude_stream(&out).expect_err("credits required");
        let s = next_state(providers_db::State::default(), &e);
        assert_eq!(s.down_until, providers_db::OWNER_HOLD);
        // No time ends it; the owner's `oboete resume` does.
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        providers_db::set_state(&conn, "claude", s).unwrap();
        assert_eq!(
            providers_db::stopped(&conn).unwrap(),
            [("claude".to_owned(), providers_db::OWNER_HOLD)]
        );
        assert!(providers_db::resume(&conn, "claude").unwrap());
        assert!(providers_db::stopped(&conn).unwrap().is_empty());
        assert_eq!(providers_db::state(&conn, "claude").unwrap().down_until, 0);
    }

    #[test]
    fn a_stream_line_that_does_not_parse_discards_the_run() {
        let out = [
            clean_init().to_string(),
            r#"{"type": "assistant", "message": {"content": [{"type": "tool_use""#.to_owned(),
            json!({"type": "result", "subtype": "success", "result": "{}"}).to_string(),
        ]
        .join("\n");
        let e = claude_stream(&out).expect_err("a truncated line");
        assert!(e.message.contains("stream line"), "{}", e.message);
    }

    /// The owner's codex, under the curator's environment: its app server reports the plan's
    /// windows (run with `--ignored`; no model is called).
    #[test]
    #[ignore]
    fn live_codex_reports_its_plan_windows() {
        let read = codex_limits(std::ffi::OsStr::new("codex"), CODEX_LIMITS_TIMEOUT).unwrap();
        let used = &read["rateLimits"]["primary"]["usedPercent"];
        assert!(used.is_number(), "{read}");
        println!("codex rest: {:?}", codex_rest(&read, db::now_ms(), false));
    }

    /// Live, with `--ignored`, in the dogfood user only (curator CLI tests run there): each
    /// subscription CLI answers under S8's environment and claude's spec 6.5 flags. The schema is
    /// the summary role's, which has neither of the curator's `claims` and `summary`: a schema
    /// with `summary` in it passed here while codex's answers to that role were all dropped.
    #[test]
    #[ignore]
    fn live_subscription_curators_answer_under_the_curator_environment() {
        let schema = crate::turns::answer_schema();
        let prompt = "Summarize this session: fill every field, in a sentence at most.\n\
            --- SESSION ---\n\
            [user] fix the date parser in src/ingest/csv_reader.py\n\
            [assistant] added %d.%m.%Y; 21 tests pass\n--- END ---";
        for (cli, model) in [("claude", "haiku"), ("codex", "gpt-6-luna")] {
            cli_preflight(cli, false).unwrap_or_else(|e| panic!("{cli}: {}", e.message));
            let a = cli_headless(cli, Some(model), 180, CURATOR_SYSTEM, prompt, &schema, None)
                .unwrap_or_else(|e| panic!("{cli}: {}", e.message));
            assert!(fits(&a.value, &schema), "{cli}: {}", a.value);
            eprintln!("{cli}: {:?}, rest until {:?}", a.usage, a.cool_until);
        }
    }

    #[test]
    fn cli_prompts_stay_off_the_command_line() {
        let scratch = scratch_dir().unwrap();
        let dir = &scratch.0;
        let prompt = "SECRET-SESSION-TEXT 秘密";
        for cli in ["agy", "claude", "grok", "codex"] {
            let (cmd, stdin) =
                headless_command(cli, Some("m"), dir, CURATOR_SYSTEM, prompt, "{}").unwrap();
            let on_stdin = stdin.is_some_and(|t| t.contains(prompt));
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(
                args.iter().all(|a| !a.contains("SECRET-SESSION-TEXT")),
                "{cli}: {args:?}"
            );
            assert!(
                args.iter().all(|a| a != "--dangerously-skip-permissions"),
                "{cli}: {args:?}"
            );
            let file = dir.join("task.md");
            let in_file = std::fs::read_to_string(&file).is_ok_and(|t| t == prompt);
            assert!(
                on_stdin != in_file,
                "{cli}: exactly one of stdin / task.md carries it"
            );
            std::fs::remove_file(&file).ok();
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_runs_neither_deadlock_nor_outlive_the_timeout() {
        let sh = |script: &str| {
            let mut c = Command::new("sh");
            c.args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            c
        };
        // Larger than any pipe buffer in both directions.
        let big = "x".repeat(300_000);
        let (out, ran) = run_cli(sh("cat"), Some(big.clone()), Duration::from_secs(10), None);
        ran.unwrap();
        assert_eq!(out, big.as_bytes());
        let err = run_cli(
            sh("head -c 1100000 /dev/zero"),
            None,
            Duration::from_secs(10),
            None,
        )
        .1
        .unwrap_err();
        assert!(err.message.contains("more than"), "{}", err.message);
        let (out, ran) = run_cli(
            sh("echo said; echo boom >&2; exit 3"),
            None,
            Duration::from_secs(10),
            None,
        );
        assert_eq!(out, b"said\n", "stdout is kept on a failure");
        let err = ran.unwrap_err();
        assert!(!err.message.contains("boom"), "{}", err.message);
        assert!(
            err.message.contains("exit") && err.message.contains("5 bytes"),
            "{}",
            err.message
        );
        let start = Instant::now();
        let (out, ran) = run_cli(
            sh("echo early; sleep 5"),
            None,
            Duration::from_secs(1),
            None,
        );
        let err = ran.unwrap_err();
        assert!(err.message.contains("timed out"), "{}", err.message);
        assert_eq!(out, b"early\n", "and on a timeout");
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_releases_dispatch_after_stdin_before_its_answer() {
        let home = tempfile::tempdir().unwrap();
        let admission = crate::dispatch::Admission::shared(home.path()).unwrap();
        let mut command = Command::new("sh");
        command
            .args(["-c", "cat >/dev/null; sleep 1; printf answered"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (done_at, done) = std::sync::mpsc::sync_channel(1);
        let sender = std::thread::spawn(move || {
            let result = run_cli(
                command,
                Some("synthetic prompt ".repeat(20_000)),
                Duration::from_secs(3),
                Some(admission),
            );
            done_at.send(()).unwrap();
            result
        });
        let path = home.path().join("dispatch.lock");
        let registration = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let until = Instant::now() + Duration::from_millis(500);
        let mut acquired = false;
        while Instant::now() < until {
            if registration.try_lock().is_ok() {
                acquired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let waiting = matches!(done.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
        let (output, result) = sender.join().unwrap();
        result.unwrap();
        assert_eq!(output, b"answered");
        assert!(acquired && waiting, "CLI admission lasted until its answer");
    }

    #[test]
    fn a_cli_stops_input_before_releasing_dispatch_even_with_a_descendant() {
        use std::cell::Cell;
        use std::rc::Rc;
        // The owned loop's deadline tests blocked input after both real fixture processes are
        // ready; starting an interpreter is a separately bounded setup operation.
        let timeout = Duration::from_secs(if cfg!(windows) { 10 } else { 1 });
        // Two Python starts on a busy Windows runner have taken longer than 20 s (#451).
        let startup = Duration::from_secs(if cfg!(windows) { 60 } else { 20 });
        // The reader outlives every wait after the fixture's start and the run: the run's 2 s, the
        // lock's 2 s and the receive's 5 s (Codex on #445).
        let watchdog = (startup.as_secs() + timeout.as_secs() + 9).to_string();
        for mode in ["timeout", "failure"] {
            let home = tempfile::tempdir().unwrap();
            let admission = crate::dispatch::Admission::shared(home.path()).unwrap();
            // A fixed synthetic Python child, not a provider or another test-executable mode.
            // Its reader deliberately escapes the Unix group (Windows kills the parent only),
            // keeps stdin after the timeout, and starts reading only after run_cli returns.
            let mut command = Command::new(if cfg!(windows) { "python" } else { "python3" });
            command
                .args([
                    "-u",
                    "-c",
                    r#"
import os, pathlib, subprocess, sys, threading, time
home = pathlib.Path(sys.argv[1])
watchdog_seconds = int(sys.argv[3])
def watchdog():
    time.sleep(watchdog_seconds)
    os._exit(99)
threading.Thread(target=watchdog, daemon=True).start()
while not (home / 'start-reader').exists():
    time.sleep(.002)
reader = '''
import os, pathlib, sys, threading, time
home = pathlib.Path(sys.argv[1])
watchdog_seconds = int(sys.argv[2])
def watchdog():
    time.sleep(watchdog_seconds)
    os._exit(99)
threading.Thread(target=watchdog, daemon=True).start()
(home / "reader-ready").write_text("ready")
while not (home / "read-now").exists():
    time.sleep(.002)
body = sys.stdin.buffer.read()
(home / "received.tmp").write_text(str(len(body)))
(home / "received.tmp").replace(home / "received")
'''
child = subprocess.Popen([sys.executable, '-u', '-c', reader, str(home), str(watchdog_seconds)],
    stdin=sys.stdin, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    start_new_session=os.name == 'posix')
(home / 'reader-pid').write_text(str(child.pid))
while not (home / 'reader-ready').exists():
    time.sleep(.002)
(home / 'fixture-ready').write_text('ready')
if sys.argv[2] == 'failure':
    sys.exit(7)
child.wait()
"#,
                ])
                .arg(home.path())
                .arg(mode)
                .arg(&watchdog)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let text = "synthetic-stalled-prompt ".repeat(200_000);
            let length = text.len();
            let began = Instant::now();
            let running = Rc::new(Cell::new(None));
            let observed = Rc::clone(&running);
            let path = home.path().to_owned();
            CLI_READY_TEST.with(|ready| {
                *ready.borrow_mut() = Some(Box::new(move || {
                    std::fs::write(path.join("start-reader"), "go")?;
                    let until = Instant::now() + startup;
                    while !path.join("fixture-ready").exists() {
                        if Instant::now() >= until {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "the synthetic CLI did not become ready",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    observed.set(Some(Instant::now()));
                    Ok(())
                }))
            });
            let (_, result) = run_cli(command, Some(text), timeout, Some(admission.clone()));
            CLI_READY_TEST.with(|ready| ready.borrow_mut().take());
            let error = result.unwrap_err();
            assert!(
                error.message.contains(if mode == "timeout" {
                    "timed out"
                } else {
                    "exit"
                }),
                "{}",
                error.message
            );
            assert!(running.get().unwrap_or(began).elapsed() < timeout + Duration::from_secs(2));
            assert!(began.elapsed() < startup + timeout + Duration::from_secs(2));
            assert!(
                home.path().join("reader-pid").exists(),
                "the descendant fixture was not running"
            );
            let registration = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(home.path().join("dispatch.lock"))
                .unwrap();
            // A concurrent fork may retain the lock until exec; keep the reader paused meanwhile.
            let until = Instant::now() + Duration::from_secs(2);
            let mut unlocked = registration.try_lock().is_ok();
            while !unlocked && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(1));
                unlocked = registration.try_lock().is_ok();
            }
            // Give the escaped reader a chance to drain bytes already handed off. A detached
            // blocking feeder would resume and write the rest only after run_cli returned.
            std::fs::write(home.path().join("read-now"), "go").unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            while !home.path().join("received").exists() {
                assert!(
                    Instant::now() < until,
                    "the bounded descendant fixture did not finish"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            let received = std::fs::read_to_string(home.path().join("received"))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            eprintln!(
                "{mode} input boundary: unlocked={unlocked}, received={received}, total={length}"
            );
            assert!(
                unlocked,
                "{mode} CLI input kept dispatch admission after return"
            );
            assert!(
                received < length,
                "stdin kept writing the remaining prompt after return"
            );
        }
    }

    #[test]
    fn a_cli_spawn_failure_releases_dispatch() {
        let home = tempfile::tempdir().unwrap();
        let admission = crate::dispatch::Admission::shared(home.path()).unwrap();
        let command = Command::new(home.path().join("absent-synthetic-provider"));
        let (_, result) = run_cli(
            command,
            Some("synthetic".into()),
            Duration::from_secs(1),
            Some(admission.clone()),
        );
        let error = result.unwrap_err();
        assert!(!error.sent && error.message.starts_with("spawn:"));
        let registration = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(home.path().join("dispatch.lock"))
            .unwrap();
        // Concurrent test children may briefly inherit a CLOEXEC descriptor before exec.
        // A surviving admission clone would retain the lock beyond this bounded wait.
        let until = Instant::now() + Duration::from_millis(500);
        loop {
            match registration.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    assert!(Instant::now() < until, "failed spawn kept admission");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("dispatch fixture lock: {e}"),
            }
        }
    }

    #[test]
    fn agy_answer_is_the_last_result_event() {
        let out = concat!(
            "{\"event\":\"init\",\"session\":\"s\"}\n",
            "not json\n",
            "{\"event\":\"result\",\"result\":{\"structured_output\":{\"summary\":\"x\"}}}\n",
        );
        let text = agy_result(out).unwrap();
        assert_eq!(
            extract_structured("agy", &text).unwrap(),
            json!({"summary": "x"})
        );
        assert!(agy_result("{\"event\":\"init\"}\n").is_err());
    }

    /// One-shot HTTP server on localhost that answers any request with `body`.
    /// Also hands back the request it received.
    fn serve_once(
        body: Vec<u8>,
        extra_headers: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        serve("200 OK", body, extra_headers)
    }

    /// `serve_once` with another status line.
    fn serve(
        status: &'static str,
        body: Vec<u8>,
        extra_headers: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        serve_each(vec![(status, body)], extra_headers)
    }

    /// `serve` for one request after another, each answered in turn.
    pub(crate) fn serve_each(
        answers: Vec<(&'static str, Vec<u8>)>,
        extra_headers: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let mut answers = answers.into_iter();
        serve_by(
            answers.len(),
            move |_| answers.next().unwrap(),
            extra_headers,
        )
    }

    /// `serve_each` for `n` requests, each answered by what `answer` makes of it.
    pub(crate) fn serve_by(
        n: usize,
        mut answer: impl FnMut(&str) -> (&'static str, Vec<u8>) + Send + 'static,
        extra_headers: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..n {
                let (mut conn, _) = listener.accept().unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                // Read the headers and the JSON body (its length is in Content-Length).
                while let Ok(n) = conn.read(&mut buf) {
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= end + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let req = String::from_utf8_lossy(&req).into_owned();
                let (status, body) = answer(&req);
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // Before the answer: a client has it once its last byte is read, and a test that
                // then asks for the requests so far (`try_iter`) is to find this one.
                tx.send(req).ok();
                conn.write_all(head.as_bytes()).ok();
                conn.write_all(&body).ok();
            }
        });
        (format!("http://{addr}"), rx)
    }

    /// The JSON body of a request a test server received.
    pub(crate) fn request_body(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    /// One OpenAI-compatible call with `extra`, to `url` as OpenRouter's when `openrouter`.
    fn send_to(
        url: &str,
        model: &str,
        extra: Value,
        openrouter: bool,
    ) -> Result<Answer, CallError> {
        OPENROUTER_AT.set(openrouter.then(|| url.to_owned()));
        http_call(
            Api::Openai,
            url,
            None,
            model,
            10,
            extra.as_object().unwrap(),
            &Default::default(),
            "p",
            &json!({}),
            None,
            0,
            false,
        )
    }

    /// claude-mem 15.0.2: a call to OpenRouter asks for no reasoning, unless its entry sets
    /// reasoning itself; a call elsewhere gets no such field.
    #[test]
    fn an_openrouter_call_asks_for_no_reasoning_unless_its_entry_sets_it() {
        let answer = json!({"choices": [{"message": {"content": "{\"summary\": \"s\"}"},
            "finish_reason": "stop"}]})
        .to_string()
        .into_bytes();
        let cases = [
            (json!({}), true, Some(json!({"enabled": false}))),
            (
                json!({"reasoning": {"effort": "low"}}),
                true,
                Some(json!({"effort": "low"})),
            ),
            (json!({"reasoning_effort": "low"}), true, None),
            (json!({"include_reasoning": true}), true, None),
            (json!({}), false, None),
        ];
        for (extra, openrouter, reasoning) in cases {
            let (url, request) = serve_once(answer.clone(), "");
            let a = send_to(&url, "switch-asked", extra.clone(), openrouter).unwrap();
            assert_eq!(a.value, json!({"summary": "s"}));
            let body = request_body(&request.recv().unwrap());
            assert_eq!(
                body.get("reasoning"),
                reasoning.as_ref(),
                "{extra} {openrouter}"
            );
        }
    }

    /// A model that cannot stop reasoning refuses the switch with a 400, unbilled: the chain asks
    /// again without it, after the budget and the egress gate again, as a 429's retry does (Codex
    /// on #643), and the model gets no switch from then on; the settings probe does the same.
    /// Another 400 is no refusal and is not sent again.
    #[test]
    fn a_model_that_refuses_the_reasoning_switch_is_asked_again_without_it() {
        let refused = || {
            json!({"error": {"message":
                "Reasoning is mandatory for this endpoint and cannot be disabled.", "code": 400}})
            .to_string()
            .into_bytes()
        };
        let answer = |content: &str| {
            json!({"choices": [{"message": {"content": content}, "finish_reason": "stop"}]})
                .to_string()
                .into_bytes()
        };
        let (url, requests) = serve_each(
            vec![
                ("400 Bad Request", refused()),
                ("200 OK", answer("{\"summary\": \"s\"}")),
                ("200 OK", answer("{\"summary\": \"s\"}")),
            ],
            "",
        );
        OPENROUTER_AT.set(Some(url.clone()));
        let mut p = stub(url);
        if let Provider::Openai { model, .. } = &mut p {
            *model = "switch-refused".into();
        }
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let first = std::cell::RefCell::new(None);
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            // The second request's gate comes after the first went out.
            if gates.get() == 2 {
                let sent = requests.recv_timeout(Duration::from_secs(2)).unwrap();
                *first.borrow_mut() = Some(request_body(&sent));
            }
            Ok(None)
        };
        let run = |p: &Provider| {
            Chain::new(std::slice::from_ref(p), &conn).gate(&gate).run(
                "curator",
                "s",
                "synthetic",
                &json!({}),
            )
        };
        assert_eq!(run(&p).unwrap().output, json!({"summary": "s"}));
        assert_eq!(gates.get(), 2);
        assert_eq!(outcomes(&conn), ["error", "ok"]);
        let first = first.borrow_mut().take().unwrap();
        assert_eq!(first["reasoning"], json!({"enabled": false}));
        let second = request_body(&requests.recv().unwrap());
        assert!(second.get("reasoning").is_none(), "{second}");
        run(&p).unwrap();
        let third = request_body(&requests.recv().unwrap());
        assert!(third.get("reasoning").is_none(), "{third}");

        let (url, requests) = serve_each(
            vec![
                ("400 Bad Request", refused()),
                ("200 OK", answer("{\"ok\":true}")),
            ],
            "",
        );
        OPENROUTER_AT.set(Some(url.clone()));
        let mut probed = stub(url);
        if let Provider::Openai { model, .. } = &mut probed {
            *model = "switch-refused-probe".into();
        }
        let probes = std::cell::Cell::new(0);
        let gate = || {
            probes.set(probes.get() + 1);
            Ok(None)
        };
        let result = probe(&conn, &probed, 5.0, &gate).unwrap();
        assert_eq!(result["code"], "ok", "{result}");
        assert_eq!(probes.get(), 2);
        assert_eq!(
            request_body(&requests.recv().unwrap())["reasoning"],
            json!({"enabled": false})
        );
        assert!(
            request_body(&requests.recv().unwrap())
                .get("reasoning")
                .is_none()
        );
        // Each request settles (Codex on #643): the refused one, then the answered one.
        assert_eq!(outcomes(&conn)[3..], ["error", "ok"]);
        // A probe at its entry's last request of the day is not sent again.
        let (url, requests) = serve_each(vec![("400 Bad Request", refused())], "");
        OPENROUTER_AT.set(Some(url.clone()));
        let mut last = stub(url);
        if let Provider::Openai {
            model,
            name,
            daily_budget,
            ..
        } = &mut last
        {
            *model = "switch-refused-last".into();
            *name = "last".into();
            *daily_budget = Some(1);
        }
        let result = probe(&conn, &last, 5.0, &|| Ok(None)).unwrap();
        assert_eq!(result["status"], "blocked", "{result}");
        assert!(requests.recv().is_ok());
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(providers_db::calls_in_a_day(&conn, "last").unwrap().0, 1);

        let other =
            json!({"error": {"message": "Invalid schema for response_format", "code": 400}})
                .to_string()
                .into_bytes();
        let (url, requests) = serve_each(vec![("400 Bad Request", other)], "");
        OPENROUTER_AT.set(Some(url.clone()));
        let mut plain = stub(url);
        if let Provider::Openai { model, .. } = &mut plain {
            *model = "switch-other-400".into();
        }
        let failed = Chain::new(&[plain], &conn)
            .run("curator", "s", "synthetic", &json!({}))
            .unwrap_err();
        assert!(
            !failed.to_string().contains("reasoning refused"),
            "{failed}"
        );
        assert_eq!(
            request_body(&requests.recv().unwrap())["reasoning"],
            json!({"enabled": false})
        );
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// An answer cut at its length limit before any text, its output spent on reasoning, fails as
    /// such with its usage; an empty answer that stopped on its own is read as before.
    #[test]
    fn an_answer_cut_at_its_length_limit_before_any_text_says_so() {
        for content in [json!(""), json!(null), json!("  ")] {
            let answer = json!({"choices": [{"message": {"content": content},
                "finish_reason": "length"}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 4000}});
            let (url, _) = serve_once(answer.to_string().into_bytes(), "");
            let e = send_to(&url, "cut", json!({}), false).unwrap_err();
            assert!(e.invalid(), "{}", e.message);
            assert!(e.message.contains("length limit"), "{}", e.message);
            assert_eq!(e.usage.completion, Some(4000));
        }
        let stopped = json!({"choices": [{"message": {"content": ""}, "finish_reason": "stop"}]});
        let (url, _) = serve_once(stopped.to_string().into_bytes(), "");
        let a = send_to(&url, "cut", json!({}), false).unwrap();
        assert_eq!(a.value, json!(""));
    }

    /// R14 (parity row 2): a call out keeps the worker's watchdog from calling the loop stuck for
    /// as long as its timeout allows: an answer that takes a second, against 20 looks of 10 ms.
    #[test]
    fn a_call_out_keeps_the_watchdog_waiting() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let answer =
            json!({"choices": [{"message": {"content": "{\"summary\": \"x\"}"}}]}).to_string();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = conn.read(&mut [0u8; 65_536]);
            std::thread::sleep(Duration::from_secs(1));
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        });
        let watched = crate::watchdog::Watched::here();
        let (tx, stuck) = std::sync::mpsc::channel();
        let _watch = crate::watchdog::watch(
            watched,
            Duration::from_millis(10),
            20,
            |_| {},
            move |n| {
                let _ = tx.send(n);
            },
        );
        // The loop has begun: the watchdog counts from here on.
        crate::watchdog::progressed();
        let _ = call(
            &stub(url),
            CURATOR_SYSTEM,
            "prompt",
            &json!({}),
            None,
            false,
        );
        assert!(stuck.try_recv().is_err(), "stuck while a call was out");
    }

    #[test]
    fn only_loopback_urls_skip_the_proxy() {
        for url in [
            "http://127.0.0.1:11434/v1",
            "http://localhost/v1",
            "http://[::1]:8080",
        ] {
            assert!(is_loopback(url), "{url}");
        }
        for url in [
            "https://api.groq.com/openai/v1",
            "http://127.0.0.1.evil.example/v1",
            "http://localhost@evil.example/",
        ] {
            assert!(!is_loopback(url), "{url}");
        }
    }

    #[test]
    fn token_usage_is_read_from_every_answer_shape() {
        let http = json!({"usage": {"prompt_tokens": 3585, "completion_tokens": 254,
            "prompt_tokens_details": {"cached_tokens": 1024},
            "completion_tokens_details": {"reasoning_tokens": 75}}});
        let want = Usage {
            prompt: Some(3585),
            completion: Some(254),
            cached: Some(1024),
            reasoning: Some(75),
            written: None,
        };
        assert_eq!(usage_openai(&http), want);
        // A provider's own numbers, but only as numbers: nothing else is kept.
        let odd = json!({"usage": {"prompt_tokens": -3, "completion_tokens": "many"}});
        assert_eq!(usage_openai(&odd), Usage::default());
        let claude = json!({"usage": {"input_tokens": 10, "cache_creation_input_tokens": 5306,
            "cache_read_input_tokens": 200, "output_tokens": 1353}})
        .to_string();
        assert_eq!(
            usage_cli("claude", &claude),
            Usage {
                prompt: Some(5516),
                completion: Some(1353),
                cached: Some(200),
                reasoning: None,
                written: Some(5306),
            }
        );
        let codex = [
            r#"{"type":"thread.started","thread_id":"t"}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"{}"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":8990,"cached_input_tokens":2816,"output_tokens":88,"reasoning_output_tokens":12}}"#,
        ]
        .join("\n");
        assert_eq!(
            usage_cli("codex", &codex),
            Usage {
                prompt: Some(8990),
                completion: Some(88),
                cached: Some(2816),
                reasoning: Some(12),
                written: None,
            }
        );
        assert!(!codex_searched(&codex));
        let searched = format!(
            "{codex}\n{}",
            r#"{"type":"item.completed","item":{"id":"i","type":"web_search","query":"q"}}"#
        );
        assert!(codex_searched(&searched));
        assert_eq!(usage_cli("grok", "{}"), Usage::default());
        let huge = json!({"usage": {"input_tokens": i64::MAX, "cache_read_input_tokens": 1}});
        assert_eq!(usage_cli("claude", &huge.to_string()).prompt, None);
    }

    /// #238: a key's limit is read again only when its last read no longer holds: a day after it
    /// gave one, an hour after it failed, and at once for another key. An entry with the owner's
    /// own budget never reads it, and a key file without a key sends nothing.
    #[test]
    fn a_keys_limit_is_read_again_only_when_its_last_read_no_longer_holds() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let key_file = home.path().join("KEY.md");
        let key = |k: &str| std::fs::write(&key_file, format!("# a test key\n{k}\n")).unwrap();
        let free =
            "kind = \"openai\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m:free\"\n";
        let p: Provider = toml::from_str(&format!("name = \"o\"\n{free}")).unwrap();
        let unread = |_: &str| -> Option<u32> { panic!("read while the last read holds") };
        let last = || {
            providers_db::key_limit(&conn, "o")
                .unwrap()
                .map(|(l, at, _)| (l, at))
        };
        let refresh = |now, read: &dyn Fn(&str) -> Option<u32>| {
            refresh_key_limit(&conn, &p, now, Some(&key_file), read).unwrap()
        };
        key("key-a");
        let t0 = 1_000_000_000_000;
        refresh(t0, &|k| (k == "key-a").then_some(1000));
        refresh(t0 + budget::KEY_READ_HOLDS_MS - 1, &unread);
        assert_eq!(last(), Some((Some(1000), t0)));
        key("key-b");
        refresh(t0 + 1, &|k| (k == "key-b").then_some(50));
        assert_eq!(last(), Some((Some(50), t0 + 1)));
        let t1 = t0 + 1 + budget::KEY_READ_HOLDS_MS;
        refresh(t1, &|_| None);
        refresh(t1 + budget::KEY_READ_RETRY_MS - 1, &unread);
        assert_eq!(last(), Some((None, t1)));
        let t2 = t1 + budget::KEY_READ_RETRY_MS;
        refresh(t2, &|_| Some(1000));
        assert_eq!(last(), Some((Some(1000), t2)));
        std::fs::write(&key_file, "# no key on line 2\n").unwrap();
        refresh(t2 + 1, &unread);
        assert_eq!(last(), Some((None, t2 + 1)));
        // An entry whose key file is taken out of its config keeps no earlier key's limit.
        key("key-c");
        refresh(t2 + 2, &|_| Some(1000));
        assert_eq!(last(), Some((Some(1000), t2 + 2)));
        refresh_key_limit(&conn, &p, t2 + 3, None, unread).unwrap();
        assert_eq!(last(), Some((None, t2 + 3)));
        let own: Provider =
            toml::from_str(&format!("name = \"own\"\n{free}daily_budget = 30\n")).unwrap();
        refresh_key_limit(&conn, &own, t2, Some(&key_file), unread).unwrap();
        assert_eq!(providers_db::key_limit(&conn, "own").unwrap(), None);
    }

    /// #238: who is asked includes which key an entry's budget comes from, so a window held under
    /// one key is tried again with another; an entry with the owner's own budget adds nothing.
    #[test]
    fn the_key_a_budget_comes_from_is_part_of_who_is_asked() {
        let home = tempfile::tempdir().unwrap();
        let key_file = home.path().join("KEY.md");
        let entry = |extra: &str| -> Provider {
            toml::from_str(&format!(
                "kind = \"openai\"\nname = \"o\"\nbase_url = \"https://openrouter.ai/api/v1\"\n\
                 model = \"m:free\"\nkey_file = {key_file:?}\n{extra}"
            ))
            .unwrap()
        };
        let asked = |k: &str| {
            std::fs::write(&key_file, format!("# a test key\n{k}\n")).unwrap();
            budget_keys(&[entry("")])
        };
        let (a, b) = (asked("key-a"), asked("key-b"));
        assert_eq!(a.len(), 1);
        assert_ne!(a, b);
        assert!(!a[0].contains("key-a"));
        assert!(budget_keys(&[entry("daily_budget = 30\n")]).is_empty());
    }

    /// #238: the limit is `data.free_model_daily_requests.limit` of the answer to a GET sent with
    /// the key; any other answer gives none.
    #[test]
    fn a_keys_free_limit_is_read_from_its_answer() {
        let key = "test-key-238";
        let answer = json!({"data": {"free_model_daily_requests": {"used": 3, "limit": 1000, "remaining": 997}}});
        let (url, sent) = serve("200 OK", answer.to_string().into_bytes(), "");
        assert_eq!(free_limit(&format!("{url}/key"), key), Some(1000));
        let sent = sent.recv().unwrap();
        assert!(sent.starts_with("GET /key "), "{sent}");
        assert!(
            sent.to_lowercase()
                .contains("authorization: bearer test-key-238\r\n"),
            "{sent}"
        );
        for (status, body) in [
            ("200 OK", json!({"data": {"limit": null}})),
            (
                "200 OK",
                json!({"data": {"free_model_daily_requests": {"limit": "1000"}}}),
            ),
            (
                "401 Unauthorized",
                json!({"data": {"free_model_daily_requests": {"limit": 1000}}}),
            ),
        ] {
            let (url, _sent) = serve(status, body.to_string().into_bytes(), "");
            assert_eq!(free_limit(&url, key), None, "{status} {body}");
        }
        // A limit of 0 is a limit: a fifth of it skips the entry.
        let zero = json!({"data": {"free_model_daily_requests": {"limit": 0}}});
        let (url, _sent) = serve("200 OK", zero.to_string().into_bytes(), "");
        assert_eq!(free_limit(&url, key), Some(0));
        // A redirect is not followed, even to an answer with a limit.
        let (there, _sent) = serve("200 OK", answer.to_string().into_bytes(), "");
        let to: &'static str = Box::leak(format!("Location: {there}/key\r\n").into_boxed_str());
        let (url, _sent) = serve("302 Found", Vec::new(), to);
        assert_eq!(free_limit(&url, key), None);
    }

    /// D10, D11: each provider gone past says whether time, a budget reset or the owner will let
    /// it be tried again, or whether it failed.
    #[test]
    fn each_provider_gone_past_says_what_it_waits_for() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let named = |name: &str, budget: u32| {
            let mut p = stub("http://127.0.0.1:9".into());
            if let Provider::Openai {
                name: n,
                daily_budget,
                ..
            } = &mut p
            {
                *n = name.into();
                *daily_budget = Some(budget);
            }
            p
        };
        let cool_until = crate::db::now_ms() + 60_000;
        let state = |until| providers_db::State {
            down_until: until,
            ..Default::default()
        };
        providers_db::set_state(&conn, "held", state(providers_db::OWNER_HOLD)).unwrap();
        providers_db::set_state(&conn, "cooling", state(cool_until)).unwrap();
        // A 400 sets no cooldown: tried, and failed.
        let (bad, _) = serve("400 Bad Request", b"{}".to_vec(), "");
        let mut refused = stub(bad);
        if let Provider::Openai { name, .. } = &mut refused {
            *name = "refused".into();
        }
        let providers = [
            named("held", 10),
            named("cooling", 10),
            named("spent", 0),
            refused,
        ];
        let before = crate::db::now_ms();
        let err = Chain::new(&providers, &conn)
            .run("curator", "s", "p", &json!({}))
            .unwrap_err();
        let after = crate::db::now_ms();
        let failed = err.downcast_ref::<ChainFailed>().expect("a ChainFailed");
        let skips: Vec<&Skip> = failed.0.iter().map(|f| &f.skip).collect();
        // A budget with no call in the last 24 hours waits a whole day.
        let &&Skip::Budget(until) = &skips[2] else {
            panic!("{skips:?}")
        };
        assert!((before..=after).contains(&(until - providers_db::DAY_MS - 1)));
        assert_eq!(
            [skips[0], skips[1], skips[3]],
            [&Skip::Owner, &Skip::Wait(cool_until), &Skip::Failed]
        );
        assert!(
            err.to_string()
                .starts_with("every provider failed: held: stopped")
        );
    }

    #[test]
    #[cfg(unix)]
    fn an_unbounded_cli_probe_cannot_run_a_nonconforming_long_response() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let executable = home.path().join("nonconforming-claude");
        std::fs::write(&executable, r#"#!/usr/bin/env python3
import json, pathlib, sys
pathlib.Path(__file__).with_suffix('.ran').write_text('ran')
sys.stdin.read()
print(json.dumps({"type":"system","subtype":"init","tools":[],"mcp_servers":[],"plugins":[],"permissionMode":"dontAsk","apiKeySource":"none"}))
print(json.dumps({"type":"result","is_error":False,"result":json.dumps({"ok":True,"extra":"x"*20000}),"usage":{"input_tokens":12,"output_tokens":5000}}))
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let p = Provider::Cli {
            credits: false,
            name: "fixture".into(),
            enabled: true,
            cli: "claude".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 2,
            limits: config::Limits {
                daily_tokens: Some(2_000),
                ..Default::default()
            },
        };
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            Ok(None)
        };
        let result = probe_attempt(&conn, &p, 5.0, &gate, false, Some(&executable)).unwrap();
        assert_eq!(
            providers_db::tokens_since(&conn, "fixture", 0).unwrap().0,
            0,
            "a 1250-token estimate must not admit a 5000-token CLI response"
        );
        assert_eq!(result["status"], "blocked");
        assert_eq!(result["code"], "cli_probe_unbounded");
        assert_eq!(gates.get(), 0);
        assert!(!executable.with_extension("ran").exists());
        assert!(outcomes(&conn).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn normal_cli_keeps_fixed_stdin_private_home_and_checked_init() {
        use std::os::unix::fs::PermissionsExt;
        for failed in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let executable = home
                .path()
                .join(if failed { "error-cli" } else { "answer-cli" });
            std::fs::write(&executable, r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(__file__).parent
prompt = sys.stdin.read()
trace = {"stdin":prompt,"argv":sys.argv[1:],"home":os.environ.get("HOME"),"cwd":os.getcwd(),
         "secret_names":[k for k in os.environ if any(word in k.upper() for word in ("KEY","TOKEN","SECRET","PASSWORD"))]}
(root / "trace.json").write_text(json.dumps(trace))
print(json.dumps({"type":"system","subtype":"init","tools":[],"mcp_servers":[],"plugins":[],
                  "permissionMode":"dontAsk","apiKeySource":"none"}))
print(json.dumps({"type":"result","result":'{"ok":true}',"is_error":"error" in pathlib.Path(__file__).name,
                  "subtype":"private-cli-error-canary","usage":{"input_tokens":12,"output_tokens":4}}))
"#).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = cli_headless_at(
                "claude",
                Some("synthetic-model"),
                2,
                CURATOR_SYSTEM,
                PROBE_PROMPT,
                &probe_schema(),
                None,
                Some(&executable),
            );
            assert_eq!(result.is_ok(), !failed);
            if let Ok(answer) = result {
                assert_eq!(answer.value, json!({"ok":true}));
            }
            let trace: Value =
                serde_json::from_slice(&std::fs::read(home.path().join("trace.json")).unwrap())
                    .unwrap();
            assert_eq!(trace["stdin"], PROBE_PROMPT);
            assert_eq!(trace["home"], home.path().to_str().unwrap());
            assert_eq!(trace["secret_names"], json!([]));
            assert_ne!(trace["cwd"], trace["home"]);
            assert!(
                !Path::new(trace["cwd"].as_str().unwrap()).exists(),
                "scratch was not removed"
            );
            let argv: Vec<&str> = trace["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert!(!argv.contains(&PROBE_PROMPT));
            for expected in [
                ["--tools", ""],
                ["--permission-mode", "dontAsk"],
                ["--permission-prompts", "none"],
                ["--model", "synthetic-model"],
            ] {
                assert!(argv.windows(2).any(|pair| pair == expected));
            }
        }
    }

    #[test]
    fn explicit_probe_refusals_and_forced_failure_never_reach_allowance_or_dispatch() {
        use std::cell::Cell;
        use std::rc::Rc;
        for reason in ["budget", "owner", "disabled", "forced"] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let p = if reason == "forced" {
                Provider::Cli {
                    credits: false,
                    name: "codex".into(),
                    enabled: true,
                    cli: "codex".into(),
                    model: None,
                    daily_budget: 10,
                    timeout_s: 1,
                    limits: Default::default(),
                }
            } else {
                let mut p = stub("http://127.0.0.1:9".into());
                if let Provider::Openai {
                    name,
                    enabled,
                    daily_budget,
                    ..
                } = &mut p
                {
                    *name = "codex".into();
                    *enabled = reason != "disabled";
                    *daily_budget = Some(if reason == "budget" { 0 } else { 10 });
                }
                p
            };
            if reason == "owner" {
                providers_db::set_state(
                    &conn,
                    "codex",
                    providers_db::State {
                        down_until: providers_db::OWNER_HOLD,
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            let reads = Rc::new(Cell::new(0));
            let counted = Rc::clone(&reads);
            CODEX_REST_TEST.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |_| {
                    counted.set(counted.get() + 1);
                    None
                }))
            });
            let gates = Cell::new(0);
            let gate = || {
                gates.set(gates.get() + 1);
                anyhow::bail!("must not dispatch")
            };
            let result = probe_attempt(&conn, &p, 5.0, &gate, reason == "forced", None).unwrap();
            CODEX_REST_TEST.with(|slot| slot.borrow_mut().take());
            assert_eq!(
                result["code"],
                match reason {
                    "owner" => "owner_hold",
                    "forced" => "forced_failure",
                    other => other,
                }
            );
            assert_eq!(reads.get(), 0);
            assert_eq!(gates.get(), 0);
            assert_eq!(
                providers_db::calls_in_a_day(&conn, "codex").unwrap().0,
                u32::from(reason == "forced")
            );
            if reason == "owner" {
                assert_eq!(
                    providers_db::state(&conn, "codex").unwrap().down_until,
                    providers_db::OWNER_HOLD
                );
            }
        }
    }

    #[test]
    fn explicit_probe_gate_errors_refund_the_proved_unsent_reservation() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let peer = providers_db::open(home.path()).unwrap();
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let mut p = stub(format!("http://{}", target.local_addr().unwrap()));
        if let Provider::Openai { limits, .. } = &mut p {
            limits.usd_per_mtok_in = 1.0;
            limits.usd_per_mtok_out = 1.0;
        }
        let gate = || {
            assert!((providers_db::usd_this_month(&peer)? - 0.000231).abs() < 1e-12);
            providers_db::set_state(
                &peer,
                "stub",
                providers_db::State {
                    down_until: providers_db::OWNER_HOLD,
                    ..Default::default()
                },
            )?;
            anyhow::bail!("stale synthetic selection")
        };
        assert_eq!(
            probe(&conn, &p, 5.0, &gate).unwrap_err().to_string(),
            "stale synthetic selection"
        );
        assert!(outcomes(&conn).is_empty());
        assert_eq!(providers_db::usd_this_month(&peer).unwrap(), 0.0);
        assert_eq!(
            providers_db::state(&peer, "stub").unwrap().down_until,
            providers_db::OWNER_HOLD
        );
        assert!(matches!(target.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn explicit_probe_rejects_header_overrides_before_gate_or_key_read() {
        for name in [
            "Host",
            "Authorization",
            "Proxy-Authorization",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "Content-Type",
            "Cookie",
            "X-Bad\r\nHeader",
        ] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let mut p = stub("http://127.0.0.1:9".into());
            if let Provider::Openai {
                headers, key_file, ..
            } = &mut p
            {
                headers.insert(name.into(), "private-header-canary".into());
                *key_file = Some(home.path().join("NONEXISTENT_KEY.md"));
            }
            let result = probe(&conn, &p, 5.0, &|| anyhow::bail!("must not reach gate")).unwrap();
            assert_eq!(result["code"], "unsafe_headers");
            assert!(!result.to_string().contains("canary"));
            assert!(outcomes(&conn).is_empty());
        }
    }

    /// A codex in `dir` whose last message is the bytes of the file `answer` beside it, and what
    /// it answers when asked for `schema`.
    #[cfg(unix)]
    fn synthetic_codex(dir: &Path) -> impl Fn(&Value, &[u8]) -> Result<Answer, CallError> {
        use std::os::unix::fs::PermissionsExt;
        let executable = dir.join("synthetic-codex");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, pathlib, sys
args = sys.argv[1:]
sys.stdin.read()
pathlib.Path(args[args.index('-o') + 1]).write_bytes(pathlib.Path(__file__).with_name('answer').read_bytes())
print(json.dumps({"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":4}}))
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        move |schema, answer| {
            std::fs::write(executable.with_file_name("answer"), answer).unwrap();
            cli_headless_at(
                "codex",
                None,
                2,
                CURATOR_SYSTEM,
                PROBE_PROMPT,
                schema,
                None,
                Some(&executable),
            )
        }
    }

    #[test]
    #[cfg(unix)]
    fn codex_rejects_an_oversized_last_message_with_a_valid_prefix() {
        let home = tempfile::tempdir().unwrap();
        let ask = synthetic_codex(home.path());
        for (schema, prefix) in [
            (probe_schema(), json!({"ok": true})),
            (
                crate::curate::schema(),
                json!({"claims": [], "summary": "s", "observations": []}),
            ),
        ] {
            let mut answer = prefix.to_string().into_bytes();
            answer.resize(MAX_RESPONSE_BYTES as usize, b' ');
            answer.extend_from_slice(b"private-overflow-canary");
            let error = ask(&schema, &answer)
                .expect_err("a valid prefix does not make an oversized result valid");
            assert!(error.invalid(), "{prefix}: {}", error.message);
            assert!(!error.message.contains("canary"));
        }
    }

    /// Codex's last message is the model's answer under `--output-schema`, not an envelope (four
    /// of four calls on 2026-11-12), so it is the answer whole, whatever was asked. It was taken
    /// whole only with the curator's `claims` or `summary` in it, and every answer of codex's to
    /// the summary role and to the My Work jobs that ask for neither was dropped as
    /// `no structured_output in codex response`.
    #[test]
    #[cfg(unix)]
    fn codex_last_message_is_the_answer_whatever_was_asked() {
        let home = tempfile::tempdir().unwrap();
        let ask = synthetic_codex(home.path());
        let summary = json!({"skip": false, "request": "r", "investigated": "", "learned": "l",
            "completed": "", "next_steps": "", "notes": ""});
        for (schema, answer) in [
            (probe_schema(), json!({"ok": true})),
            (crate::turns::answer_schema(), summary),
            (
                json!({"type": "object", "required": ["groups", "ignored"]}),
                json!({"groups": [], "ignored": []}),
            ),
            (
                crate::curate::schema(),
                json!({"claims": [], "summary": "s", "observations": []}),
            ),
        ] {
            let got = ask(&schema, answer.to_string().as_bytes())
                .unwrap_or_else(|e| panic!("{answer}: {}", e.message));
            assert_eq!(got.value, answer);
            assert!(fits(&got.value, &schema), "{answer}");
        }
        // An answer to another question comes back as it is: the chain's check refuses it, the
        // schema here and the summary role's own in `turns`.
        let other = ask(&crate::curate::schema(), br#"{"ok":true}"#).unwrap();
        assert_eq!(other.value, json!({"ok": true}));
        assert!(!fits(&other.value, &crate::curate::schema()));
        // What is no JSON is invalid output, and none of it is in the message.
        let prose = ask(&crate::turns::answer_schema(), b"private-prose-canary").unwrap_err();
        assert!(prose.invalid(), "{}", prose.message);
        assert!(!prose.message.contains("canary"), "{}", prose.message);
    }

    #[test]
    fn explicit_probe_rejects_redirects_and_http_refusals_without_retry() {
        for status in [
            "302 Found",
            "307 Temporary Redirect",
            "429 Too Many Requests",
        ] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            target.set_nonblocking(true).unwrap();
            let headers = Box::leak(
                format!(
                    "Location: http://{}/private\r\nRetry-After: 0\r\n",
                    target.local_addr().unwrap()
                )
                .into_boxed_str(),
            );
            let (url, request) = serve(
                status,
                br#"{"error":{"message":"provider-private-canary"}}"#.to_vec(),
                headers,
            );
            let mut p = stub(url);
            let file = home.path().join("TEST_KEY.md");
            std::fs::write(&file, "# synthetic only\nprobe-test-key\n").unwrap();
            if let Provider::Openai {
                key_file,
                retry_429,
                ..
            } = &mut p
            {
                *key_file = Some(file);
                *retry_429 = true;
            }
            let gates = std::cell::Cell::new(0);
            let gate = || {
                gates.set(gates.get() + 1);
                Ok(None)
            };
            let result = probe(&conn, &p, 5.0, &gate).unwrap();
            assert_eq!(result["status"], "failed");
            assert_eq!(result["http_status"], status[..3].parse::<u16>().unwrap());
            assert_eq!(gates.get(), 1);
            assert_eq!(outcomes(&conn), ["error"]);
            assert_eq!(providers_db::calls_in_a_day(&conn, "stub").unwrap().0, 1);
            let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer probe-test-key")
            );
            assert!(
                matches!(target.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
            for text in [
                result.to_string(),
                providers_db::last_calls(&conn, 1).unwrap()[0].clone(),
            ] {
                assert!(!text.contains("canary"));
                assert!(!text.contains("probe-test-key"));
            }
            assert_eq!(providers_db::usd_this_month(&conn).unwrap(), 0.0);
        }
    }

    /// #653: a connection test refused for its key leaves the chain's vetted detail, so the rest it
    /// sets names the key and its remedy where doctor and a session's start read it; another
    /// refusal keeps a status and a vetted code, never the provider's words (issue #91).
    #[test]
    fn a_connection_test_refused_for_its_key_names_the_cause_of_its_rest() {
        let cases = [
            ("401 Unauthorized", "invalid_api_key", Some(Cause::Key)),
            ("500 Internal Server Error", "server_error", None),
        ];
        for (status, code, cause) in cases {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let body = json!({"error": {"code": code, "message": "provider-private-canary"}});
            let (url, _) = serve(status, body.to_string().into_bytes(), "");
            let result = probe(&conn, &stub(url), 5.0, &|| Ok(None)).unwrap();
            assert_eq!(result["status"], "failed");
            let detail = providers_db::last_detail(&conn, "stub").unwrap().unwrap();
            assert_eq!(detail, format!("http {}: {code}", &status[..3]));
            assert_eq!(Cause::of(&detail), cause, "{detail}");
            let resting = providers_db::stopped(&conn).unwrap();
            assert!(resting.iter().any(|(p, _)| p == "stub"), "{resting:?}");
        }
    }

    #[test]
    fn explicit_probe_accepts_only_the_exact_result_and_keeps_usage_on_failure() {
        for content in [
            "{\"ok\":false}",
            "{\"ok\":true,\"private\":\"canary\"}",
            "true",
            "private-canary",
        ] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let body = json!({"choices": [{"message": {"content": content}}], "usage": {"prompt_tokens": 12, "completion_tokens": 4}});
            let (url, _) = serve_once(body.to_string().into_bytes(), "");
            let mut p = stub(url);
            if let Provider::Openai { limits, .. } = &mut p {
                limits.usd_per_mtok_in = 1.0;
                limits.usd_per_mtok_out = 1.0;
            }
            let result = probe(&conn, &p, 5.0, &|| Ok(None)).unwrap();
            assert_eq!(result["code"], "invalid");
            assert!(!result.to_string().contains("canary"));
            assert_eq!(outcomes(&conn), ["invalid"]);
            assert_eq!(providers_db::tokens_since(&conn, "stub", 0).unwrap().0, 16);
            assert!((providers_db::usd_this_month(&conn).unwrap() - 0.000016).abs() < 1e-12);
            assert!(!providers_db::last_calls(&conn, 1).unwrap()[0].contains("canary"));
        }
    }

    #[test]
    fn a_probe_cannot_send_when_the_normal_legacy_fallback_exhausts_the_day() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut p = stub(format!("http://{}", listener.local_addr().unwrap()));
        if let Provider::Openai { limits, .. } = &mut p {
            limits.daily_tokens = Some(4_100);
            limits.max_output_tokens = 4_000;
            limits.usd_per_mtok_in = 1.0;
        }
        providers_db::record(
            &conn,
            &providers_db::Call {
                provider: "stub",
                role: "curator",
                span: "legacy",
                outcome: "error",
                ms: 1,
                detail: Some("transport"),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            anyhow::bail!("legacy budget must refuse before dispatch")
        };
        let result =
            probe(&conn, &p, 5.0, &gate).expect("legacy usage is checked before the send gate");
        assert_eq!(result["status"], "blocked");
        assert_eq!(result["code"], "budget");
        assert_eq!(gates.get(), 0);
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert_eq!(
            providers_db::last_calls(&conn, 1).unwrap(),
            ["stub curator error 1ms transport"]
        );
        assert_eq!(providers_db::calls_in_a_day(&conn, "stub").unwrap().0, 1);
    }

    #[test]
    fn legacy_fallback_is_shared_while_a_normal_request_is_in_flight() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let peer = providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}], "usage": {"prompt_tokens": 3, "completion_tokens": 7}});
        let (url, normal_request) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { limits, .. } = &mut p {
            limits.daily_tokens = Some(8_200);
            limits.max_output_tokens = 4_000;
            limits.usd_per_mtok_in = 1.0;
            limits.usd_per_mtok_out = 1.0;
        }
        providers_db::record(
            &conn,
            &providers_db::Call {
                provider: "stub",
                role: "curator",
                span: "legacy",
                outcome: "error",
                ms: 1,
                detail: Some("transport"),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let gates = std::cell::Cell::new(0);
        let probe_gate = || {
            gates.set(gates.get() + 1);
            anyhow::bail!("in-flight allowance must refuse the probe")
        };
        let gate = || {
            // Legacy 4,100 plus the normal in-flight 4,003 leaves 97, not the probe's 231.
            let result = probe(&peer, &p, 5.0, &probe_gate)?;
            assert_eq!(result["status"], "blocked");
            assert_eq!(result["code"], "budget");
            assert!(
                result["retry_at"]
                    .as_i64()
                    .is_some_and(|t| t < db::now_ms() + 2_000)
            );
            Ok(None)
        };
        Chain::new(std::slice::from_ref(&p), &conn)
            .gate(&gate)
            .run("curator", "normal", "synthetic", &json!({}))
            .unwrap();
        assert_eq!(gates.get(), 0);
        assert!(normal_request.recv_timeout(Duration::from_secs(2)).is_ok());
        // The worker reports only ten tokens, releasing its unused reservation; one later
        // explicit probe can send its own small request while preserving the legacy fallback.
        let answer = json!({"choices": [{"message": {"content": "{\"ok\":true}"}}], "usage": {"prompt_tokens": 12, "completion_tokens": 4}});
        let (url, probe_request) = serve_once(answer.to_string().into_bytes(), "");
        if let Provider::Openai { base_url, .. } = &mut p {
            *base_url = url;
        }
        assert_eq!(probe(&peer, &p, 5.0, &|| Ok(None)).unwrap()["status"], "ok");
        let request = probe_request.recv_timeout(Duration::from_secs(2)).unwrap();
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(outcomes(&peer), ["error", "ok", "ok"]);
    }

    #[test]
    fn explicit_probe_budget_counts_the_fixed_schema_and_request_envelope() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let mut p = stub("http://127.0.0.1:9".into());
        if let Provider::Openai { limits, .. } = &mut p {
            limits.usd_per_mtok_in = 1.0;
            limits.usd_per_mtok_out = 1.0;
        }
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            anyhow::bail!("budget should have refused before dispatch")
        };
        // Prompt alone plus 128 output tokens would cost $0.000141, but the fixed schema and
        // request envelope also take input tokens, so $0.000150 cannot cover this probe.
        let result = probe(&conn, &p, 0.00015, &gate).unwrap();
        assert_eq!(result["code"], "budget");
        assert_eq!(gates.get(), 0);
        assert!(outcomes(&conn).is_empty());
    }

    #[test]
    fn probe_preparation_rejects_cli_models_that_can_be_flags() {
        for model in ["--settings", "-p", "", "\nprivate", &"m".repeat(201)] {
            let p = Provider::Cli {
                credits: false,
                name: "fixture".into(),
                enabled: true,
                cli: "claude".into(),
                model: Some(model.into()),
                daily_budget: 10,
                timeout_s: 1,
                limits: Default::default(),
            };
            assert!(probe_provider(&p).is_err(), "invalid model accepted");
        }
    }

    #[test]
    fn probe_preparation_rejects_ambiguous_numeric_hosts() {
        for endpoint in [
            "https://127.1/v1",
            "https://999.999.999.999/v1",
            "https://123/v1",
        ] {
            assert!(
                probe_provider(&stub(endpoint.into())).is_err(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn explicit_probe_sends_only_the_fixed_fixture_and_returns_no_provider_payload() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let body = json!({"choices": [{"message": {"content": "{\"ok\":true}"}}],
            "private": "provider-body-canary", "usage": {"prompt_tokens": 12, "completion_tokens": 4}});
        let (url, request) = serve_once(body.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai {
            extra,
            headers,
            retry_429,
            ..
        } = &mut p
        {
            *retry_429 = true;
            *extra = json!({"messages": [{"content": "native-private-message"}],
                "model": "native-model-override", "models": ["fallback-model"],
                "provider": {"allow_fallbacks": true}, "max_tokens": 9000,
                "max_completion_tokens": 9000, "stream": true, "tools": [{"type":"function"}],
                "response_format": {"type": "text"}})
            .as_object()
            .unwrap()
            .clone();
            headers.insert("x-opencode-session".into(), "synthetic-session".into());
        }
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            Ok(None)
        };
        let result = probe(&conn, &p, 5.0, &gate).unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["http_status"], 200);
        assert_eq!(gates.get(), 1);
        assert!(!result.to_string().contains("canary"));
        assert!(result.get("output").is_none());
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            request
                .to_lowercase()
                .contains("x-opencode-session: synthetic-session")
        );
        let sent: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            sent["messages"],
            json!([{"role": "user", "content": PROBE_PROMPT}])
        );
        assert_eq!(sent["model"], "m");
        assert_eq!(sent["max_tokens"], 128);
        assert_eq!(sent["stream"], false);
        assert_eq!(
            sent["response_format"]["json_schema"]["schema"],
            probe_schema()
        );
        for forbidden in ["models", "provider", "tools", "max_completion_tokens"] {
            assert!(sent.get(forbidden).is_none(), "{forbidden}");
        }
        assert_eq!(outcomes(&conn), ["ok"]);
    }

    /// Decision 80: a request to OpenAI's own API is shaped as its models take one:
    /// `max_completion_tokens` in place of `max_tokens`; for the models that take
    /// `reasoning_effort: "none"`, that and the temperature, for the others neither; for GPT-6 no
    /// implicit cache write; an entry's `extra` wins; any other host's request as before.
    #[test]
    fn an_openai_request_is_shaped_as_its_models_take_it() {
        let schema = json!({"type": "object"});
        let max = json!({"max_tokens": 4000});
        let max = max.as_object().unwrap();
        let shaped = |model: &str, extra: &serde_json::Map<String, Value>| {
            openai_request(model, "p", &schema, extra, true)
        };
        let luna = shaped("gpt-6-luna", max);
        assert_eq!(
            [
                &luna["reasoning_effort"],
                &luna["temperature"],
                &luna["max_completion_tokens"]
            ],
            [&json!("none"), &json!(0.2), &json!(4000)]
        );
        assert_eq!(luna["prompt_cache_options"], json!({"mode": "explicit"}));
        assert!(luna.get("max_tokens").is_none(), "{luna}");
        assert_eq!(
            shaped("gpt-6.1-terra-2026-10-01", max)["reasoning_effort"],
            "none"
        );
        // GPT-6.1 Sol takes no `none`: no reasoning effort is sent, nor a temperature.
        let sol = shaped("gpt-6.1-sol", max);
        assert!(
            sol.get("reasoning_effort").is_none() && sol.get("temperature").is_none(),
            "{sol}"
        );
        assert_eq!(sol["prompt_cache_options"], json!({"mode": "explicit"}));
        // An older model: no cache option, no temperature, its bound renamed.
        let older = shaped("gpt-4.1", max);
        assert!(
            older.get("prompt_cache_options").is_none() && older.get("temperature").is_none(),
            "{older}"
        );
        assert_eq!(older["max_completion_tokens"], 4000);
        // A name only like GPT-6's: no cache option (#861).
        assert!(shaped("gpt-60", max).get("prompt_cache_options").is_none());
        // The entry's own `extra` wins: an effort of its own sends no temperature, and its own bound
        // is kept.
        let own =
            json!({"reasoning_effort": "low", "max_completion_tokens": 900, "max_tokens": 4000});
        let low = shaped("gpt-6-luna", own.as_object().unwrap());
        assert_eq!(
            [&low["reasoning_effort"], &low["max_completion_tokens"]],
            [&json!("low"), &json!(900)]
        );
        assert!(
            low.get("temperature").is_none() && low.get("max_tokens").is_none(),
            "{low}"
        );
        // Another host: as before.
        let other = openai_request("gpt-6-luna", "p", &schema, max, false);
        assert_eq!(
            [&other["temperature"], &other["max_tokens"]],
            [&json!(0.2), &json!(4000)]
        );
        assert!(
            other.get("reasoning_effort").is_none() && other.get("prompt_cache_options").is_none(),
            "{other}"
        );
    }

    /// Decision 86: a fenced prompt to OpenAI's own API marks one explicit cache breakpoint at the
    /// end of its instructions, the same bytes for every call of its kind, so a later call reads
    /// them at its model's cached rate; the record after the breakpoint is neither cached nor
    /// written. An older model, another host, an unfenced prompt and one with no instructions before
    /// its fence sentence send the prompt as one string.
    #[test]
    fn a_fenced_prompts_instructions_end_at_an_openai_cache_breakpoint() {
        let schema = json!({"type": "object"});
        let none = serde_json::Map::new();
        let sentence = crate::curate::FENCE_SENTENCE;
        let fenced = format!(
            "Extract the claims.\n{sentence}=== RECORD a ===`.\n\n=== RECORD a ===\nL1 hi\n=== RECORD a ==="
        );
        let at = fenced.find(sentence).unwrap();
        let luna = openai_request("gpt-6-luna", &fenced, &schema, &none, true);
        let content = &luna["messages"][0]["content"];
        assert_eq!(
            content,
            &json!([
                {"type": "text", "text": &fenced[..at],
                    "prompt_cache_breakpoint": {"mode": "explicit"}},
                {"type": "text", "text": &fenced[at..]},
            ]),
            "{luna}"
        );
        assert_eq!(luna["prompt_cache_options"], json!({"mode": "explicit"}));
        for body in [
            openai_request("gpt-4.1", &fenced, &schema, &none, true),
            openai_request("gpt-6-luna", &fenced, &schema, &none, false),
            openai_request("gpt-6-luna", "p", &schema, &none, true),
            openai_request("gpt-6-luna", &fenced[at..], &schema, &none, true),
        ] {
            assert!(body["messages"][0]["content"].is_string(), "{body}");
        }
    }

    /// Decision 80: the connection test to OpenAI's own API sends the same shape, its 128-token
    /// bound as `max_completion_tokens`, and its cost counts the tokens its usage says were written
    /// to the cache; another host's request and cost are as before, a written count in its usage
    /// left out (Codex's review of 80a).
    #[test]
    fn an_openai_connection_test_sends_its_shape_and_counts_its_cache_writes() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let body = json!({"choices": [{"message": {"content": "{\"ok\":true}"}}],
            "usage": {"prompt_tokens": 1200, "completion_tokens": 4,
                "prompt_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 1100}}});
        let priced = |url: String| {
            let mut p = stub(url);
            if let Provider::Openai { model, limits, .. } = &mut p {
                *model = "gpt-6-luna".into();
                limits.usd_per_mtok_in = 1.0;
                limits.usd_per_mtok_out = 1.0;
            }
            p
        };
        let usd = |result: &Value| result["usd"].as_f64().unwrap_or(f64::NAN);
        let (url, request) = serve_once(body.to_string().into_bytes(), "");
        config::OPENAI_AT.set(Some(url.clone()));
        let result = probe(&conn, &priced(url), 5.0, &|| Ok(None)).unwrap();
        config::OPENAI_AT.set(None);
        assert_eq!(result["status"], "ok", "{result}");
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        let sent: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            [
                &sent["max_completion_tokens"],
                &sent["reasoning_effort"],
                &sent["temperature"]
            ],
            [&json!(128), &json!("none"), &json!(0.2)]
        );
        assert!(sent.get("max_tokens").is_none(), "{sent}");
        assert_eq!(sent["prompt_cache_options"], json!({"mode": "explicit"}));
        // 100 fresh and 1,100 written at 1.25 times, and 4 out, at USD 1 a million: USD 0.001479.
        assert!((usd(&result) - 0.001479).abs() < 1e-12, "{result}");
        let (url, request) = serve_once(body.to_string().into_bytes(), "");
        let result = probe(&conn, &priced(url), 5.0, &|| Ok(None)).unwrap();
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        let sent: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            [&sent["max_tokens"], &sent["temperature"]],
            [&json!(128), &json!(0.2)]
        );
        // Its 1,200 prompt tokens once, and 4 out: USD 0.001204.
        assert!((usd(&result) - 0.001204).abs() < 1e-12, "{result}");
    }

    #[test]
    fn a_429_retry_wait_holds_other_senders_until_its_reset() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let peer = providers_db::open(home.path()).unwrap();
        let (url, sent) = serve(
            "429 Too Many Requests",
            b"{}".to_vec(),
            "Retry-After: 1\r\n",
        );
        let mut p = stub(url);
        if let Provider::Openai { retry_429, .. } = &mut p {
            *retry_429 = true;
        }
        let during = std::thread::scope(|scope| {
            let worker_p = p.clone();
            let worker = scope.spawn(move || {
                let gates = std::cell::Cell::new(0);
                let gate = || {
                    gates.set(gates.get() + 1);
                    anyhow::ensure!(gates.get() == 1, "finished retry fixture");
                    Ok(None)
                };
                Chain::new(&[worker_p], &conn).gate(&gate).run(
                    "curator",
                    "s",
                    "synthetic",
                    &json!({}),
                )
            });
            sent.recv_timeout(Duration::from_secs(2)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            while !providers_db::last_calls(&peer, 1).unwrap()[0].contains(" wait ") {
                assert!(Instant::now() < deadline, "first response was not settled");
                std::thread::sleep(Duration::from_millis(1));
            }
            let during = budget::reserve(&peer, &p, "probe", "concurrent", 3, 5.0, &[]).unwrap();
            assert!(worker.join().unwrap().is_err());
            during
        });
        assert!(
            matches!(
                during,
                Err(budget::Refusal {
                    skip: Skip::Wait(_),
                    ..
                })
            ),
            "{during:?}"
        );
    }

    #[test]
    fn a_429_retry_settles_then_reserves_and_checks_a_fresh_gate() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let peer = providers_db::open(home.path()).unwrap();
        let (url, sent) = serve(
            "429 Too Many Requests",
            b"{}".to_vec(),
            "Retry-After: 0\r\n",
        );
        let mut p = stub(url);
        if let Provider::Openai {
            limits, retry_429, ..
        } = &mut p
        {
            *retry_429 = true;
            limits.usd_per_mtok_in = 1.0;
            limits.usd_per_mtok_out = 1.0;
            limits.max_output_tokens = 100;
        }
        let gates = std::cell::Cell::new(0);
        let gate = || {
            gates.set(gates.get() + 1);
            // Only one paid allowance remains in flight, including at the retry gate.
            assert!((providers_db::usd_this_month(&peer)? - 0.000103).abs() < 1e-12);
            if gates.get() == 2 {
                assert_eq!(outcomes(&peer), ["wait", "reserved"]);
                anyhow::bail!("retry gate refused");
            }
            Ok(None)
        };
        let mut events = Vec::new();
        let error = Chain::new(&[p], &conn)
            .paid_cap(0.00015)
            .gate(&gate)
            .run_report("curator", "s", "synthetic", &json!({}), &mut |event| {
                events.push(*event)
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "retry gate refused");
        assert_eq!(gates.get(), 2);
        assert!(sent.recv_timeout(Duration::from_secs(2)).is_ok());
        assert_eq!(outcomes(&peer), ["wait"]);
        assert_eq!(providers_db::usd_this_month(&peer).unwrap(), 0.0);
        assert_eq!(providers_db::calls_in_a_day(&peer, "stub").unwrap().0, 1);
        assert!(
            matches!(events.as_slice(), [
            AttemptEvent::Reserved { attempt: first, usd_bound: Some(first_usd) },
            AttemptEvent::Sent { attempt: sent },
            AttemptEvent::Settled { attempt: settled, outcome: AttemptOutcome::Wait,
                sent: true, usd: None, usage_unknown: true },
            AttemptEvent::Reserved { attempt: retry, usd_bound: Some(retry_usd) },
            AttemptEvent::Cancelled { attempt: cancelled },
        ] if first == sent && first == settled && retry == cancelled && first != retry
            && (*first_usd - 0.000103).abs() < 1e-12
            && (*retry_usd - 0.000103).abs() < 1e-12),
            "own retry receipt: {events:?}"
        );
    }

    #[test]
    fn chain_settlement_uses_state_written_while_the_request_was_in_flight() {
        for held in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let peer = providers_db::open(home.path()).unwrap();
            let body = json!({"choices": [{"message": {"content": "{}"}}]});
            let (url, _) = serve(
                if held { "200 OK" } else { "400 Bad Request" },
                body.to_string().into_bytes(),
                "",
            );
            let latest = providers_db::State {
                down_until: if held { providers_db::OWNER_HOLD } else { 0 },
                fails: 1,
                backoff: 0,
                ..Default::default()
            };
            let gate = || {
                providers_db::set_state(&peer, "stub", latest)?;
                Ok(None)
            };
            let result = Chain::new(&[stub(url)], &conn).gate(&gate).run(
                "curator",
                "s",
                "synthetic",
                &json!({}),
            );
            assert_eq!(result.is_ok(), held);
            let state = providers_db::state(&peer, "stub").unwrap();
            if held {
                assert_eq!(state, latest);
            } else {
                assert_eq!(state.fails, 2);
            }
        }
    }

    #[test]
    fn an_answer_clears_an_expired_cooldown() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        providers_db::set_state(
            &conn,
            "stub",
            providers_db::State {
                down_until: db::now_ms() - 1,
                fails: 1,
                backoff: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        Chain::new(&[stub(url)], &conn)
            .run("curator", "s", "synthetic", &json!({}))
            .unwrap();
        assert_eq!(
            providers_db::state(&conn, "stub").unwrap(),
            providers_db::State::default()
        );
    }

    #[test]
    fn a_disabled_entry_cannot_dispatch_through_a_direct_chain() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]});
        let (url, sent) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { enabled, .. } = &mut p {
            *enabled = false;
        }
        let error = Chain::new(&[p], &conn)
            .run("curator", "disabled", "synthetic", &json!({}))
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<ChainFailed>().unwrap().0[0].skip,
            Skip::Owner
        );
        assert!(sent.try_recv().is_err());
        assert!(outcomes(&conn).is_empty());
    }

    #[test]
    fn a_chain_reserves_before_dispatch_on_an_independent_connection() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let peer = providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { daily_budget, .. } = &mut p {
            *daily_budget = Some(1);
        }
        let gate = || {
            let refusal = budget::admit_with_history(&peer, (&p, &p), "curator", 1.0, 5.0, &[])?;
            assert!(
                refusal.is_some(),
                "the in-flight call must take the only daily slot"
            );
            assert!(matches!(refusal.unwrap().skip, Skip::Wait(_)));
            Ok(None)
        };
        Chain::new(std::slice::from_ref(&p), &conn)
            .gate(&gate)
            .run("curator", "s", "synthetic", &json!({}))
            .unwrap();
        assert_eq!(providers_db::calls_in_a_day(&peer, "stub").unwrap().0, 1);
    }

    #[test]
    fn an_answer_records_its_token_usage() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{\"summary\": \"s\"}"}}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 30}});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        Chain::new(&[stub(url)], &conn)
            .run("curator", "s", "p", &json!({"type": "object"}))
            .unwrap();
        let row: (String, Option<i64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT outcome, prompt_tokens, completion_tokens, cached_tokens FROM provider_calls",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("ok".into(), Some(120), Some(30), None));
    }

    #[test]
    fn http_answers_are_parsed_and_capped() {
        let answer = json!({"choices": [{"message": {"content": "{\"summary\":\"s\",\"observations\":[]}"}}]});
        let (url, request) = serve_once(answer.to_string().into_bytes(), "");
        // OpenCode Go refuses a request without its session header (HTTP 400 MissingSessionID).
        let headers = [("x-opencode-session".to_string(), "oboete".to_string())].into();
        let Answer { value: v, .. } = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &headers,
            "p",
            &json!({}),
            None,
        )
        .unwrap();
        assert_eq!(v["summary"], "s");
        let request = request.recv().unwrap().to_lowercase();
        assert!(request.contains("x-opencode-session: oboete"), "{request}");
        // OpenCode Go's glm-5.3-flash fenced its JSON in 3 of 4 calls under a strict schema.
        for content in [
            "```json\n{\"summary\":\"f\",\"observations\":[]}\n```",
            " ```\n{\"summary\":\"f\",\"observations\":[]}\n```\n",
        ] {
            let answer = json!({"choices": [{"message": {"content": content}}]});
            let (url, _) = serve_once(answer.to_string().into_bytes(), "");
            let Answer { value: v, .. } = openai_compat(
                &url,
                None,
                "m",
                10,
                &Default::default(),
                &Default::default(),
                "p",
                &json!({}),
                None,
            )
            .unwrap();
            assert_eq!(v["summary"], "f");
        }
        let (url, _) = serve_once(vec![b' '; MAX_RESPONSE_BYTES as usize + 10], "");
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert!(e.invalid(), "{}", e.message);
        // An error answer too large to read is still an error answer, not a billed one.
        let (url, _) = serve(
            "429 Too Many Requests",
            vec![b' '; MAX_RESPONSE_BYTES as usize + 10],
            "",
        );
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(429), "http 429"));
        // 2 KB on the wire, 2 MiB once decoded.
        let bomb = include_bytes!("../testdata/two-mib-of-spaces.gz").to_vec();
        let (url, _) = serve_once(bomb, "Content-Encoding: gzip\r\n");
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert!(e.message.contains("larger than"), "{}", e.message);
    }

    #[test]
    fn error_bodies_are_never_kept() {
        // An ordinary sentence and a generation echoed back: neither is a secret pattern.
        let canary = "今日は設計の続きをします canary-91";
        let cases = [
            (
                "400 Bad Request",
                json!({"error": {"message": format!("bad request: {canary}"), "type": "invalid_request_error",
                    "code": "json_validate_failed", "failed_generation": canary}})
                .to_string(),
                "http 400: json_validate_failed",
            ),
            ("401 Unauthorized", format!("<html>{canary}</html>"), "http 401"),
            (
                "429 Too Many Requests",
                json!({"error": {"message": format!("Please try again in 1.5s. {canary}"), "code": "rate_limit_exceeded"}}).to_string(),
                "http 429: rate_limit_exceeded, retry in 2s",
            ),
            ("500 Internal Server Error", format!("{canary}\n{canary}"), "http 500"),
            (
                "403 Forbidden",
                json!({"error": {"message": format!("Your input was flagged: {canary}"), "code": 403}}).to_string(),
                "http 403: 403 (moderation)",
            ),
            (
                "400 Bad Request",
                json!({"error": {"code": format!("{canary} with spaces")}}).to_string(),
                "http 400",
            ),
            // Identifier-shaped but not a known code: user data in `code` stays out too.
            (
                "400 Bad Request",
                json!({"error": {"code": "customer-1234", "type": "invalid_request_error"}}).to_string(),
                "http 400: invalid_request_error",
            ),
            (
                "402 Payment Required",
                json!({"error": {"code": 40212345}}).to_string(),
                "http 402",
            ),
        ];
        for (status, body, want) in cases {
            let (url, _) = serve(status, body.into_bytes(), "");
            let e = openai_compat(
                &url,
                None,
                "m",
                10,
                &Default::default(),
                &Default::default(),
                "p",
                &json!({}),
                None,
            )
            .unwrap_err();
            assert_eq!(e.message, want);
        }
        // The 429's wait is still read from the body; moderation still gets no cooldown.
        let (url, _) = serve(
            "429 Too Many Requests",
            b"{\"error\": {\"message\": \"Please try again in 1.5s.\"}}".to_vec(),
            "",
        );
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert_eq!(e.retry_after_s, Some(1.5));
        // A redirect whose Location quotes the canary fails inside ureq: its text is not kept.
        let (url, _) = serve(
            "302 Found",
            Vec::new(),
            "Location: canary-91-location/../../../../../../../\r\n",
        );
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert!(!e.message.contains("canary"), "{}", e.message);
        assert!(e.message.starts_with("http request: "), "{}", e.message);
        // A permission error quoting a generation that says "flagged" is not moderation.
        let (url, _) = serve(
            "403 Forbidden",
            json!({"error": {"message": "not allowed for this key", "failed_generation": "the item was flagged"}})
                .to_string()
                .into_bytes(),
            "",
        );
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert_eq!(e.message, "http 403");
        // A negative Retry-After is ignored, not a panic in the chain's sleep.
        let (url, _) = serve(
            "429 Too Many Requests",
            b"{}".to_vec(),
            "Retry-After: -1\r\n",
        );
        let e = openai_compat(
            &url,
            None,
            "m",
            10,
            &Default::default(),
            &Default::default(),
            "p",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert_eq!(e.retry_after_s, None);
        let flagged = CallError {
            status: Some(403),
            retry_after_s: None,
            message: "http 403 (moderation)".into(),
            usage: Usage::default(),
            sent: true,
            cool_until: None,
            rate: None,
            uncredited: false,
        };
        assert_eq!(cooldown_for(&flagged), None);
    }

    /// Spec 5.5: the gate is asked before each call, and its refusal ends the run: the next entry
    /// is never called.
    #[test]
    fn a_gate_that_refuses_stops_the_chain_before_its_next_call() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let entry = |name: &str, base_url: String| Provider::Openai {
            enabled: true,
            name: name.into(),
            base_url,
            api: Default::default(),
            key_file: None,
            model: "m".into(),
            daily_budget: Some(10),
            timeout_s: 10,
            retry_429: false,
            extra: Default::default(),
            headers: Default::default(),
            limits: Default::default(),
            subscription: false,
        };
        let (failing, first) = serve("500 Internal Server Error", b"{}".to_vec(), "");
        let (answering, second) = serve("200 OK", b"{}".to_vec(), "");
        let providers = [entry("one", failing), entry("two", answering)];
        let asked = std::cell::Cell::new(0);
        let gate = || {
            asked.set(asked.get() + 1);
            anyhow::ensure!(asked.get() == 1, "the list changed");
            Ok(None)
        };
        let Err(err) =
            Chain::new(&providers, &conn)
                .gate(&gate)
                .run("curator", "s", "p", &json!({}))
        else {
            panic!("the gate refuses the second call");
        };
        assert_eq!(format!("{err:#}"), "the list changed");
        let wait = std::time::Duration::from_secs(5);
        assert!(first.recv_timeout(wait).is_ok());
        let called: Vec<String> = conn
            .prepare("SELECT provider FROM provider_calls")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(called, ["one"]);
        assert!(second.try_recv().is_err());
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn a_failed_chain_keeps_no_error_body_in_provider_calls() {
        let canary = "要約の途中の文 canary-91-chain";
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let (url, _) = serve(
            "400 Bad Request",
            json!({"error": {"code": "json_validate_failed", "failed_generation": canary}})
                .to_string()
                .into_bytes(),
            "",
        );
        let providers = [Provider::Openai {
            enabled: true,
            name: "stub".into(),
            base_url: url,
            api: Default::default(),
            key_file: None,
            model: "m".into(),
            daily_budget: Some(10),
            timeout_s: 10,
            retry_429: false,
            extra: Default::default(),
            headers: Default::default(),
            limits: Default::default(),
            subscription: false,
        }];
        let Err(err) = Chain::new(&providers, &conn).run("curator", "s", "p", &json!({})) else {
            panic!("the stub only fails");
        };
        assert!(!format!("{err:#}").contains("canary"), "{err:#}");
        let details: Vec<String> = conn
            .prepare("SELECT COALESCE(detail, '') FROM provider_calls")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(details, ["http 400: json_validate_failed"]);
        drop(conn);
    }

    // Decision 107 (docs/curator-conversation.md, #971): the curator's windows as one
    // conversation an entry reads from its prompt cache.

    /// A Messages entry that keeps a conversation: a ceiling, Haiku 5.5's prices, calls enough.
    fn talker(url: &str, ceiling: u32) -> Provider {
        let mut p = stub(url.to_owned());
        if let Provider::Openai {
            api,
            limits,
            daily_budget,
            ..
        } = &mut p
        {
            *api = Api::Anthropic;
            *daily_budget = Some(100_000);
            limits.max_request_tokens = Some(ceiling);
            (limits.usd_per_mtok_in, limits.usd_per_mtok_out) = (0.10, 0.50);
        }
        p
    }

    /// A Messages answer whose prompt was `fresh` tokens not cached, `read` from the cache and
    /// `written` to it.
    fn messages_answer(fresh: i64, read: i64, written: i64) -> Vec<u8> {
        messages_saying("s", fresh, read, written)
    }

    /// `messages_answer` whose summary is `summary`.
    fn messages_saying(summary: &str, fresh: i64, read: i64, written: i64) -> Vec<u8> {
        let text = json!({ "summary": summary }).to_string();
        json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": text}],
            "usage": {"input_tokens": fresh, "cache_read_input_tokens": read,
                "cache_creation_input_tokens": written, "output_tokens": 4}})
        .to_string()
        .into_bytes()
    }

    type Parts = (String, std::ops::Range<usize>);

    /// A window's three-part prompt, its one line saying `line`, and where its stretch is.
    fn window_prompt(line: &str, known: &str, cards: usize) -> Parts {
        let text = format!("## claude session s\nL1 [user] {line}\n");
        crate::curate::parts("English", &text, known, "", cards, 1)
    }

    /// `parts` as the turn of a window over records `from` to `to`, under `key`.
    fn turn_of<'a>(parts: &'a Parts, key: &'a str, from: i64, to: i64) -> Turn<'a> {
        Turn {
            prompt: &parts.0,
            stretch: parts.1.clone(),
            key,
            from: (from, None),
            to,
            removed: 0,
        }
    }

    /// One run of the curator's chain as `turn`.
    fn talk_run(
        providers: &[Provider],
        conn: &Connection,
        talk: &mut Talk,
        turn: &Turn,
    ) -> Result<ChainResult> {
        let span = format!("{}-{}", turn.from.0, turn.to);
        Chain::new(providers, conn).talk(talk, turn).run(
            "curator",
            &span,
            turn.prompt,
            &json!({"type": "object"}),
        )
    }

    /// The content of the Messages request a test server received: each block's text, and
    /// whether it carries a breakpoint.
    fn blocks(request: &str) -> Vec<(String, bool)> {
        let body = request_body(request);
        let content = body["messages"][0]["content"].as_array().unwrap();
        content
            .iter()
            .map(|b| {
                let marked = b.get("cache_control").is_some();
                (b["text"].as_str().unwrap().to_owned(), marked)
            })
            .collect()
    }

    /// A three-part prompt's instructions, stretch and tail.
    fn three(parts: &Parts) -> [String; 3] {
        let (p, at) = parts;
        [&p[..at.start], &p[at.clone()], &p[at.end..]].map(str::to_owned)
    }

    /// What a server that counts `times` the estimate counts of a Messages request's first `n`
    /// blocks, and of all of them.
    fn counted(request: &str, n: Option<usize>, times: f64) -> i64 {
        let texts: Vec<String> = blocks(request).into_iter().map(|b| b.0).collect();
        let text = texts[..n.unwrap_or(texts.len())].concat();
        (f64::from(budget::estimate(&text)) * times).ceil() as i64
    }

    /// A Messages server for `calls` requests that counts `times` the estimate, and reads from
    /// its cache what a request was sent again: all before its newest stretch.
    fn counting(calls: usize, times: f64) -> (String, std::sync::mpsc::Receiver<String>) {
        serve_by(
            calls,
            move |request| {
                let sent = blocks(request).len();
                let read = if sent > 3 {
                    counted(request, Some(sent - 2), times)
                } else {
                    0
                };
                let fresh = counted(request, None, times) - read;
                ("200 OK", messages_answer(fresh, read, 0))
            },
            "",
        )
    }

    /// How many stretches went before each turn's own, as the ledger has them (None: no turn).
    fn earlier(conn: &Connection) -> Vec<Option<i64>> {
        conn.prepare("SELECT earlier FROM provider_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// #971 conditions 8, 13 and 15: an entry's second turn sends its first call's instructions
    /// and stretch again as the same blocks, byte for byte, then its own stretch, with two
    /// breakpoints: on the instructions and on the newest stretch. The tail carries none. The
    /// ledger says how many stretches went before each call's own, counts the whole prompt as
    /// sent, and prices what was read at a tenth and what was written at 1.25 times. The spans
    /// read are those of the stretches sent again, for the call's own span only.
    #[test]
    fn a_second_turn_sends_the_first_stretch_again_and_marks_the_newest() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answers = vec![
            ("200 OK", messages_answer(60, 0, 1_940)),
            ("200 OK", messages_answer(70, 1_900, 60)),
        ];
        let (url, requests) = serve_each(answers, "");
        let providers = [talker(&url, 100_000)];
        let mut talk = Talk::default();
        let one = window_prompt("One.", "", 5);
        let two = window_prompt("Two.", "u1: kept", 15);
        talk_run(&providers, &conn, &mut talk, &turn_of(&one, "k", 1, 2)).unwrap();
        assert_eq!(talk.read("stub", "1-2"), []);
        talk_run(&providers, &conn, &mut talk, &turn_of(&two, "k", 3, 4)).unwrap();
        assert_eq!(talk.read("stub", "3-4"), [(1, 2)]);
        assert_eq!(talk.read("stub", "1-2"), [], "another call's span");
        assert_eq!(talk.read("another", "3-4"), []);
        let first = requests.recv().unwrap();
        let second = requests.recv().unwrap();
        let ([instructions, stretch_1, tail_1], [_, stretch_2, tail_2]) =
            (three(&one), three(&two));
        assert_eq!(
            blocks(&first),
            [
                (instructions.clone(), true),
                (stretch_1.clone(), true),
                (tail_1, false)
            ]
        );
        assert_eq!(
            blocks(&second),
            [
                (instructions, true),
                (stretch_1, false),
                (stretch_2, true),
                (tail_2, false)
            ]
        );
        let content = &request_body(&second)["messages"][0]["content"];
        let five_minutes = json!({"type": "ephemeral"});
        assert_eq!(
            (&content[0]["cache_control"], &content[2]["cache_control"]),
            (&five_minutes, &five_minutes)
        );
        let sent: String = blocks(&second).into_iter().map(|b| b.0).collect();
        let row: (i64, i64, i64, f64) = conn
            .query_row(
                "SELECT earlier, est_tokens, bytes_out, usd FROM provider_calls
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (row.0, row.1, row.2),
            (1, i64::from(budget::estimate(&sent)), sent.len() as i64)
        );
        assert_eq!(earlier(&conn), [Some(0), Some(1)]);
        // 70 new tokens, 60 written at 1.25 times, 1,900 read at a tenth, and 4 of output.
        let usd = (70.0 + 60.0 * 1.25 + 1_900.0 * 0.1) * 0.10e-6 + 4.0 * 0.50e-6;
        assert!((row.3 - usd).abs() < 1e-12, "{} against {usd}", row.3);
    }

    /// #971 condition 9: a conversation starts again, with the turn's own stretch alone, when
    /// the turn's key is another (the gate's state changed), its instructions are others (another
    /// language), the entry's model or address changed, its last call started longer ago than
    /// the cache keeps an entry or after now, or that call was sent and came to nothing. A call
    /// that was not sent changes nothing (condition 11).
    #[test]
    fn a_conversation_starts_again_when_what_it_holds_may_not_go_again() {
        // How many stretches went before the last turn's own, after `case` happened to a
        // conversation of one turn.
        let before_the_last = |case: &str| -> usize {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let ok = || ("200 OK", messages_answer(60, 0, 1_940));
            let failing = ("500 Internal Server Error", b"{}".to_vec());
            let answers = match case {
                "error" => vec![ok(), failing, ok()],
                _ => vec![ok(), ok()],
            };
            let (url, requests) = serve_each(answers, "");
            let (other_url, other_requests) = serve_each(vec![ok()], "");
            let mut p = talker(&url, 100_000);
            let mut talk = Talk::default();
            let one = window_prompt("One.", "", 5);
            // What was removed from each window's records: one over what an op lists with the
            // first's, or as many as it lists.
            let first = Turn {
                removed: usize::from(case == "many removals"),
                ..turn_of(&one, "k", 1, 2)
            };
            talk_run(&[p.clone()], &conn, &mut talk, &first).unwrap();
            let mut two = window_prompt("Two.", "", 5);
            let mut key = "k";
            let started = &mut talk.entries.get_mut("stub").unwrap().started;
            match case {
                "none" | "few removals" | "many removals" => {}
                "key" => key = "another",
                "language" => {
                    let text = "## claude session s\nL1 [user] Two.\n";
                    two = crate::curate::parts("Japanese", text, "", "", 5, 1);
                }
                "model" | "address" => {
                    let Provider::Openai {
                        model, base_url, ..
                    } = &mut p
                    else {
                        unreachable!()
                    };
                    if case == "model" {
                        *model = "m2".into();
                    } else {
                        *base_url = other_url;
                    }
                }
                "late" => *started -= TALK_LIFETIME_MS,
                "in time" => *started -= TALK_LIFETIME_MS - 5_000,
                "clock back" => *started += 60_000,
                "error" => {
                    // The entry answers 500: the call was sent and came to nothing.
                    let failed =
                        talk_run(&[p.clone()], &conn, &mut talk, &turn_of(&two, key, 3, 4));
                    assert!(failed.is_err());
                    conn.execute("DELETE FROM provider_state", []).unwrap();
                }
                "unsent" => {
                    let gate = || anyhow::bail!("the gate is shut");
                    let unsent = Chain::new(std::slice::from_ref(&p), &conn)
                        .gate(&gate)
                        .talk(&mut talk, &turn_of(&two, key, 3, 4))
                        .run("curator", "3-4", &two.0, &json!({"type": "object"}));
                    assert!(unsent.is_err());
                }
                "no key" => {
                    // Its key file is not there: the call fails before anything is sent.
                    let mut keyless = p.clone();
                    if let Provider::Openai { key_file, .. } = &mut keyless {
                        *key_file = Some(home.path().join("no-key.md"));
                    }
                    let unsent = talk_run(&[keyless], &conn, &mut talk, &turn_of(&two, key, 3, 4));
                    assert!(unsent.is_err());
                    conn.execute("DELETE FROM provider_state", []).unwrap();
                }
                other => panic!("{other}"),
            }
            let last = Turn {
                removed: if case.ends_with("removals") {
                    TALK_REMOVED
                } else {
                    0
                },
                ..turn_of(&two, key, 3, 4)
            };
            talk_run(&[p], &conn, &mut talk, &last).unwrap();
            let all = if case == "address" {
                other_requests
            } else {
                requests
            };
            let last = all.try_iter().last().unwrap();
            let sent = blocks(&last);
            assert_eq!(sent.last().unwrap().0, three(&two)[2], "{case}");
            assert_eq!(
                talk.read("stub", "3-4").len(),
                usize::from(sent.len() == 4),
                "{case}"
            );
            sent.len() - 3
        };
        for (case, earlier) in [
            ("none", 1),
            ("in time", 1),
            ("unsent", 1),
            ("no key", 1),
            ("few removals", 1),
            ("many removals", 0),
            ("key", 0),
            ("language", 0),
            ("model", 0),
            ("address", 0),
            ("late", 0),
            ("clock back", 0),
            ("error", 0),
        ] {
            assert_eq!(before_the_last(case), earlier, "{case}");
        }
    }

    /// #971 condition 11, and a window sent again: a stretch joins its entry's conversation after
    /// an answer the check refused, as after one kept. A window sent again, grown or not, takes
    /// its own earlier stretch's place: no window's lines are in a prompt twice, and its op does
    /// not read its own span.
    #[test]
    fn a_refused_answers_stretch_joins_and_a_window_sent_again_takes_its_place() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answers = (0..4).map(|_| ("200 OK", messages_answer(60, 1_900, 60)));
        let (url, requests) = serve_each(answers.collect(), "");
        let providers = [talker(&url, 100_000)];
        let mut talk = Talk::default();
        let one = window_prompt("One.", "", 5);
        let two = window_prompt("Two.", "", 5);
        let grown = window_prompt("Two, and more.", "", 5);
        let three_ = window_prompt("Three.", "", 5);
        talk_run(&providers, &conn, &mut talk, &turn_of(&one, "k", 1, 2)).unwrap();
        // Its answer is refused, as one none of whose quotes anchored.
        let refuse = |_: &Value| Some("unanchored");
        let turn = turn_of(&two, "k", 3, 4);
        let refused = Chain::new(&providers, &conn)
            .check(&refuse)
            .talk(&mut talk, &turn)
            .run("curator", "3-4", &two.0, &json!({"type": "object"}));
        assert!(refused.is_err());
        // The same window again, grown by a record.
        talk_run(&providers, &conn, &mut talk, &turn_of(&grown, "k", 3, 5)).unwrap();
        assert_eq!(talk.read("stub", "3-5"), [(1, 2)]);
        talk_run(&providers, &conn, &mut talk, &turn_of(&three_, "k", 6, 7)).unwrap();
        assert_eq!(
            talk.read("stub", "6-7"),
            [(1, 5)],
            "next to each other: one span"
        );
        let sent: Vec<Vec<String>> = requests
            .try_iter()
            .map(|r| blocks(&r).into_iter().skip(1).map(|b| b.0).collect())
            .collect();
        let stretch = |p: &Parts| three(p)[1].clone();
        assert_eq!(sent[1][..2], [stretch(&one), stretch(&two)]);
        assert_eq!(sent[2][..2], [stretch(&one), stretch(&grown)]);
        assert_eq!(sent[2].len(), 3);
        assert_eq!(
            sent[3][..3],
            [stretch(&one), stretch(&grown), stretch(&three_)]
        );
        assert_eq!(earlier(&conn), [Some(0), Some(1), Some(1), Some(2)]);
    }

    /// #971 condition 10: no request of a conversation passes the entry's ceiling. A server that
    /// counts one and a half times the estimate, as a Messages entry about does, is sent windows
    /// of many sizes. One that counts two and a half times, and one that counts four times, from
    /// their first answer: before an entry's own answers say how it counts, an estimated token
    /// is taken for 2.6, and the tokens its last answer reported count for what they are.
    #[test]
    fn a_conversation_starts_again_before_its_prompt_passes_the_ceiling() {
        const CEILING: u32 = 100_000;
        // The most tokens of a request, and how many stretches went before each call's own, for
        // windows whose one line is `words(n)` words long.
        let drive = |times: f64, calls: usize, words: &dyn Fn(usize) -> usize| {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let (url, requests) = counting(calls, times);
            let providers = [talker(&url, CEILING)];
            let mut talk = Talk::default();
            for n in 0..calls {
                let parts = window_prompt(&"word ".repeat(words(n)), "", 5);
                let from = n as i64 * 2 + 1;
                talk_run(
                    &providers,
                    &conn,
                    &mut talk,
                    &turn_of(&parts, "k", from, from + 1),
                )
                .unwrap();
            }
            let most = requests.try_iter().map(|r| counted(&r, None, times));
            (most.max().unwrap(), earlier(&conn))
        };
        // From about 300 to about 21,000 estimated tokens a window.
        let (most, went) = drive(1.5, 40, &|n| 200 + (n * 7_919) % 15_000);
        assert!(most <= i64::from(CEILING), "{most}");
        let restarts = went.iter().skip(1).filter(|n| **n == Some(0)).count();
        assert!(restarts >= 2, "{went:?}");
        assert!(went.iter().any(|n| n.is_some_and(|n| n >= 3)), "{went:?}");
        // And it used the room it had: a restart comes when the tokens so far and twice the
        // new window's estimate (at most about 21,300 here) pass the nine tenths it may fill.
        assert!(most > 47_000, "{most}");
        // Windows of about 20,000 estimated tokens, counted two and a half times from the first:
        // each would pass the ceiling beside the one before, and starts again. A call that starts
        // again reads nothing it was sent before, and is no miss: the fourth is a turn still.
        let (most, went) = drive(2.5, 4, &|_| 14_300);
        assert!(most <= i64::from(CEILING), "{most}");
        assert_eq!(went, [Some(0), Some(0), Some(0), Some(0)]);
        // Windows of about 5,000, counted four times: the estimate alone would let the fifth go
        // on (five of them and the instructions, at 2.6); the tokens the fourth reported do not.
        let (most, went) = drive(4.0, 8, &|_| 3_570);
        assert!(most <= i64::from(CEILING), "{most}");
        assert_eq!(
            went[..5],
            [Some(0), Some(1), Some(2), Some(3), Some(0)],
            "{went:?}"
        );
    }

    /// Decision 107: for the size of a conversation an estimated token is taken for more than the
    /// entry counted of late: the most its tokens were to its estimates over its last 50 answers
    /// that said both, and a third over that; 2.6 until it has answered five times.
    #[test]
    fn the_size_of_a_conversation_takes_an_estimate_for_more_than_the_entry_counted() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answered = |provider: &str, prompt: i64| {
            let usage = Usage {
                prompt: Some(prompt),
                ..Usage::default()
            };
            let call = providers_db::Call {
                provider,
                role: "curator",
                span: "s",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: Some(1_000),
                usage,
                usd: None,
            };
            providers_db::record(&conn, &call).unwrap();
        };
        assert_eq!(ratio(&conn, "stub").unwrap(), TALK_RATIO);
        for prompt in [1_200, 1_500, 1_100, 1_300] {
            answered("stub", prompt);
        }
        assert_eq!(ratio(&conn, "stub").unwrap(), 2.6, "four answers");
        answered("stub", 1_400);
        answered("another", 9_000);
        assert_eq!(ratio(&conn, "stub").unwrap(), 1.5 * 4.0 / 3.0);
        // Only its last 50 answers: an older, larger count no longer says how it counts.
        for _ in 0..50 {
            answered("stub", 1_200);
        }
        assert_eq!(ratio(&conn, "stub").unwrap(), 1.2 * 4.0 / 3.0);
    }

    /// Decision 107: a continued call that reads too little of what it was sent again from the
    /// cache wrote it all instead, at 1.25 times. Two in a row, and the entry keeps no
    /// conversation for an hour: its calls are the prompt as it is, as an entry that never kept
    /// one gets it. A call that read its cache between two misses starts the count again.
    #[test]
    fn an_entry_whose_cache_does_not_answer_keeps_no_conversation() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        // Every answer counts its prompt 2,000 tokens. What goes again is taken at its least:
        // those less the last tail at 2.6 times its estimate; half of that read is a hit, a
        // token less a miss.
        let tail = budget::estimate(&three(&window_prompt("Window 0.", "", 5))[2]);
        let half = ((2_000.0 - f64::from(tail) * 2.6) / 2.0).ceil() as i64;
        assert!(half > 500, "{half}");
        // What each call read from the cache: a miss, a hit, then two misses.
        let read = [0, half - 1, half, half - 1, half - 1, 0, 0];
        let answers = read.map(|read| ("200 OK", messages_answer(2_000 - read, read, 0)));
        let (url, requests) = serve_each(answers.to_vec(), "");
        let providers = [talker(&url, 100_000)];
        let mut talk = Talk::default();
        let run = |talk: &mut Talk, n: i64| {
            let parts = window_prompt(&format!("Window {n}."), "", 5);
            talk_run(
                &providers,
                &conn,
                talk,
                &turn_of(&parts, "k", n * 2 + 1, n * 2 + 2),
            )
            .unwrap();
        };
        for n in 0..5 {
            run(&mut talk, n);
        }
        assert_eq!(talk.read("stub", "9-10"), [(1, 8)]);
        // The fifth window again: no turn, and it read nothing before its own this time.
        run(&mut talk, 4);
        assert_eq!(talk.read("stub", "9-10"), []);
        // The first started one; four went on (a miss, a hit, two misses); the sixth is no turn.
        assert_eq!(
            earlier(&conn),
            [Some(0), Some(1), Some(2), Some(3), Some(4), None]
        );
        let sent: Vec<Vec<(String, bool)>> = requests.try_iter().map(|r| blocks(&r)).collect();
        let plain = &sent[5];
        assert_eq!(plain.len(), 2, "its instructions and the rest");
        assert_eq!((plain[0].1, plain[1].1), (true, false));
        // An hour on, it keeps one again.
        *talk.quiet.get_mut("stub").unwrap() -= TALK_QUIET_MS;
        run(&mut talk, 6);
        assert_eq!(earlier(&conn).last(), Some(&Some(0)));
    }

    /// Decision 107: a turn is within what admission lets through too. Admission counts the whole
    /// estimate at the entry's own factor, the median of its last 50 answers: an entry that has
    /// counted twice its estimates is sent no turn its budget would refuse as too big, though the
    /// lines of this conversation count only one and a half times.
    #[test]
    fn a_turn_stays_within_what_admission_counts() {
        const CEILING: u32 = 100_000;
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        for _ in 0..50 {
            let usage = Usage {
                prompt: Some(2_000),
                ..Usage::default()
            };
            let call = providers_db::Call {
                provider: "stub",
                role: "summary",
                span: "s",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: Some(1_000),
                usage,
                usd: None,
            };
            providers_db::record(&conn, &call).unwrap();
        }
        assert_eq!(budget::factor(&conn, "stub").unwrap(), 2.0);
        let calls = 14;
        let (url, requests) = counting(calls, 1.5);
        let providers = [talker(&url, CEILING)];
        let mut talk = Talk::default();
        // About 5,000 estimated tokens a window.
        let parts = window_prompt(&"word ".repeat(3_570), "", 5);
        for n in 0..calls as i64 {
            // None is refused: each is a turn its budget admits.
            let turn = turn_of(&parts, "k", n * 2 + 1, n * 2 + 2);
            talk_run(&providers, &conn, &mut talk, &turn).unwrap();
        }
        let most = requests.try_iter().map(|r| counted(&r, None, 2.0));
        assert!(most.max().unwrap() <= i64::from(CEILING) * 9 / 10);
        let went = earlier(&conn);
        let went = &went[50..];
        assert!(went.iter().all(Option::is_some), "{went:?}");
        assert!(went.contains(&Some(6)), "{went:?}");
        assert!(went[1..].contains(&Some(0)), "{went:?}");
    }

    /// Decision 107: what a call reads from the cache is what went again: never the last call's
    /// tail, nor a stretch this window's takes the place of, however large they were. Neither is
    /// a miss: after a small window and a large one, the large one twice more, the conversation
    /// goes on with the next; and so it does through windows whose tails are large.
    #[test]
    fn a_turn_misses_nothing_by_what_does_not_go_again() {
        // How many stretches went before each call's own, and whether the entry still keeps one.
        let went = |windows: &[(&Parts, i64)]| {
            let home = tempfile::tempdir().unwrap();
            let conn = providers_db::open(home.path()).unwrap();
            let (url, _requests) = counting(windows.len(), 1.7);
            let providers = [talker(&url, 100_000)];
            let mut talk = Talk::default();
            for (parts, from) in windows {
                let turn = turn_of(parts, "k", *from, from + 1);
                talk_run(&providers, &conn, &mut talk, &turn).unwrap();
            }
            (earlier(&conn), talk.quiet.is_empty())
        };
        let small = window_prompt("One.", "", 5);
        let large = window_prompt(&"word ".repeat(14_000), "", 5);
        let next = window_prompt("Three.", "", 5);
        assert_eq!(
            went(&[
                (&small, 1),
                (&large, 3),
                (&large, 3),
                (&large, 3),
                (&next, 5)
            ]),
            (vec![Some(0), Some(1), Some(1), Some(1), Some(2)], true)
        );
        let known = "- [c1] word word word word\n".repeat(700);
        let tailed = window_prompt("One.", &known, 5);
        assert!(budget::estimate(&three(&tailed)[2]) > 4_000);
        assert_eq!(
            went(&[(&tailed, 1), (&tailed, 3), (&tailed, 5), (&tailed, 7)]),
            (vec![Some(0), Some(1), Some(2), Some(3)], true)
        );
    }

    /// #971 condition 15: a conversation, and what a call read, are each entry's own. A window
    /// whose first entry's answer the check refuses, after that entry's conversation, and whose
    /// second entry answers at its first turn, was read nothing before by the entry whose answer
    /// is kept. When the first entry fails at the next window, the second goes on with its own.
    #[test]
    fn each_entrys_conversation_and_what_it_read_are_its_own() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let of_a = vec![
            ("200 OK", messages_saying("kept", 60, 0, 1_940)),
            ("200 OK", messages_saying("refused", 70, 1_900, 60)),
            ("500 Internal Server Error", b"{}".to_vec()),
        ];
        let of_b = vec![
            ("200 OK", messages_saying("kept", 60, 0, 1_940)),
            ("200 OK", messages_saying("kept", 70, 1_900, 60)),
        ];
        let ((url_a, _requests_a), (url_b, requests_b)) =
            (serve_each(of_a, ""), serve_each(of_b, ""));
        let named = |url: &str, to: &str| {
            let mut p = talker(url, 100_000);
            if let Provider::Openai { name, .. } = &mut p {
                *name = to.into();
            }
            p
        };
        let providers = [named(&url_a, "a"), named(&url_b, "b")];
        let mut talk = Talk::default();
        let refuse = |v: &Value| (v["summary"] == "refused").then_some("unanchored");
        let run = |talk: &mut Talk, parts: &Parts, from: i64| {
            let turn = turn_of(parts, "k", from, from + 1);
            let span = format!("{from}-{}", from + 1);
            Chain::new(&providers, &conn)
                .check(&refuse)
                .talk(talk, &turn)
                .run("curator", &span, &parts.0, &json!({"type": "object"}))
                .unwrap()
                .provider
        };
        let windows = ["One.", "Two.", "Three."].map(|line| window_prompt(line, "", 5));
        assert_eq!(run(&mut talk, &windows[0], 1), "a");
        assert_eq!(run(&mut talk, &windows[1], 3), "b");
        assert_eq!(talk.read("a", "3-4"), [(1, 2)]);
        assert_eq!(talk.read("b", "3-4"), [], "its first turn");
        assert_eq!(run(&mut talk, &windows[2], 5), "b");
        assert_eq!(talk.read("b", "5-6"), [(3, 4)]);
        let last = requests_b.try_iter().last().unwrap();
        let sent: Vec<String> = blocks(&last).into_iter().skip(1).map(|b| b.0).collect();
        let stretch = |p: &Parts| three(p)[1].clone();
        assert_eq!(sent[..2], [stretch(&windows[1]), stretch(&windows[2])]);
        // a: its first turn, a second its check refused, a third that failed; b: two turns.
        let by: Vec<(String, Option<i64>)> = conn
            .prepare("SELECT provider, earlier FROM provider_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let by: Vec<(&str, Option<i64>)> = by.iter().map(|(p, n)| (p.as_str(), *n)).collect();
        assert_eq!(
            by,
            [
                ("a", Some(0)),
                ("a", Some(1)),
                ("b", Some(0)),
                ("a", Some(2)),
                ("b", Some(1))
            ]
        );
    }

    /// Decision 107 (the security review of this branch): a turn's op lists what it read whole,
    /// and an op's bytes are capped, so a turn reads no more spans than `TALK_SPANS`. Windows
    /// with records between them that went to no window of the conversation are a span each: the
    /// turn after that many starts again.
    #[test]
    fn a_turn_reads_no_more_spans_than_its_op_lists() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let calls = TALK_SPANS + 3;
        let (url, _requests) = counting(calls, 1.5);
        let providers = [talker(&url, 100_000)];
        let mut talk = Talk::default();
        let parts = window_prompt("A line.", "", 5);
        for n in 0..calls as i64 {
            // Two records, then eight that are no window's.
            let turn = turn_of(&parts, "k", n * 10 + 1, n * 10 + 2);
            talk_run(&providers, &conn, &mut talk, &turn).unwrap();
            let read = talk.read("stub", &format!("{}-{}", n * 10 + 1, n * 10 + 2));
            assert!(read.len() <= TALK_SPANS, "{n}: {}", read.len());
        }
        let went = earlier(&conn);
        assert_eq!(
            went[TALK_SPANS - 1..TALK_SPANS + 3],
            [
                Some(TALK_SPANS as i64 - 1),
                Some(TALK_SPANS as i64),
                Some(0),
                Some(1)
            ],
            "{went:?}"
        );
    }

    /// Decision 107 (the security review of this branch): a conversation, and what a call read,
    /// are kept by an entry's name, so an entry whose name another entry of the chain has keeps
    /// none. Its calls are the prompt as it is, no turn in the ledger, and read nothing.
    #[test]
    fn an_entry_whose_name_another_has_keeps_no_conversation() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answers = (0..2).map(|_| ("200 OK", messages_answer(60, 0, 1_940)));
        let (url, requests) = serve_each(answers.collect(), "");
        let providers = [talker(&url, 100_000), talker("http://127.0.0.1:9", 100_000)];
        let mut talk = Talk::default();
        let one = window_prompt("One.", "", 5);
        let two = window_prompt("Two.", "", 5);
        talk_run(&providers, &conn, &mut talk, &turn_of(&one, "k", 1, 2)).unwrap();
        talk_run(&providers, &conn, &mut talk, &turn_of(&two, "k", 3, 4)).unwrap();
        let last = requests.try_iter().last().unwrap();
        assert_eq!(blocks(&last).len(), 2, "its instructions and the rest");
        assert_eq!(earlier(&conn), [None, None]);
        assert_eq!(talk.read("stub", "3-4"), []);
        assert!(talk.entries.is_empty());
    }

    /// Decision 107 (the security review): a 413 for a turn that went on says nothing of the
    /// entries after it, which are sent another prompt: one with the same ceiling is still
    /// asked. A 413 for a turn that started again is of the window alone, and they are skipped
    /// as before.
    #[test]
    fn a_413_for_a_turn_that_went_on_skips_no_other_entry() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let ok = || ("200 OK", messages_answer(60, 0, 1_940));
        let too_large = || {
            let body = json!({"error": {"code": "request_too_large"}}).to_string();
            ("413 Payload Too Large", body.into_bytes())
        };
        let (url_a, _requests_a) = serve_each(vec![ok(), too_large(), too_large()], "");
        let (url_b, requests_b) = serve_each(vec![ok()], "");
        let named = |url: &str, to: &str| {
            let mut p = talker(url, 100_000);
            if let Provider::Openai { name, .. } = &mut p {
                *name = to.into();
            }
            p
        };
        let providers = [named(&url_a, "a"), named(&url_b, "b")];
        let mut talk = Talk::default();
        let windows = ["One.", "Two.", "Three."].map(|line| window_prompt(line, "", 5));
        let mut run = |n: usize| {
            conn.execute("DELETE FROM provider_state", []).unwrap();
            let from = n as i64 * 2 + 1;
            let turn = turn_of(&windows[n], "k", from, from + 1);
            talk_run(&providers, &conn, &mut talk, &turn).map(|r| r.provider)
        };
        assert_eq!(run(0).unwrap(), "a");
        // a's turn went on and is refused as too large: b is asked, and sent the window alone.
        assert_eq!(run(1).unwrap(), "b");
        assert_eq!(blocks(&requests_b.try_iter().last().unwrap()).len(), 3);
        // a starts again and is refused again: b would get no smaller a prompt.
        assert!(run(2).is_err());
        assert_eq!(outcomes(&conn), ["ok", "error", "ok", "error", "too_big"]);
    }

    /// Decision 107: a turn answered 429 with a short wait is sent again as it was planned,
    /// through the budget and the gate again. Both attempts are turns in the ledger, with the
    /// same stretches before their own, and the conversation's time is the second's: the call
    /// whose request its cache was written by.
    #[test]
    fn a_turn_sent_again_after_a_429_is_the_same_turn() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answers = vec![
            ("200 OK", messages_answer(60, 0, 1_940)),
            ("429 Too Many Requests", b"{}".to_vec()),
            ("200 OK", messages_answer(70, 1_900, 60)),
            ("200 OK", messages_answer(70, 1_960, 60)),
        ];
        let (url, requests) = serve_each(answers, "Retry-After: 0\r\n");
        let mut p = talker(&url, 100_000);
        if let Provider::Openai { retry_429, .. } = &mut p {
            *retry_429 = true;
        }
        let providers = [p];
        let mut talk = Talk::default();
        let windows = ["One.", "Two.", "Three."].map(|line| window_prompt(line, "", 5));
        talk_run(
            &providers,
            &conn,
            &mut talk,
            &turn_of(&windows[0], "k", 1, 2),
        )
        .unwrap();
        let before = db::now_ms();
        talk_run(
            &providers,
            &conn,
            &mut talk,
            &turn_of(&windows[1], "k", 3, 4),
        )
        .unwrap();
        // The second attempt went half a second after the 429.
        let started = talk.entries["stub"].started;
        assert!(started >= before + 400, "{} ms after", started - before);
        talk_run(
            &providers,
            &conn,
            &mut talk,
            &turn_of(&windows[2], "k", 5, 6),
        )
        .unwrap();
        let sent: Vec<Vec<(String, bool)>> = requests.try_iter().map(|r| blocks(&r)).collect();
        assert_eq!(sent[1], sent[2], "the same turn twice");
        assert_eq!((sent[1].len(), sent[3].len()), (4, 5));
        assert_eq!(earlier(&conn), [Some(0), Some(1), Some(1), Some(2)]);
    }

    /// #971 conditions 12 and 14: only an entry of the Messages API that declares a ceiling and
    /// marks no cache of its own keeps a conversation, and only for the curator's windows. Any
    /// other entry is sent the turn's prompt as it is, and its call is no turn in the ledger.
    #[test]
    fn only_a_messages_entry_with_a_ceiling_keeps_a_conversation_for_the_curator() {
        let talker = talker("http://127.0.0.1:9", 100_000);
        assert!(talks(&talker, "curator").is_some());
        for role in ["summary", "judge", crate::my_work::ROLE] {
            assert!(talks(&talker, role).is_none(), "{role}");
        }
        // Its prompt stays within nine tenths of its ceiling, and within what admission lets
        // through with the answer's bound.
        assert_eq!(talks(&talker, "curator").unwrap().0, 90_000.0);
        let mut bounded = talker.clone();
        let mut marked = talker.clone();
        let mut unbounded = talker.clone();
        let mut openai = talker.clone();
        for (p, change) in [
            (&mut bounded, "output"),
            (&mut marked, "marker"),
            (&mut unbounded, "ceiling"),
            (&mut openai, "api"),
        ] {
            let Provider::Openai {
                api, extra, limits, ..
            } = p
            else {
                unreachable!()
            };
            match change {
                "output" => limits.max_output_tokens = 16_000,
                "marker" => {
                    extra.insert("cache_control".into(), json!({"type": "ephemeral"}));
                }
                "ceiling" => limits.max_request_tokens = None,
                _ => *api = Api::Openai,
            }
        }
        assert_eq!(talks(&bounded, "curator").unwrap().0, 84_000.0);
        for p in [&marked, &unbounded, &openai] {
            assert!(talks(p, "curator").is_none());
        }
        let cli = Provider::Cli {
            enabled: true,
            name: "claude".into(),
            cli: "claude".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 10,
            limits: Default::default(),
            credits: false,
        };
        assert!(talks(&cli, "curator").is_none());
        // An OpenAI-compatible entry given the turn: its prompt whole, and no turn in the ledger.
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{\"summary\":\"s\"}"}}]});
        let (url, requests) = serve_once(answer.to_string().into_bytes(), "");
        let mut talk = Talk::default();
        let one = window_prompt("One.", "", 5);
        talk_run(&[stub(url)], &conn, &mut talk, &turn_of(&one, "k", 1, 2)).unwrap();
        let body = request_body(&requests.recv().unwrap());
        assert_eq!(body["messages"][0]["content"], json!(one.0));
        assert_eq!(earlier(&conn), [None]);
        assert_eq!(talk.read("stub", "1-2"), []);
    }

    fn stub(url: String) -> Provider {
        Provider::Openai {
            enabled: true,
            name: "stub".into(),
            base_url: url,
            api: Default::default(),
            key_file: None,
            model: "m".into(),
            daily_budget: Some(10),
            timeout_s: 10,
            retry_429: false,
            extra: Default::default(),
            headers: Default::default(),
            limits: Default::default(),
            subscription: false,
        }
    }

    fn outcomes(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT outcome FROM provider_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_429_with_no_reset_doubles_its_cooldown_up_to_an_hour() {
        let e = CallError {
            status: Some(429),
            retry_after_s: None,
            message: "http 429".into(),
            usage: Usage::default(),
            sent: true,
            cool_until: None,
            rate: None,
            uncredited: false,
        };
        let mut s = providers_db::State::default();
        let mut waits = Vec::new();
        for _ in 0..9 {
            let before = db::now_ms();
            s = next_state(s, &e);
            waits.push((s.down_until - before + 500) / 1000);
        }
        assert_eq!(waits, [45, 90, 180, 360, 720, 1440, 2880, 3600, 3600]);
        // A 429 that names its reset starts the doubling again (an answer does too: the chain
        // stores the default state).
        let named = CallError {
            retry_after_s: Some(10.0),
            ..e
        };
        assert_eq!(next_state(s, &named).backoff, 0);
    }

    /// A key the provider rejects by name fails every call until the owner replaces it: its rest
    /// doubles up to a day instead of re-sending every window each ten minutes. A bare 401 keeps
    /// the flat outage rest: OpenCode Go answers 401 for credits and monthly limits too.
    #[test]
    fn a_rejected_key_doubles_its_rest_up_to_a_day() {
        let e = |message: &str| CallError {
            status: Some(401),
            retry_after_s: None,
            message: message.into(),
            usage: Usage::default(),
            sent: true,
            cool_until: None,
            rate: None,
            uncredited: false,
        };
        let waits = |e: &CallError| {
            let mut s = providers_db::State::default();
            (0..10)
                .map(|_| {
                    let before = db::now_ms();
                    s = next_state(s, e);
                    (s.down_until - before + 30_000) / 60_000
                })
                .collect::<Vec<_>>()
        };
        let day = [10, 20, 40, 80, 160, 320, 640, 1280, 1440, 1440];
        assert_eq!(waits(&e("http 401: invalid_api_key")), day);
        assert_eq!(waits(&e("http 401: authentication_error")), day);
        let denied = CallError {
            status: Some(403),
            ..e("http 403: PERMISSION_DENIED")
        };
        assert_eq!(waits(&denied), day);
        assert_eq!(waits(&e("http 401")), [10; 10]);
    }

    /// Gemini names a rejected key in `status` next to a numeric `code`: the name is kept, so the
    /// key's rest doubles.
    #[test]
    fn geminis_permission_denied_is_a_rejected_key() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let (url, _) = serve(
            "403 Forbidden",
            json!([{"error": {"code": 403, "status": "PERMISSION_DENIED",
                "message": "Method doesn't allow unregistered callers canary-gemini"}}])
            .to_string()
            .into_bytes(),
            "",
        );
        let providers = [stub(url)];
        assert!(
            Chain::new(&providers, &conn)
                .run("curator", "s", "p", &json!({}))
                .is_err()
        );
        let detail: String = conn
            .query_row("SELECT detail FROM provider_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(detail, "http 403: PERMISSION_DENIED");
        assert_eq!(
            crate::providers_db::state(&conn, "stub").unwrap().backoff,
            1
        );
    }

    #[test]
    fn mistrals_429_is_read_from_the_body_root_and_backs_off() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        // Mistral's shape (2026-09-27): the fields at the root, the message in English.
        let (url, _) = serve(
            "429 Too Many Requests",
            json!({"object": "error", "message": "Rate limit exceeded canary-mistral",
                "type": "rate_limited", "param": null, "code": "1300", "raw_status_code": 429})
            .to_string()
            .into_bytes(),
            "",
        );
        let providers = [stub(url)];
        let Err(err) = Chain::new(&providers, &conn).run("curator", "s", "p", &json!({})) else {
            panic!("the stub only refuses");
        };
        assert!(!format!("{err:#}").contains("canary"), "{err:#}");
        let detail: String = conn
            .query_row("SELECT detail FROM provider_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(detail, "http 429: rate_limited");
        let s = crate::providers_db::state(&conn, "stub").unwrap();
        assert_eq!(s.backoff, 1);
        assert!(
            (44_000..=46_000).contains(&(s.down_until - db::now_ms())),
            "{s:?}"
        );
    }

    #[test]
    fn an_unusable_answer_still_records_the_tokens_it_billed() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let usage = json!({"prompt_tokens": 100, "completion_tokens": 20});
        // Not JSON, then JSON of the wrong shape: both billed, both unusable.
        for content in ["no json here", "{\"other\": 1}"] {
            let answer = json!({"choices": [{"message": {"content": content}}], "usage": usage});
            let (url, _) = serve_once(answer.to_string().into_bytes(), "");
            let schema = json!({"type": "object", "required": ["summary"]});
            assert!(
                Chain::new(&[stub(url)], &conn)
                    .run("curator", "s", "p", &schema)
                    .is_err()
            );
        }
        let rows: Vec<(String, Option<i64>, Option<i64>)> = conn
            .prepare("SELECT outcome, prompt_tokens, completion_tokens FROM provider_calls")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let want = ("invalid".to_string(), Some(100), Some(20));
        assert_eq!(rows, [want.clone(), want]);
    }

    fn capped(url: String, name: &str) -> Provider {
        let mut p = stub(url);
        if let Provider::Openai {
            enabled: true,
            name: n,
            limits,
            ..
        } = &mut p
        {
            *n = name.into();
            limits.max_request_tokens = Some(8000);
        }
        p
    }

    #[test]
    fn a_window_over_a_providers_ceiling_is_skipped_without_a_call() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        // 16,800 Japanese-heavy characters: about 9,700 tokens, over Groq free's 8,000.
        let prompt = "日付の列が dd.mm.yyyy 形式の行が落ちている。".repeat(600);
        let answer = json!({"choices": [{"message": {"content": "{}"}}]});
        let (next, _) = serve_once(answer.to_string().into_bytes(), "");
        // The capped entry points at a closed port: a call would be an error row, not too_big.
        let providers = [capped("http://127.0.0.1:9".into(), "groq"), stub(next)];
        let r = Chain::new(&providers, &conn)
            .run("curator", "s", &prompt, &json!({"type": "object"}))
            .unwrap();
        assert_eq!(r.provider, "stub");
        let rows: Vec<(String, String, i64)> = conn
            .prepare("SELECT provider, outcome, bytes_out FROM provider_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows[0], ("groq".into(), "too_big".into(), 0));
        assert_eq!(rows[1].1, "ok");
        let err = Chain::new(&providers[..1], &conn)
            .run("curator", "s", &prompt, &json!({"type": "object"}))
            .unwrap_err();
        let failed = err.downcast_ref::<ChainFailed>().expect("a ChainFailed");
        assert_eq!(failed.0[0].skip, Skip::TooBig);
    }

    #[test]
    fn after_a_413_the_entries_with_the_same_ceiling_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let (big, _) = serve(
            "413 Payload Too Large",
            json!({"error": {"code": "request_too_large"}})
                .to_string()
                .into_bytes(),
            "",
        );
        let providers = [
            capped(big, "groq"),
            capped("http://127.0.0.1:9".into(), "groq-20b"),
        ];
        assert!(
            Chain::new(&providers, &conn)
                .run("curator", "s", "short", &json!({}))
                .is_err()
        );
        assert_eq!(outcomes(&conn), ["error", "too_big"]);
    }

    #[test]
    fn a_failure_before_dispatch_records_no_egress() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let mut p = stub("http://127.0.0.1:9".into());
        if let Provider::Openai { key_file, .. } = &mut p {
            *key_file = Some(home.path().join("NO_SUCH_KEY.md"));
        }
        let missing_cli = Provider::Cli {
            credits: false,
            enabled: true,
            name: "nocli".into(),
            cli: "oboete-no-such-cli".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 5,
            limits: Default::default(),
        };
        assert!(
            Chain::new(&[p, missing_cli], &conn)
                .run("curator", "s", "private text", &json!({}))
                .is_err()
        );
        let sent: Vec<i64> = conn
            .prepare("SELECT bytes_out FROM provider_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sent, [0, 0]);
    }

    /// D11: a failure that sets a cooldown passes by itself, so it is a wait, not a failure.
    #[test]
    fn a_failure_that_sets_a_cooldown_is_gone_past_until_it_ends() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let (url, _) = serve(
            "429 Too Many Requests",
            b"{}".to_vec(),
            "Retry-After: 600\r\n",
        );
        let before = crate::db::now_ms();
        let err = Chain::new(&[stub(url)], &conn)
            .run("curator", "s", "p", &json!({}))
            .unwrap_err();
        let failed = err.downcast_ref::<ChainFailed>().expect("a ChainFailed");
        let down_until = providers_db::state(&conn, "stub").unwrap().down_until;
        assert!(down_until >= before + 600_000, "{down_until}");
        assert_eq!(failed.0[0].skip, Skip::Wait(down_until));
    }

    /// The isolation probe takes seconds: a call the budget refuses runs none.
    #[test]
    fn a_curator_cli_the_budget_refuses_is_not_probed() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let spent = Provider::Cli {
            credits: false,
            enabled: true,
            name: "codex".into(),
            cli: "codex".into(),
            model: None,
            daily_budget: 0,
            timeout_s: 5,
            limits: Default::default(),
        };
        assert!(
            Chain::new(&[spent], &conn)
                .run("curator", "s", "p", &json!({}))
                .is_err()
        );
        let probed: i64 = conn
            .query_row("SELECT COUNT(*) FROM isolation", [], |r| r.get(0))
            .unwrap();
        let outcome: String = conn
            .query_row("SELECT outcome FROM provider_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!((probed, outcome.as_str()), (0, "budget"));
    }

    #[test]
    fn groqs_rate_headers_are_kept_and_a_request_they_cannot_take_goes_elsewhere() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]}).to_string();
        let (url, _) = serve_once(
            answer.clone().into_bytes(),
            "x-ratelimit-remaining-tokens: 300\r\nx-ratelimit-reset-tokens: 2m59.5s\r\n\
             x-ratelimit-remaining-requests: 999\r\nx-ratelimit-reset-requests: 7.66s\r\n",
        );
        let schema = json!({"type": "object"});
        let before = db::now_ms();
        Chain::new(&[stub(url.clone())], &conn)
            .run("curator", "s", "short", &schema)
            .unwrap();
        let after = db::now_ms();
        let rate = crate::providers_db::rate(&conn, "stub").unwrap();
        assert_eq!((rate.tokens, rate.requests), (Some(300), Some(999)));
        // 2m59.5s from when the answer was read, however long the call took (#479).
        let reset = rate.tokens_reset_at.unwrap();
        assert!(
            (before + 179_500..=after + 179_500).contains(&reset),
            "{reset}"
        );
        // 2,000 characters are about 560 tokens: more than the 300 left this minute.
        let (next, _) = serve_once(answer.into_bytes(), "");
        let providers = [stub(url), {
            let mut p = stub(next);
            if let Provider::Openai { name, .. } = &mut p {
                *name = "next".into();
            }
            p
        }];
        let r = Chain::new(&providers, &conn)
            .run("curator", "s", &"a".repeat(2000), &schema)
            .unwrap();
        assert_eq!(r.provider, "next");
        assert_eq!(outcomes(&conn), ["ok", "budget", "ok"]);
    }

    #[test]
    fn each_answer_failure_is_recorded_as_its_own_outcome() {
        // A window with no lines: no quote is in it.
        let w = crate::curate::Window {
            device: "d".into(),
            from_seq: 1,
            from_offset: None,
            to_seq: 1,
            to_offset: None,
            text: String::new(),
            elided: Vec::new(),
            shortened: Vec::new(),
            full: false,
            ends: false,
            excluded: Vec::new(),
            aside: None,
            lines: Vec::new(),
            reading: Default::default(),
            top: 0,
            scale: 1,
        };
        let check = |v: &Value| crate::curate::check(&w, v);
        let claim = json!({"id": "c1", "kind": "decision", "status": "decided", "speaker": "user",
            "scope": "repo", "body": "b", "quote": "q", "line": "L1", "supersedes": []});
        let content = |c: String| json!({"choices": [{"message": {"content": c}}]}).to_string();
        let answers = [
            ("empty", String::new()),
            ("prose", "Sure! Here are the claims.".to_owned()),
            ("shape", json!({"claims": "c1", "summary": "s"}).to_string()),
            (
                "over_cap",
                json!({"claims": vec![claim.clone(); 51]}).to_string(),
            ),
            (
                "unanchored",
                json!({"claims": [claim], "summary": "s"}).to_string(),
            ),
        ];
        let kept = content(json!({"claims": [], "summary": "s"}).to_string());
        for (outcome, answer) in answers {
            let home = tempfile::tempdir().unwrap();
            let conn = crate::providers_db::open(home.path()).unwrap();
            let (url, _) = serve_once(content(answer.clone()).into_bytes(), "");
            let (next, _) = serve_once(kept.clone().into_bytes(), "");
            let providers = [stub(url), {
                let mut p = stub(next);
                if let Provider::Openai { name, .. } = &mut p {
                    *name = "next".into();
                }
                p
            }];
            // A note an earlier check left on the thread is not this answer's.
            note_refusal("claims: 9".into());
            let r = Chain::new(&providers, &conn)
                .check(&check)
                .run("curator", "s", "p", &crate::curate::schema())
                .unwrap();
            assert_eq!(r.provider, "next", "{outcome}");
            assert_eq!(outcomes(&conn), [outcome, "ok"]);
            let detail: String = conn
                .query_row("SELECT detail FROM provider_calls ORDER BY id", [], |r| {
                    r.get(0)
                })
                .unwrap();
            if outcome == "prose" {
                // Why it is not JSON, never the answer's text.
                assert!(detail.contains("expected value"), "{detail}");
                assert!(!detail.contains("Sure"), "{detail}");
            }
            // #368: what an unanchored answer's claims missed, in numbers; no other outcome has
            // a note, one left by an earlier check on the thread either.
            let note = "(unanchored; claims: 1, no such line: 1, quote in another line: 0)";
            assert_eq!(detail.contains(note), outcome == "unanchored", "{detail}");
            assert_eq!(detail.contains("; "), outcome == "unanchored", "{detail}");
            // Alone, it is a provider that failed (D11 counts it); one whose answer only did not
            // anchor answered, and is not counted toward its breaker (#330).
            let before = crate::providers_db::state(&conn, "stub").unwrap().fails;
            let (url, _) = serve_once(content(answer.clone()).into_bytes(), "");
            let Err(err) = Chain::new(&[stub(url)], &conn).check(&check).run(
                "curator",
                "s",
                "p",
                &crate::curate::schema(),
            ) else {
                panic!("{outcome}: refused answer accepted");
            };
            let ChainFailed(fallbacks) = err.downcast::<ChainFailed>().unwrap();
            let fails = crate::providers_db::state(&conn, "stub").unwrap().fails;
            // Only an answer that did not anchor is kept, for its cards (docs/cards.md C5).
            if outcome == "unanchored" {
                assert_eq!((&fallbacks[0].skip, fails), (&Skip::Refused, 0));
                let answer: Value = serde_json::from_str(&answer).unwrap();
                assert_eq!(fallbacks[0].answer.as_ref(), Some(&answer));
            } else {
                assert_eq!(
                    (&fallbacks[0].skip, fails),
                    (&Skip::Failed, before + 1),
                    "{outcome}"
                );
                assert_eq!(fallbacks[0].answer, None, "{outcome}");
            }
            // Each was a request sent: the daily budget counts it.
            let (sent, _) = crate::providers_db::calls_in_a_day(&conn, "stub").unwrap();
            assert_eq!(sent, 2, "{outcome}");
        }
    }

    #[test]
    fn rate_headers_are_kept_when_the_answer_has_the_wrong_shape() {
        let schema = json!({"type": "object", "required": ["summary"]});
        // Valid JSON of another shape, a body that is not JSON, and content that is not JSON.
        let shape = json!({"choices": [{"message": {"content": "{}"}}]}).to_string();
        let prose = json!({"choices": [{"message": {"content": "Sure!"}}]}).to_string();
        for body in [shape, "<html>".to_owned(), prose] {
            let home = tempfile::tempdir().unwrap();
            let conn = crate::providers_db::open(home.path()).unwrap();
            let (url, _) = serve_once(
                body.clone().into_bytes(),
                "x-ratelimit-remaining-tokens: 300\r\nx-ratelimit-reset-tokens: 1m\r\n",
            );
            let r = Chain::new(&[stub(url)], &conn).run("curator", "s", "short", &schema);
            assert!(r.is_err());
            assert_eq!(
                crate::providers_db::rate(&conn, "stub").unwrap().tokens,
                Some(300),
                "{body}"
            );
        }
    }

    #[test]
    fn a_paid_entry_never_asks_for_more_output_than_its_admission_counted() {
        let answer = json!({"choices": [{"message": {"content": "{}"}}]}).to_string();
        for extra in [
            json!({}),
            json!({"max_tokens": 32_000}),
            json!({"max_completion_tokens": 100}),
        ] {
            let (url, got) = serve_once(answer.clone().into_bytes(), "");
            let mut p = stub(url);
            if let Provider::Openai {
                extra: e, limits, ..
            } = &mut p
            {
                *e = extra.as_object().unwrap().clone();
                limits.usd_per_mtok_out = 1.0;
                limits.max_output_tokens = 4000;
            }
            call(
                &p,
                CURATOR_SYSTEM,
                "short",
                &json!({"type": "object"}),
                None,
                false,
            )
            .unwrap();
            let req = got.recv().unwrap();
            let body: Value =
                serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
            let asked = body["max_tokens"]
                .as_u64()
                .or(body["max_completion_tokens"].as_u64());
            assert!(asked.is_some_and(|n| n <= 4000), "{extra}: {body}");
        }
    }

    #[test]
    fn a_curator_cli_not_proven_isolated_is_skipped_without_a_call() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]}).to_string();
        let (url, _) = serve_once(answer.into_bytes(), "");
        let agy = Provider::Cli {
            credits: false,
            enabled: true,
            name: "agy".into(),
            cli: "agy".into(),
            model: None,
            daily_budget: 10,
            timeout_s: 5,
            limits: Default::default(),
        };
        let r = Chain::new(&[agy, stub(url)], &conn)
            .run("curator", "s", "short", &json!({"type": "object"}))
            .unwrap();
        assert_eq!(r.provider, "stub");
        assert_eq!(outcomes(&conn), ["gate", "ok"]);
    }

    #[test]
    fn a_paid_answer_is_recorded_with_its_cost() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 100}});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { limits, .. } = &mut p {
            limits.usd_per_mtok_in = 1.0;
            limits.usd_per_mtok_out = 10.0;
        }
        Chain::new(&[p], &conn)
            .run("curator", "s", "short", &json!({"type": "object"}))
            .unwrap();
        // 1,000 in (0.001) and 100 out (0.001).
        let usd = crate::providers_db::usd_this_month(&conn).unwrap();
        assert!((usd - 0.002).abs() < 1e-9, "{usd}");
    }

    #[test]
    fn go_durations_read_as_seconds() {
        assert_eq!(go_duration("2m59.56s"), Some(179.56));
        assert_eq!(go_duration("7.66s"), Some(7.66));
        assert_eq!(go_duration("580ms"), Some(0.58));
        assert_eq!(go_duration("soon"), None);
    }

    #[test]
    fn a_call_records_what_it_sent_and_for_what() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let answer = json!({"choices": [{"message": {"content": "{}"}}]});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        Chain::new(&[stub(url)], &conn)
            .run(
                "curator",
                "dev1:1-9",
                "a prompt of 25 bytes here",
                &json!({"type": "object"}),
            )
            .unwrap();
        let row: (String, String, i64) = conn
            .query_row(
                "SELECT role, span, bytes_out FROM provider_calls",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("curator".into(), "dev1:1-9".into(), 25));
    }

    #[test]
    fn a_cooldown_outlives_the_run() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let (url, _) = serve(
            "429 Too Many Requests",
            json!({"error": {"message": "Please try again in 6m20.064s."}})
                .to_string()
                .into_bytes(),
            "",
        );
        let providers = [stub(url)];
        assert!(
            Chain::new(&providers, &conn)
                .run("curator", "s", "p", &json!({}))
                .is_err()
        );
        // The next run (a new Chain, as each worker run makes) does not call it again.
        let Err(err) = Chain::new(&providers, &conn).run("curator", "s", "p", &json!({})) else {
            panic!("the stub is cooling down");
        };
        assert!(format!("{err:#}").contains("cooling down"), "{err:#}");
        assert_eq!(outcomes(&conn), ["error"]);
        let until = crate::providers_db::state(&conn, "stub")
            .unwrap()
            .down_until;
        let left = until - crate::db::now_ms();
        assert!((370_000..=381_000).contains(&left), "{left}");
    }

    #[test]
    fn three_bad_answers_in_a_row_open_the_breaker_and_an_answer_closes_it() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::providers_db::open(home.path()).unwrap();
        let bad = || {
            let (url, _) = serve(
                "400 Bad Request",
                json!({"error": {"code": "json_validate_failed"}})
                    .to_string()
                    .into_bytes(),
                "",
            );
            [stub(url)]
        };
        for _ in 0..2 {
            assert!(
                Chain::new(&bad(), &conn)
                    .run("curator", "s", "p", &json!({}))
                    .is_err()
            );
            assert_eq!(
                crate::providers_db::state(&conn, "stub")
                    .unwrap()
                    .down_until,
                0
            );
        }
        assert!(
            Chain::new(&bad(), &conn)
                .run("curator", "s", "p", &json!({}))
                .is_err()
        );
        let crate::providers_db::State {
            down_until: until,
            fails,
            ..
        } = crate::providers_db::state(&conn, "stub").unwrap();
        assert!(until - crate::db::now_ms() > 29 * 60_000, "{until}");
        assert_eq!(fails, 0);
        // An answer clears the streak.
        crate::providers_db::set_state(
            &conn,
            "stub",
            crate::providers_db::State {
                fails: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let answer = json!({"choices": [{"message": {"content": "{\"summary\": \"s\"}"}}]});
        let (url, _) = serve_once(answer.to_string().into_bytes(), "");
        Chain::new(&[stub(url)], &conn)
            .run("curator", "s", "p", &json!({"type": "object"}))
            .unwrap();
        assert_eq!(
            crate::providers_db::state(&conn, "stub").unwrap(),
            crate::providers_db::State::default()
        );
    }

    /// X11: a sweep removes a directory without a hold (`oboete-cli-*`) unchanged for longer than
    /// `SWEPT_AFTER`, with what it holds, and nothing else: not a newer one, a file or a link of
    /// either name (nor what the link names), or another name.
    #[test]
    fn a_sweep_removes_a_directory_without_a_hold_by_its_age() {
        let root = tempfile::tempdir().unwrap();
        let at = |name: &str| root.path().join(name);
        let old = "oboete-cli-00000000000000a1";
        std::fs::create_dir(at(old)).unwrap();
        std::fs::write(at(old).join("last.json"), "{}").unwrap();
        std::fs::write(at("oboete-cli-00000000000000a2"), "").unwrap();
        std::fs::write(at("oboete-run-00000000000000a3"), "").unwrap();
        // Another program's of a like name (Node's mkdtemp gives six characters) is not oboete's.
        for name in [
            "oboete-cli-a1b2c3",
            "oboete-cli-0000000000000ABC",
            "oboete-cli-00000000000000a10",
        ] {
            std::fs::create_dir(at(name)).unwrap();
        }
        std::fs::create_dir(at("other-old")).unwrap();
        let named = tempfile::tempdir().unwrap();
        std::fs::write(named.path().join("kept"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(named.path(), at("oboete-run-00000000000000a4")).unwrap();
        let now = SystemTime::now();
        assert_eq!(sweep_scratch(root.path(), now), 0);
        let minute = Duration::from_secs(60);
        assert_eq!(sweep_scratch(root.path(), now + SWEPT_AFTER - minute), 0);
        assert!(at(old).exists());
        assert_eq!(sweep_scratch(root.path(), now + SWEPT_AFTER + minute), 1);
        assert!(!at(old).exists());
        assert!(
            at("oboete-cli-00000000000000a2").exists()
                && at("oboete-run-00000000000000a3").exists()
        );
        for name in [
            "oboete-cli-a1b2c3",
            "oboete-cli-0000000000000ABC",
            "oboete-cli-00000000000000a10",
        ] {
            assert!(at(name).exists(), "{name}");
        }
        assert!(at("other-old").exists());
        #[cfg(unix)]
        assert!(at("oboete-run-00000000000000a4").symlink_metadata().is_ok());
        assert!(named.path().join("kept").exists());
        assert_eq!(sweep_scratch(&at("nothing here"), now + 2 * SWEPT_AFTER), 0);
    }

    /// X11: a run's directory is held while the run goes on, and a sweep leaves a held one
    /// whatever its age; once nobody holds it (its oboete was killed), a sweep removes it after
    /// `HOLD_GRACE`.
    #[cfg(unix)]
    #[test]
    fn a_sweep_removes_a_runs_directory_once_nobody_holds_it() {
        let scratch = scratch_dir().unwrap();
        let name = scratch.0.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("oboete-run-"), "{name}");
        let other = std::fs::File::open(&scratch.0).unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(scratch);
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("oboete-run-0123456789abcdef");
        std::fs::create_dir(&run).unwrap();
        std::fs::write(run.join("last.json"), "{}").unwrap();
        let lock = std::fs::File::open(&run).unwrap();
        lock.try_lock().unwrap();
        let now = SystemTime::now();
        assert_eq!(sweep_scratch(root.path(), now + 2 * SWEPT_AFTER), 0);
        assert!(run.join("last.json").exists());
        drop(lock);
        assert_eq!(sweep_scratch(root.path(), now), 0);
        let second = Duration::from_secs(1);
        assert_eq!(sweep_scratch(root.path(), now + HOLD_GRACE + second), 1);
        assert!(!run.exists());
    }

    /// X11: a sweep leaves a temp directory where another user could put something else in an
    /// entry's place between its look and its removal: one others can write to that is not sticky.
    /// A FIFO in a run's directory's place is not waited on.
    #[cfg(unix)]
    #[test]
    fn a_sweep_leaves_a_temp_directory_others_can_rename_in() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("oboete-cli-00000000000000b1");
        let mode = |m: u32| {
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(m)).unwrap()
        };
        let later = SystemTime::now() + 2 * SWEPT_AFTER;
        for (m, removed) in [(0o777, 0), (0o770, 0), (0o1777, 1)] {
            std::fs::create_dir_all(&old).unwrap();
            mode(m);
            assert_eq!(sweep_scratch(root.path(), later), removed, "{m:o}");
        }
        mode(0o700);
        let fifo = root.path().join("oboete-run-00000000000000b2");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid C path and mode.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(open_dir(&fifo).is_err());
        assert_eq!(holder(&fifo), None);
    }

    /// #605: a run whose directory has no hold, which a sweep goes by the age for, is held below that
    /// age whatever its timeout; a held run keeps its timeout.
    #[test]
    fn a_run_without_a_hold_ends_before_the_sweeps_age() {
        let root = tempfile::tempdir().unwrap();
        let unheld = Scratch(root.path().join("a"), None);
        let long = 24 * 60 * 60;
        assert_eq!(run_limit(&unheld, long), SWEPT_AFTER / 2);
        assert!(run_limit(&unheld, long) < SWEPT_AFTER);
        assert_eq!(run_limit(&unheld, 300), Duration::from_secs(300));
        let dir = root.path().join("b");
        std::fs::create_dir(&dir).unwrap();
        let file = std::fs::File::open(&dir).ok();
        let held = Scratch(dir, file);
        if held.1.is_some() {
            assert_eq!(run_limit(&held, long), Duration::from_secs(long));
        }
    }

    /// X11: a run's directory that could take no hold takes the name a sweep goes by the age for,
    /// and is still removed when dropped.
    #[test]
    fn a_run_without_a_hold_is_named_for_the_age() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("oboete-run-0123456789abcdef");
        std::fs::create_dir(&dir).unwrap();
        let mut scratch = Scratch(dir.clone(), None);
        aged(&mut scratch, "0123456789abcdef").unwrap();
        let to = root.path().join("oboete-cli-0123456789abcdef");
        assert_eq!(scratch.0, to);
        assert!(to.is_dir() && !dir.exists());
        let now = SystemTime::now();
        assert_eq!(sweep_scratch(root.path(), now + 2 * HOLD_GRACE), 0);
        drop(scratch);
        assert!(!to.exists());
    }

    #[test]
    fn unusable_cli_answers_are_invalid_output() {
        let dir = scratch_dir().unwrap();
        let e = headless_command("nano", None, &dir.0, CURATOR_SYSTEM, "p", "{}").unwrap_err();
        assert!(e.message.contains("unsupported"), "{}", e.message);
        for bad in [r#"{"other": 1}"#, "plain text"] {
            let e = extract_structured("claude", bad).unwrap_err();
            assert!(e.invalid(), "{bad}: {}", e.message);
        }
        // An answer that is text reaches the chain as text, which the schema refuses.
        let prose = extract_structured("claude", r#"{"result": "not json"}"#).unwrap();
        assert_eq!(prose, json!("not json"));
        assert!(!fits(&prose, &crate::curate::schema()));
    }

    #[test]
    fn cooldown_depends_on_status_and_moderation() {
        let err = |status: Option<u16>, msg: &str| CallError {
            status,
            retry_after_s: None,
            message: msg.into(),
            usage: Usage::default(),
            sent: true,
            cool_until: None,
            rate: None,
            uncredited: false,
        };
        assert_eq!(cooldown_for(&err(Some(429), "")), Some(COOLDOWN_429));
        assert_eq!(
            cooldown_for(&err(Some(401), "invalid api key")),
            Some(COOLDOWN_OUTAGE)
        );
        assert_eq!(
            cooldown_for(&err(Some(403), "forbidden")),
            Some(COOLDOWN_OUTAGE)
        );
        assert_eq!(
            cooldown_for(&err(Some(403), "Your input was flagged for violence")),
            None
        );
        assert_eq!(cooldown_for(&err(Some(403), "blocked by moderation")), None);
        assert_eq!(
            cooldown_for(&err(Some(400), "Generated JSON does not match")),
            None
        );
        assert_eq!(cooldown_for(&err(Some(503), "")), Some(COOLDOWN_OUTAGE));
        assert_eq!(cooldown_for(&err(None, "timed out")), Some(COOLDOWN_OUTAGE));
    }

    #[test]
    fn retry_after_is_read_from_groq_bodies() {
        let body = r#"{"error":{"message":"Rate limit reached ... Please try again in 17.2875s. Need more tokens?"}}"#;
        assert_eq!(retry_after_in_body(body), Some(17.2875));
        // Groq's daily-token 429 (every one in the owner's store, 2026-09-22..26).
        let tpd = "on tokens per day (TPD): Limit 200000. Please try again in 6m20.064s. Need more";
        assert_eq!(retry_after_in_body(tpd), Some(380.064));
        assert_eq!(retry_after_in_body("try again in 1h2m3.5s."), Some(3723.5));
        assert_eq!(retry_after_in_body("try again in 580ms"), Some(0.58));
        assert_eq!(retry_after_in_body("try again in 2m"), Some(120.0));
        for bad in [
            "nothing",
            "try again in s",
            "try again in 5",
            "try again in 1e999s",
            "try again in 3x",
        ] {
            assert_eq!(retry_after_in_body(bad), None, "{bad}");
        }
    }

    #[test]
    fn gemini_errors_are_read_from_its_array_body_and_details() {
        let body = |id: &str| {
            json!([{"error": {"code": 429, "status": "RESOURCE_EXHAUSTED",
                "message": "You exceeded your current quota.",
                "details": [
                    {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                     "violations": [{"quotaMetric": "generate_content_requests", "quotaId": id}]},
                    {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "35s"}]}}])
            .to_string()
        };
        // The array root is read (it gave no code before), and its status name before the number.
        assert_eq!(
            error_code(&body("x")).as_deref(),
            Some("RESOURCE_EXHAUSTED")
        );
        // A per-minute quota: its RetryInfo delay.
        let minute = body("GenerateRequestsPerMinutePerProjectPerModel-FreeTier");
        assert_eq!(retry_after_in_error(429, &minute), Some(35.0));
        // A daily one resets at midnight Pacific, not in 35 s.
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let day = body("GenerateRequestsPerDayPerProjectPerModel-FreeTier");
        let noon = at(86_400 * 100 + 12 * 3600);
        assert_eq!(
            retry_after_in_error_at(429, &day, noon),
            Some(20.0 * 3600.0)
        );
        assert_eq!(retry_after_in_error(400, &day), None);
        assert_eq!(until_pacific_midnight(at(86_400 * 100)), 8.0 * 3600.0);
        assert_eq!(
            until_pacific_midnight(at(86_400 * 100 + 9 * 3600)),
            23.0 * 3600.0
        );
    }

    #[test]
    fn the_reset_comes_only_from_a_429s_error_message() {
        let msg = |m: &str| json!({"error": {"message": m}}).to_string();
        assert_eq!(
            retry_after_in_error(429, &msg("Please try again in 2m3s.")),
            Some(123.0)
        );
        // An echoed generation or prompt says "24h": it is not the provider's reset.
        let echoed = json!({"error": {"failed_generation": "try again in 24h",
            "message": "Please try again in 2s."}})
        .to_string();
        assert_eq!(retry_after_in_error(429, &echoed), Some(2.0));
        let only_echo =
            json!({"error": {"failed_generation": "try again in 24h", "message": "slow down"}});
        assert_eq!(retry_after_in_error(429, &only_echo.to_string()), None);
        assert_eq!(retry_after_in_error(429, "try again in 24h"), None);
        assert_eq!(retry_after_in_error(400, &msg("try again in 24h")), None);
    }

    #[test]
    fn a_429_cools_down_until_the_providers_reset() {
        let e = |retry: Option<f64>| CallError {
            status: Some(429),
            retry_after_s: retry,
            message: String::new(),
            usage: Usage::default(),
            sent: true,
            cool_until: None,
            rate: None,
            uncredited: false,
        };
        assert_eq!(cooldown_for(&e(None)), Some(COOLDOWN_429));
        assert_eq!(cooldown_for(&e(Some(10.0))), Some(COOLDOWN_429));
        assert_eq!(
            cooldown_for(&e(Some(380.064))),
            Some(Duration::from_secs_f64(380.064))
        );
        assert_eq!(cooldown_for(&e(Some(1e12))), Some(MAX_COOLDOWN));
    }

    #[test]
    fn structured_output_is_found_in_every_cli_envelope() {
        let want = json!({"observations": [], "summary": "s"});
        let claude =
            json!({"result": "{\"observations\":[],\"summary\":\"s\"}", "structured_output": want})
                .to_string();
        let grok = json!({"text": "x", "structuredOutput": want}).to_string();
        let agy = json!({"response": "{\"observations\":[],\"summary\":\"s\"}", "structured_output": want}).to_string();
        let text_only = json!({"result": "{\"observations\":[],\"summary\":\"s\"}"}).to_string();
        for (cli, text) in [
            ("claude", claude),
            ("grok", grok),
            ("agy", agy),
            ("claude", text_only),
        ] {
            assert_eq!(extract_structured(cli, &text).unwrap(), want, "{cli}");
        }
        assert!(
            extract_structured("grok", "not json")
                .unwrap_err()
                .invalid()
        );
        assert_eq!(
            extract_structured("grok", "{\"text\":\"plain\"}").unwrap(),
            json!("plain")
        );
    }

    /// Unix ms `at` as an RFC 3339 time, as Anthropic writes its resets.
    fn rfc3339(at: i64) -> String {
        chrono::DateTime::from_timestamp_millis(at)
            .unwrap()
            .to_rfc3339()
    }

    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// An entry of `api` at `url` with the key `key` in a key file of `home`.
    fn keyed(home: &Path, url: String, api: Api, key: &str) -> Provider {
        let key_file = home.join("LIST_KEY.md");
        std::fs::write(&key_file, format!("# synthetic only\n{key}\n")).unwrap();
        let mut p = stub(url);
        if let Provider::Openai {
            api: a,
            key_file: k,
            ..
        } = &mut p
        {
            *a = api;
            *k = Some(key_file);
        }
        p
    }

    /// Decision 78: an entry's model list is read with its key as its requests send it (Bearer,
    /// or the Messages API's `x-api-key` and version, `limit` there), following no redirect, ids
    /// only, kept ten minutes and read again on `refresh`.
    #[test]
    fn an_entrys_model_list_is_read_with_its_key_and_kept() {
        let home = tempfile::tempdir().unwrap();
        let list = json!({"object": "list", "data": [{"id": "model-b"}, {"id": "model-a"},
            {"id": "model-b"}, {"id": "bad id"}, {"x": 1}]});
        let (url, request) = serve_each(
            vec![
                ("200 OK", list.to_string().into_bytes()),
                ("200 OK", list.to_string().into_bytes()),
            ],
            "",
        );
        let p = keyed(home.path(), url, Api::Openai, "list-test-key");
        assert_eq!(
            list_models(&p, false).unwrap(),
            (vec!["model-b".to_owned(), "model-a".to_owned()], false)
        );
        let head = request.recv().unwrap().to_lowercase();
        assert!(head.starts_with("get /models "), "{head}");
        assert!(
            head.contains("authorization: bearer list-test-key\r\n"),
            "{head}"
        );
        // Kept: no second request until `refresh`.
        assert!(list_models(&p, false).unwrap().1);
        assert!(!list_models(&p, true).unwrap().1);
        assert!(request.recv().is_ok());
        // Each key's list is its own: another key at the same URL reads the service again, and
        // the first key's list is still kept (Codex's review of 78a).
        let (url, request) = serve_each(
            vec![
                ("200 OK", list.to_string().into_bytes()),
                ("200 OK", list.to_string().into_bytes()),
                ("200 OK", list.to_string().into_bytes()),
            ],
            "",
        );
        let first = keyed(home.path(), url, Api::Openai, "first-list-key");
        let mut second = first.clone();
        if let Provider::Openai { key_file, .. } = &mut second {
            let other = home.path().join("OTHER_LIST_KEY.md");
            std::fs::write(&other, "# synthetic only\nsecond-list-key\n").unwrap();
            *key_file = Some(other);
        }
        assert!(!list_models(&first, false).unwrap().1);
        assert!(
            request
                .recv()
                .unwrap()
                .to_lowercase()
                .contains("bearer first-list-key")
        );
        assert!(!list_models(&second, false).unwrap().1);
        assert!(
            request
                .recv()
                .unwrap()
                .to_lowercase()
                .contains("bearer second-list-key")
        );
        assert!(list_models(&first, false).unwrap().1);
        // An entry with a header of its own is a list of its own (cubic on #857).
        let mut projected = first.clone();
        if let Provider::Openai { headers, .. } = &mut projected {
            headers.insert("OpenAI-Project".into(), "proj_synthetic".into());
        }
        assert!(!list_models(&projected, false).unwrap().1);
        let head = request.recv().unwrap().to_lowercase();
        assert!(head.contains("openai-project: proj_synthetic"), "{head}");
        let messages = json!({"data": [{"id": "claude-haiku-5-5", "type": "model"}]});
        let (url, request) = serve_once(messages.to_string().into_bytes(), "");
        let p = keyed(home.path(), url, Api::Anthropic, "messages-list-key");
        assert_eq!(list_models(&p, false).unwrap().0, ["claude-haiku-5-5"]);
        let head = request.recv().unwrap().to_lowercase();
        assert!(head.starts_with("get /models?limit=1000 "), "{head}");
        assert!(head.contains("x-api-key: messages-list-key\r\n"), "{head}");
        assert!(head.contains("anthropic-version: 2023-06-01\r\n"), "{head}");
        assert!(!head.contains("authorization"), "{head}");
    }

    /// Decision 78: each failure has its code, and no redirect is followed with the key.
    #[test]
    fn an_entrys_model_list_fails_by_its_cause() {
        let home = tempfile::tempdir().unwrap();
        let read = |status: &'static str, body: &str, headers: &'static str| {
            let (url, _) = serve(status, body.as_bytes().to_vec(), headers);
            list_models(&keyed(home.path(), url, Api::Openai, "k"), false).unwrap_err()
        };
        assert_eq!(read("401 Unauthorized", "{}", ""), ListError::KeyRefused);
        assert_eq!(read("403 Forbidden", "{}", ""), ListError::KeyRefused);
        assert_eq!(read("500 Oops", "{}", ""), ListError::Unavailable);
        assert_eq!(read("200 OK", "not json", ""), ListError::Unavailable);
        assert_eq!(read("200 OK", "{\"data\": 1}", ""), ListError::Unavailable);
        let big = format!("{{\"data\": [{{\"id\": \"{}\"}}]}}", "a".repeat(5 << 10));
        assert_eq!(read("200 OK", &big, ""), ListError::Unavailable);
        assert_eq!(
            read("302 Found", "", "location: http://127.0.0.1:9/models\r\n"),
            ListError::Unavailable
        );
        // A key file that holds no key, and a subscription's program.
        let (url, _) = serve_once(b"{}".to_vec(), "");
        let mut p = keyed(home.path(), url, Api::Openai, "");
        assert_eq!(list_models(&p, false).unwrap_err(), ListError::NoKey);
        if let Provider::Openai { key_file, .. } = &mut p {
            *key_file = Some(home.path().join("missing.md"));
        }
        assert_eq!(list_models(&p, false).unwrap_err(), ListError::NoKey);
        let cli = config::from_text(&home.path().join("config.toml"), "")
            .unwrap()
            .providers
            .into_iter()
            .find(|p| matches!(p, Provider::Cli { .. }))
            .unwrap();
        assert_eq!(
            list_models(&cli, false).unwrap_err(),
            ListError::Unsupported
        );
    }

    /// Decision 78: each service's list is cut to what a summary's request can use there.
    #[test]
    fn a_model_list_keeps_what_its_service_answers_a_summary_with() {
        let ids =
            |url: &str, free: bool, data: Value| models_of(url, free, data.as_array().unwrap());
        let openai = json!([{"id": "gpt-6-luna"}, {"id": "o4-mini"}, {"id": "gpt-6-audio-preview"},
            {"id": "gpt-6.1-codex"}, {"id": "gpt-5.5-pro"}, {"id": "text-embedding-3-small"},
            {"id": "dall-e-3"}, {"id": "omni-moderation-latest"}, {"id": "gpt-realtime"}]);
        assert_eq!(
            ids("https://api.openai.com/v1", false, openai),
            ["gpt-6-luna", "o4-mini"]
        );
        let gemini = json!([{"id": "models/gemini-3.1-flash-lite"},
            {"id": "models/gemini-embedding-001"}, {"id": "models/imagen-4"},
            {"id": "models/gemini-3.5-flash-image"}]);
        assert_eq!(
            ids(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                false,
                gemini
            ),
            ["gemini-3.1-flash-lite"]
        );
        // Groq's list says what a model writes (api.groq.com/openai/v1/models, 2026-11-07); a
        // model without its mark of structured output is kept.
        let groq = json!([
            {"id": "openai/gpt-oss-20b", "active": true, "output_modalities": ["text"],
                "supported_features": ["json_mode", "structured_outputs"]},
            {"id": "qwen/qwen3.8-27b", "output_modalities": ["text"], "supported_features": ["json_mode"]},
            {"id": "canopylabs/orpheus-v1-english", "output_modalities": ["speech"]},
            {"id": "whisper-large-v3", "output_modalities": ["transcription"]},
            {"id": "whisper-large-v3-turbo"}, {"id": "meta-llama/llama-prompt-guard-2-86m"},
            {"id": "old", "active": false}]);
        assert_eq!(
            ids("https://api.groq.com/openai/v1", false, groq),
            ["openai/gpt-oss-20b", "qwen/qwen3.8-27b"]
        );
        let router = json!([{"id": "a/b:free"}, {"id": "anthropic/claude-haiku-5.5"}]);
        assert_eq!(
            ids("https://openrouter.ai/api/v1", true, router.clone()),
            ["a/b:free"]
        );
        assert_eq!(ids("https://openrouter.ai/api/v1", false, router).len(), 2);
        // OpenCode Go's names the APIs a model answers, as keys, and what it takes
        // (opencode.ai/zen/go/v1/models, 2026-11-07).
        let chat =
            json!({"chat-completions": {"base_url": "https://opencode.ai/inference/go/openai/v1"}});
        let go = json!([
            {"id": "deepseek-v4.1-flash", "api": chat,
                "capabilities": {"structured_output": true, "temperature": true}},
            {"id": "claude-haiku-5-5", "api": {"messages": {}}, "capabilities": {"structured_output": true}},
            {"id": "grok-4.7", "api": {"responses": {}}},
            {"id": "hy3", "api": chat, "capabilities": {"temperature": true}},
            {"id": "kimi-k3", "api": chat, "capabilities": {"structured_output": true, "temperature": false}},
            {"id": "glm-5.3-flash"}]);
        assert_eq!(
            ids("https://opencode.ai/zen/go/v1", false, go),
            ["deepseek-v4.1-flash", "hy3", "glm-5.3-flash"]
        );
        // Another server's id is kept whole, `models/` and all (Codex's review of 78a).
        let other = json!([{"id": "llama3"}, {"id": "nomic-embed"}, {"id": "models/llama3"}]);
        assert_eq!(
            ids("http://127.0.0.1:11434/v1", false, other),
            ["llama3", "nomic-embed", "models/llama3"]
        );
        let many: Vec<Value> = (0..1_200).map(|i| json!({"id": format!("m{i}")})).collect();
        assert_eq!(ids("http://x", false, Value::Array(many)).len(), MODELS_MAX);
    }

    /// The owner, 2026-10-09: an entry with `api = "anthropic"` speaks the Messages API: its key
    /// in `x-api-key` and never in `Authorization`, the version header, the schema in
    /// `output_config.format` without the keywords the API refuses, no temperature, and always a
    /// `max_tokens`. Its answer is its first text block, unfenced as an OpenAI answer is, with its
    /// cache in the usage and its rate headers kept.
    #[test]
    fn a_messages_entry_sends_its_own_request_and_reads_its_answer() {
        let home = tempfile::tempdir().unwrap();
        let key_file = home.path().join("TEST_KEY.md");
        std::fs::write(&key_file, "# synthetic only\nmessages-test-key\n").unwrap();
        let at = db::now_ms() + 30_000;
        let reset = rfc3339(at);
        let headers = leak(format!(
            "anthropic-ratelimit-tokens-remaining: 3000\r\nanthropic-ratelimit-tokens-reset: {reset}\r\n\
             anthropic-ratelimit-requests-remaining: 49\r\nanthropic-ratelimit-requests-reset: {reset}\r\n"
        ));
        let answer = json!({"type": "message", "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": "", "signature": "s"},
                {"type": "text", "text": "```json\n{\"summary\":\"s\"}\n```"}],
            "usage": {"input_tokens": 100, "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 5, "output_tokens": 30}});
        let (url, request) = serve_once(answer.to_string().into_bytes(), headers);
        let mut p = stub(url);
        if let Provider::Openai {
            api,
            key_file: k,
            extra,
            limits,
            ..
        } = &mut p
        {
            *api = Api::Anthropic;
            *k = Some(key_file);
            extra.insert("thinking".into(), json!({"type": "disabled"}));
            limits.usd_per_mtok_out = 0.5;
            limits.max_output_tokens = 3000;
        }
        let schema = json!({"type": "object", "properties": {
            "summary": {"type": "string", "maxLength": 50},
            "minimum": {"type": "integer", "minimum": 1, "maximum": 9, "multipleOf": 1},
            "tags": {"type": "array", "minItems": 2, "maxItems": 5, "uniqueItems": true,
                "items": {"type": "object", "properties": {"t": {"type": "string", "minLength": 1}}}},
            "notes": {"type": "array", "minItems": 1, "items": {"type": "string"}}},
            "required": ["summary"], "additionalProperties": true});
        let a = call(&p, CURATOR_SYSTEM, "p", &schema, None, false).unwrap();
        assert_eq!(a.value, json!({"summary": "s"}));
        let want = Usage {
            prompt: Some(125),
            completion: Some(30),
            cached: Some(5),
            reasoning: None,
            written: Some(20),
        };
        assert_eq!(a.usage, want);
        let rate = a.rate.unwrap();
        assert_eq!((rate.tokens, rate.requests), (Some(3000), Some(49)));
        // The time as written, however long the call took (#479).
        assert_eq!(rate.tokens_reset_at, Some(at));
        assert_eq!(rate.requests_reset_at, rate.tokens_reset_at);
        let req = request.recv().unwrap();
        let (head, body) = req.split_once("\r\n\r\n").unwrap();
        let head = head.to_lowercase();
        assert!(head.starts_with("post /messages "), "{head}");
        assert!(head.contains("x-api-key: messages-test-key\r\n"), "{head}");
        assert!(head.contains("anthropic-version: 2023-06-01\r\n"), "{head}");
        assert!(!head.contains("authorization"), "{head}");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["max_tokens"], 3000);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        for absent in ["temperature", "response_format"] {
            assert!(body.get(absent).is_none(), "{absent}: {body}");
        }
        assert_eq!(body["messages"], json!([{"role": "user", "content": "p"}]));
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        // A keyword the API refuses leaves the copy it gets; a property of that name stays.
        let sent = json!({"type": "object", "properties": {
            "summary": {"type": "string"},
            "minimum": {"type": "integer"},
            "tags": {"type": "array", "items": {"type": "object",
                "properties": {"t": {"type": "string"}}, "additionalProperties": false}},
            "notes": {"type": "array", "minItems": 1, "items": {"type": "string"}}},
            "required": ["summary"], "additionalProperties": false});
        assert_eq!(body["output_config"]["format"]["schema"], sent);
        // An unpriced entry still names its answer's size, which the Messages API requires.
        let (url, request) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { api, .. } = &mut p {
            *api = Api::Anthropic;
        }
        call(
            &p,
            CURATOR_SYSTEM,
            "p",
            &json!({"type": "object"}),
            None,
            false,
        )
        .unwrap();
        let req = request.recv().unwrap();
        let body: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["max_tokens"], 4000);
        assert_eq!(p.declared_output(), 4000);
    }

    /// A refusal (billed, and it need not match the schema), an answer cut off at max_tokens and
    /// one with no text are failures that keep the tokens they billed.
    #[test]
    fn a_messages_refusal_or_cut_off_answer_fails_with_its_billed_tokens() {
        let text = |t: &str| json!([{"type": "text", "text": t}]);
        for (stop, content) in [
            ("refusal", text("{\"summary\":\"s\"}")),
            ("max_tokens", text("{\"summa")),
            ("model_context_window_exceeded", text("{\"summary\":\"s\"}")),
            // Turns that wait on the caller: a schema-valid text before them is not the answer.
            (
                "tool_use",
                json!([{"type": "text", "text": "{\"summary\":\"s\"}"},
                       {"type": "tool_use", "id": "t", "name": "n", "input": {}}]),
            ),
            ("pause_turn", text("{\"summary\":\"s\"}")),
            (
                "end_turn",
                json!([{"type": "thinking", "thinking": "", "signature": "s"}]),
            ),
        ] {
            let answer = json!({"stop_reason": stop, "content": content,
                "usage": {"input_tokens": 12, "output_tokens": 4}});
            let (url, _) = serve_once(answer.to_string().into_bytes(), "");
            let e = http_call(
                Api::Anthropic,
                &url,
                None,
                "m",
                10,
                &Default::default(),
                &Default::default(),
                "p",
                &json!({}),
                None,
                0,
                false,
            )
            .unwrap_err();
            assert!(e.invalid() && e.status.is_none(), "{stop}: {}", e.message);
            assert_eq!(
                (e.usage.prompt, e.usage.completion),
                (Some(12), Some(4)),
                "{stop}"
            );
            assert!(!e.message.contains("summa"), "{}", e.message);
        }
    }

    /// Anthropic's errors as the chain reads them: a 429 by its retry-after and rate headers, a
    /// 529 as a 503, and spent credits resting the entry until the month's end, as the paid
    /// budget's month does. No error body is kept.
    #[test]
    fn messages_errors_rest_the_entry_as_their_kind_says() {
        let body = |kind: &str, message: &str| {
            json!({"type": "error", "error": {"type": kind, "message": message},
                "request_id": "req_canary"})
            .to_string()
            .into_bytes()
        };
        let send = |api: Api, status: &'static str, body: Vec<u8>, headers: &'static str| {
            let (url, _) = serve(status, body, headers);
            http_call(
                api,
                &url,
                None,
                "m",
                10,
                &Default::default(),
                &Default::default(),
                "p",
                &json!({}),
                None,
                0,
                false,
            )
            .unwrap_err()
        };
        let headers = leak(format!(
            "retry-after: 30\r\nanthropic-ratelimit-requests-remaining: 0\r\n\
             anthropic-ratelimit-requests-reset: {}\r\n",
            rfc3339(db::now_ms() + 30_000)
        ));
        let e = send(
            Api::Anthropic,
            "429 Too Many Requests",
            body("rate_limit_error", "canary: over your rate limit"),
            headers,
        );
        assert_eq!(
            (e.status, e.retry_after_s, e.message.as_str()),
            (
                Some(429),
                Some(30.0),
                "http 429: rate_limit_error, retry in 30s"
            )
        );
        let rate = e.rate.unwrap();
        assert_eq!(rate.requests, Some(0));
        assert!(rate.requests_reset_at.unwrap() > db::now_ms());
        let e = send(
            Api::Anthropic,
            "529 Overloaded",
            body("overloaded_error", "Overloaded"),
            "",
        );
        assert_eq!(
            (e.status, e.message.as_str()),
            (Some(529), "http 529: overloaded_error")
        );
        let unavailable = CallError {
            status: Some(503),
            ..CallError::other("http 503")
        };
        assert_eq!(cooldown_for(&e), cooldown_for(&unavailable));
        assert_eq!(cooldown_for(&e), Some(COOLDOWN_OUTAGE));
        for (status, kind, message) in [
            (
                "400 Bad Request",
                "invalid_request_error",
                "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.",
            ),
            (
                "402 Payment Required",
                "billing_error",
                "canary: an issue with your billing",
            ),
        ] {
            // Decision 68: tried again 30 minutes on, as claude-mem tries a spent credential, so
            // credits added meanwhile are used without `oboete resume`.
            let before = db::now_ms();
            let e = send(Api::Anthropic, status, body(kind, message), "");
            let after = db::now_ms();
            assert_eq!(
                e.message,
                format!("http {}: {kind} (credits spent)", &status[..3])
            );
            let half_hour = 30 * 60 * 1000;
            let until = e.cool_until.unwrap();
            assert!(
                (before + half_hour..=after + half_hour).contains(&until),
                "{until}"
            );
            let rested = next_state(providers_db::State::default(), &e);
            assert_eq!(rested.down_until, until);
        }
        // Another 400 is the answer's own; an OpenAI-compatible entry's says nothing of credits.
        let e = send(
            Api::Anthropic,
            "400 Bad Request",
            body(
                "invalid_request_error",
                "canary: minLength is not supported",
            ),
            "",
        );
        assert_eq!(
            (e.message.as_str(), e.cool_until),
            ("http 400: invalid_request_error", None)
        );
        let e = send(
            Api::Openai,
            "400 Bad Request",
            body("invalid_request_error", "Your credit balance is too low"),
            "",
        );
        assert_eq!(e.cool_until, None);
    }

    /// CodeRabbit on #409: an unpriced Messages entry's `max_completion_tokens`, which the API does
    /// not know, is sent as its `max_tokens`, the output `declared_output` counted.
    #[test]
    fn a_messages_entry_sends_max_completion_tokens_as_max_tokens() {
        let answer = json!({"stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"summary\":\"s\"}"}],
            "usage": {"input_tokens": 12, "output_tokens": 4}});
        let (url, request) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { api, extra, .. } = &mut p {
            *api = Api::Anthropic;
            extra.insert("max_completion_tokens".into(), 900.into());
        }
        assert_eq!(p.declared_output(), 900);
        call(
            &p,
            CURATOR_SYSTEM,
            "p",
            &json!({"type": "object"}),
            None,
            false,
        )
        .unwrap();
        let req = request.recv().unwrap();
        let body: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["max_tokens"], 900);
        assert!(body.get("max_completion_tokens").is_none(), "{body}");
    }

    /// Codex on #409: Anthropic refuses a thinking budget of `max_tokens` or more, so a paid
    /// Messages entry whose cap leaves no room for its budget is sent without that thinking, and
    /// one whose cap holds it keeps it.
    #[test]
    fn a_messages_entry_sends_no_thinking_budget_its_cap_cannot_hold() {
        let answer = json!({"stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"summary\":\"s\"}"}],
            "usage": {"input_tokens": 12, "output_tokens": 4}});
        for (cap, kept) in [(1_000, false), (2_048, false), (4_000, true)] {
            let (url, request) = serve_once(answer.to_string().into_bytes(), "");
            let mut p = stub(url);
            let thinking = json!({"type": "enabled", "budget_tokens": 2_048});
            if let Provider::Openai {
                api, extra, limits, ..
            } = &mut p
            {
                *api = Api::Anthropic;
                extra.insert("thinking".into(), thinking.clone());
                limits.usd_per_mtok_out = 1.0;
                limits.max_output_tokens = cap;
            }
            call(
                &p,
                CURATOR_SYSTEM,
                "p",
                &json!({"type": "object"}),
                None,
                false,
            )
            .unwrap();
            let req = request.recv().unwrap();
            let body: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["max_tokens"], cap, "{body}");
            assert_eq!(
                body.get("thinking") == Some(&thinking),
                kept,
                "{cap}: {body}"
            );
        }
    }

    /// The connection test of a Messages entry: the fixed probe at its own endpoint, with the
    /// test's bounds and the entry's own thinking, no other extra, and no key header of the
    /// entry's.
    #[test]
    fn a_messages_entry_is_tested_with_the_fixed_probe_and_its_own_thinking() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let answer = json!({"stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"ok\":true}"}],
            "usage": {"input_tokens": 12, "output_tokens": 4}});
        let (url, request) = serve_once(answer.to_string().into_bytes(), "");
        let mut p = stub(url);
        if let Provider::Openai { api, extra, .. } = &mut p {
            *api = Api::Anthropic;
            extra.insert("thinking".into(), json!({"type": "disabled"}));
            extra.insert("metadata".into(), json!({"user_id": "private-canary"}));
            extra.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
        // The test sends none of the entry's `cache_control` and writes no cache: its preview prices
        // its input once (`settings::providers::preview`), and its reservation allows for the cache
        // write of a fenced prompt (`Provider::input_weight`), which it never makes.
        assert_eq!(p.input_weight("probe"), 2.0);
        assert_eq!(probe_provider(&p).unwrap().input_weight("probe"), 1.25);
        let result = probe(&conn, &p, 5.0, &|| Ok(None)).unwrap();
        assert_eq!(
            (result["status"].as_str(), result["http_status"].as_u64()),
            (Some("ok"), Some(200)),
            "{result}"
        );
        let req = request.recv().unwrap();
        let (head, body) = req.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /messages "), "{head}");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            (body["max_tokens"].as_u64(), body["stream"].as_bool()),
            (Some(128), Some(false))
        );
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert!(body.get("metadata").is_none(), "{body}");
        assert!(body.get("cache_control").is_none(), "{body}");
        assert_eq!(body["messages"][0]["content"], PROBE_PROMPT);
        // A budget of thinking cannot fit the test's 128 tokens: the test sends none.
        if let Provider::Openai { extra, .. } = &mut p {
            extra.insert(
                "thinking".into(),
                json!({"type": "enabled", "budget_tokens": 2048}),
            );
        }
        let Provider::Openai { extra, .. } = probe_provider(&p).unwrap() else {
            panic!("expected an HTTP entry")
        };
        assert!(extra.get("thinking").is_none(), "{extra:?}");
        if let Provider::Openai { headers, .. } = &mut p {
            headers.insert("X-Api-Key".into(), "private-header-canary".into());
        }
        assert_eq!(probe_provider(&p).unwrap_err(), "unsafe_headers");
    }

    /// A fenced prompt's instructions, the same bytes for every window or turn (G09), go in a block
    /// of their own that writes the prompt cache, for five minutes or as the entry's own
    /// `cache_control` asks, the record after them in a block of its own; any other prompt goes
    /// whole, uncached.
    #[test]
    fn a_fenced_prompts_instructions_write_the_prompt_cache() {
        let fenced = crate::curate::prompt(
            "English",
            "## claude session s\nL1 [user] Hi.\n",
            "",
            "",
            crate::curate::MAX_CARDS,
        );
        let schema = json!({"type": "object"});
        let none = serde_json::Map::new();
        let content =
            &anthropic_request("m", &fenced, &schema, &none, false)["messages"][0]["content"];
        let at = fenced.find(crate::curate::FENCE_SENTENCE).unwrap();
        assert_eq!(content[0]["text"], fenced[..at]);
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(content[1]["text"], fenced[at..]);
        assert!(content[1].get("cache_control").is_none(), "{content}");
        // Decision 107: a three-part prompt's instructions end at its stretch's fence sentence, and
        // are marked the same; the stretch and the tail follow, unmarked.
        let parts =
            crate::curate::prompt_parts("English", "## claude session s\nL1 Hi.\n", "", "", 5, 1);
        let content =
            &anthropic_request("m", &parts, &schema, &none, false)["messages"][0]["content"];
        let at = parts.find(crate::curate::FENCE_SENTENCE).unwrap();
        assert_eq!(content[0]["text"], parts[..at]);
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(content[1]["text"], parts[at..]);
        assert!(content[1].get("cache_control").is_none(), "{content}");
        let hour = json!({"cache_control": {"type": "ephemeral", "ttl": "1h"}});
        let content = &anthropic_request("m", &fenced, &schema, hour.as_object().unwrap(), false)["messages"]
            [0];
        assert_eq!(content["content"][0]["cache_control"]["ttl"], "1h");
        let plain = anthropic_request("m", PROBE_PROMPT, &schema, &none, false);
        assert_eq!(plain["messages"][0]["content"], PROBE_PROMPT);
        // Decision 86: a summary's or My Work's call keeps the entry for an hour; an entry's own
        // `cache_control` still wins, and one that marks a block of its own keeps five minutes,
        // since its markers come first and an hour's may not follow five minutes'.
        let content =
            &anthropic_request("m", &fenced, &schema, &none, true)["messages"][0]["content"];
        assert_eq!(
            content[0]["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );
        assert!(content[1].get("cache_control").is_none(), "{content}");
        let five = json!({"cache_control": {"type": "ephemeral"}});
        let content = &anthropic_request("m", &fenced, &schema, five.as_object().unwrap(), true)["messages"]
            [0]["content"];
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        // A request has at most four breakpoints, automatic caching's among them (Codex on #636):
        // an entry whose own take all four leaves the prompt whole, and one with three gets ours.
        let marked = json!({"type": "text", "text": "s", "cache_control": {"type": "ephemeral"}});
        let three = json!({"system": [marked, marked, marked]});
        let mut four = three.clone();
        four["cache_control"] = json!({"type": "ephemeral"});
        let five = json!({"system": [marked, marked, marked, marked, marked]});
        for (extra, split) in [(&three, true), (&four, false), (&five, false)] {
            for hour in [false, true] {
                let body =
                    anthropic_request("m", &fenced, &schema, extra.as_object().unwrap(), hour);
                let content = &body["messages"][0]["content"];
                assert_eq!(content.is_array(), split, "{extra}");
                if split {
                    assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
                } else {
                    assert_eq!(content, &json!(fenced), "{extra}");
                }
            }
        }
    }

    /// Decision 86: the role sets the lifetime of oboete's marker on a Messages request. A summary's
    /// and My Work's keep the instructions' entry for an hour, the curator's five minutes, and a
    /// connection test marks nothing.
    #[test]
    fn a_summary_and_my_work_keep_their_messages_cache_for_an_hour() {
        let home = tempfile::tempdir().unwrap();
        let conn = providers_db::open(home.path()).unwrap();
        let sentence = crate::curate::FENCE_SENTENCE;
        let fenced = format!(
            "Sum it up.\n{sentence}=== RECORD a ===`.\n\n=== RECORD a ===\nL1 hi\n=== RECORD a ==="
        );
        let answer = json!({"stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"summary\":\"s\"}"}],
            "usage": {"input_tokens": 12, "output_tokens": 4}});
        let roles = [
            ("summary", Some("1h")),
            (crate::my_work::ROLE, Some("1h")),
            ("curator", None),
        ];
        for (role, ttl) in roles {
            let (url, request) = serve_once(answer.to_string().into_bytes(), "");
            let mut p = stub(url);
            if let Provider::Openai { api, .. } = &mut p {
                *api = Api::Anthropic;
            }
            Chain::new(&[p], &conn)
                .run(role, "s", &fenced, &json!({"type": "object"}))
                .unwrap();
            let req = request.recv().unwrap();
            let body: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
            let cache = &body["messages"][0]["content"][0]["cache_control"];
            assert_eq!(cache["type"], "ephemeral", "{role}: {body}");
            assert_eq!(cache["ttl"].as_str(), ttl, "{role}: {body}");
        }
    }

    /// What a call's input is billed as: on a Messages entry a token read from its prompt cache
    /// at a tenth of the input price and one written at 1.25 times, or twice when the entry asks for
    /// a cache of its own anywhere in its `extra`, at the top or on a block (Codex on #636);
    /// elsewhere a written token twice and a read one once. A reported prompt counts every input
    /// token, those its cache wrote and read too.
    #[test]
    fn a_calls_cache_reads_and_writes_are_priced_as_billed() {
        let url = "http://127.0.0.1:9/v1".to_owned();
        let mut messages = stub(url.clone());
        if let Provider::Openai { api, .. } = &mut messages {
            *api = Api::Anthropic;
        }
        assert_eq!(
            messages.billed_input("curator", 10_000, Some(4_000), None),
            6_400.0
        );
        assert_eq!(
            messages.billed_input("curator", 10_000, None, Some(4_000)),
            11_000.0
        );
        assert_eq!(
            messages.billed_input("curator", 10_000, None, None),
            10_000.0
        );
        assert_eq!(messages.input_weight("curator"), 1.25);
        // Decision 86: a summary's or My Work's write is an hour's, at twice the input price.
        for role in ["summary", crate::my_work::ROLE] {
            assert_eq!(
                messages.billed_input(role, 10_000, None, Some(4_000)),
                14_000.0,
                "{role}"
            );
            assert_eq!(messages.input_weight(role), 2.0, "{role}");
        }
        let own = [
            ("cache_control", json!({"type": "ephemeral"})),
            ("cache_control", json!({"type": "ephemeral", "ttl": "1h"})),
            (
                "system",
                json!([{"type": "text", "text": "s",
                    "cache_control": {"type": "ephemeral", "ttl": "1h"}}]),
            ),
        ];
        for (key, value) in own {
            let mut entry = messages.clone();
            if let Provider::Openai { extra, .. } = &mut entry {
                extra.insert(key.into(), value.clone());
            }
            assert_eq!(
                entry.billed_input("curator", 10_000, None, Some(4_000)),
                14_000.0,
                "{value}"
            );
            assert_eq!(entry.input_weight("curator"), 2.0, "{value}");
        }
        let other = stub(url);
        assert_eq!(
            other.billed_input("summary", 10_000, Some(4_000), Some(1_000)),
            11_000.0
        );
        assert_eq!(other.input_weight("summary"), 1.0);
    }

    /// Haiku 5.5's thinking is adaptive and its depth is `output_config.effort`: an entry's own
    /// `output_config` joins the one that carries the schema, which it cannot replace.
    #[test]
    fn a_messages_entrys_output_config_joins_the_schema() {
        let extra = json!({"output_config": {"effort": "low", "format": {"type": "text"}}});
        let body = anthropic_request(
            "m",
            "p",
            &json!({"type": "object"}),
            extra.as_object().unwrap(),
            false,
        );
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    }

    /// ureq keeps an `x-api-key` header on a redirect: a Messages entry follows none, so its key
    /// reaches no other host.
    #[test]
    fn a_messages_entry_follows_no_redirect_with_its_key() {
        let home = tempfile::tempdir().unwrap();
        let key_file = home.path().join("TEST_KEY.md");
        std::fs::write(&key_file, "# synthetic only\nredirect-test-key\n").unwrap();
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let to = leak(format!(
            "Location: http://{}/elsewhere\r\n",
            target.local_addr().unwrap()
        ));
        let (url, _) = serve("302 Found", Vec::new(), to);
        let mut p = stub(url);
        if let Provider::Openai {
            api, key_file: k, ..
        } = &mut p
        {
            *api = Api::Anthropic;
            *k = Some(key_file);
        }
        let e = call(
            &p,
            CURATOR_SYSTEM,
            "p",
            &json!({"type": "object"}),
            None,
            false,
        )
        .unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(302), "http 302"));
        assert!(matches!(target.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    /// Parity row 11: the causes only the owner ends, from a failed call's vetted detail. An
    /// outage, a limit, a moderation refusal or a bare 401 (OpenCode Go's credits and limits) is
    /// none, and a refused key is what `key_rejected` says.
    #[test]
    fn a_rests_cause_is_read_from_the_vetted_detail() {
        for (detail, cause) in [
            ("http 401: invalid_api_key", Some(Cause::Key)),
            ("http 401: authentication_error", Some(Cause::Key)),
            ("http 403: PERMISSION_DENIED", Some(Cause::Key)),
            ("http 401", None),
            ("http 403: 403 (moderation)", None),
            ("http 402", Some(Cause::Credits)),
            ("http 400: billing_error", Some(Cause::Credits)),
            (
                "http 400: invalid_request_error (credits spent)",
                Some(Cause::Credits),
            ),
            ("http 429: insufficient_quota", Some(Cause::Credits)),
            ("http 404: model_not_found", Some(Cause::Model)),
            ("http 404: not_found_error", Some(Cause::Model)),
            ("http 400: model_not_found", Some(Cause::Model)),
            ("http 400: not_found_error", None),
            ("http 404", None),
            ("http 429: rate_limit_exceeded, retry in 60s", None),
            ("http 503: service_unavailable", None),
            ("cooling down after an earlier failure", None),
        ] {
            assert_eq!(Cause::of(detail), cause, "{detail}");
            let status = detail[5..8]
                .parse()
                .ok()
                .filter(|_| detail.starts_with("http "));
            assert_eq!(
                refused_key(status, detail),
                cause == Some(Cause::Key),
                "{detail}"
            );
        }
    }
