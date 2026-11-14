    use super::*;

    /// `codex features list` as 0.155.1 answers it: an invalid `web_search` refused by name.
    const FEATURES: &str = "case \"$*\" in *web_search=*) \
        echo 'Error: unknown variant `oboete-probe`, expected one of `disabled`, `cached`' >&2; \
        echo 'in `web_search`' >&2; exit 1;; esac; for f in plugins apps browser_use \
        browser_use_external in_app_browser computer_use image_generation; do \
        echo \"$f  stable  false\"; done; echo 'shell_tool  stable  true'; \
        case \"$CODEX_HOME\" in */codex-home) ;; *) echo 'from_user_config  stable  true';; esac";

    /// A fake codex whose `sandbox` runs `body` with the probe's argv as "$@"; every call is
    /// appended to `calls`.
    fn fake(dir: &Path, features: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("codex");
        let calls = dir.join("calls");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho \"$1\" >> '{}'\ncase \"$1\" in\n\
                 --version) echo 'codex-cli 9.9.9';;\n\
                 features) {features};;\n\
                 sandbox) while [ \"$1\" != -- ]; do shift; done; shift\n{body};;\n\
                 exec) {EXEC};;\n\
                 esac\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    /// codex exec as 0.155.1 ran the scripted model's calls in the dogfood user, told to the model
    /// by `post <agent> <outputs>`: each action refused in the sandbox and escalated, in the root
    /// and in the sub-agent.
    const EXEC: &str = r#"for a in "$@"; do case "$a" in model_providers.oboeteprobe.base_url=*)
        url=${a#*=}; url=${url#\"}; url=${url%\"};; esac; done
        T='{"type":"additional_tools","tools":[{"type":"namespace","name":"functions","tools":[{"name":"exec"}]},{"type":"namespace","name":"collaboration","tools":[{"name":"spawn_agent"}]}]}'
        post() { curl -sS -o /dev/null -H "Authorization: Bearer $OBOETE_PROBE_KEY" --data "{\"client_metadata\":{\"x-codex-turn-metadata\":\"{\\\"agent_name\\\":\\\"$1\\\"}\"},\"input\":[$2,$T]}" "$url/responses"; }
        out() { printf '{"type":"custom_tool_call_output","call_id":"call_%s","output":"%s"}' "$1" "$2"; }
        S="cat:1:cat: x: No such file or directory\ntouch:1:touch: cannot touch x\ncurl:${CURL:-7}:curl: (7) Failed\n"
        E="cat:threw:rejected\ntouch:threw:rejected\ncurl:threw:rejected\n"
        post /root "$(out root_0 "$S"),$(out root_1 "$E"),$(out root_2 spawned)"
        post /root/probe "$(out sub_0 "$S"),$(out sub_1 "$E")""#;

    /// The refusals codex 0.155.1 gave under the probe profile in the dogfood user.
    const REFUSES: &str = "case \"$1\" in true) exit 0;;\n\
        touch) echo \"touch: cannot touch '$2': No such file or directory\" >&2; exit 1;;\n\
        cat) echo \"cat: $2: No such file or directory\" >&2; exit 1;;\n\
        curl) echo 'curl: (7) Failed to connect' >&2; exit 7;;\n\
        esac";

    fn gate_with(exe: &Path, home: &Path) -> (Connection, Gate) {
        let db = crate::providers_db::open(home).unwrap();
        let g = gate_codex(&db, exe, home).unwrap();
        (db, g)
    }

    #[test]
    fn codex_passes_when_every_probe_is_refused_and_is_probed_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake(dir.path(), FEATURES, REFUSES);
        let (db, g) = gate_with(&exe, dir.path());
        assert_eq!(g, Gate::Passed);
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(gate_codex(&db, &exe, dir.path()).unwrap(), Gate::Passed);
        let again = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(
            again.lines().count(),
            2 * calls.lines().count(),
            "probed again"
        );
        // #904: the line is the last probe's, which may be days old, and says when it was.
        let at = |db: &Connection| -> String {
            let ts: i64 = db
                .query_row("SELECT MAX(ts) FROM isolation", [], |r| r.get(0))
                .unwrap();
            crate::db::utc(ts)
        };
        let version = version(&exe, dir.path()).unwrap();
        assert_eq!(
            doctor(&db).unwrap(),
            [format!(
                "codex ({version}): cannot act, curates (as of {})",
                at(&db)
            )]
        );
        // A codex that stops answering fails before the probe, and doctor shows that, not the pass.
        std::fs::write(&exe, "#!/bin/sh\nexit 1\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(matches!(
            gate_codex(&db, &exe, dir.path()).unwrap(),
            Gate::Failed(_)
        ));
        assert_eq!(
            doctor(&db).unwrap(),
            [format!(
                "codex (unknown version): skipped as a curator: codex --version did not answer (as of {})",
                at(&db)
            )]
        );
        // A row of long ago says its own time, not now's.
        db.execute("UPDATE isolation SET ts = 1_794_303_586_000", [])
            .unwrap();
        assert!(
            doctor(&db).unwrap()[0].ends_with("(as of 2026-11-10 09:39 UTC)"),
            "{:?}",
            doctor(&db).unwrap()
        );
    }

    #[test]
    fn codex_fails_when_a_probe_reads_writes_or_fetches() {
        for (leak, why) in [
            ("cat) cat \"$2\";;", "read a file"),
            ("touch) touch \"$2\";;", "wrote outside"),
            (
                "curl) curl -sS --noproxy '*' --max-time 2 \"$7\";;",
                "reached the network",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let body = REFUSES.replacen("case \"$1\" in", &format!("case \"$1\" in {leak}"), 1);
            let (_, g) = gate_with(&fake(dir.path(), FEATURES, &body), dir.path());
            assert!(
                matches!(&g, Gate::Failed(w) if w.contains(why)),
                "{leak}: {g:?}"
            );
        }
    }

    #[test]
    fn a_probe_the_sandbox_could_not_start_is_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let body = REFUSES.replacen(
            "case \"$1\" in",
            "case \"$1\" in touch) echo \"panicked: Failed to execvp $1\" >&2; exit 101;;",
            1,
        );
        let (_, g) = gate_with(&fake(dir.path(), FEATURES, &body), dir.path());
        assert!(
            matches!(&g, Gate::Failed(w) if w.contains("touch probe did not run")),
            "{g:?}"
        );
    }

    /// Live, with `--ignored`, in the dogfood user only: the installed codex under the curator's
    /// environment and profile.
    #[test]
    #[ignore]
    fn live_codex_cannot_act_under_the_curator_profile() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::providers_db::open(dir.path()).unwrap();
        let home = crate::config::home_dir();
        assert_eq!(
            gate_codex(&db, Path::new("codex"), &home).unwrap(),
            Gate::Passed
        );
        println!("{:?}", doctor(&db).unwrap());
    }

    #[test]
    fn a_fetch_the_model_makes_through_codex_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake(dir.path(), FEATURES, REFUSES);
        let script = std::fs::read_to_string(&exe).unwrap();
        std::fs::write(&exe, script.replace("${CURL:-7}", "28")).unwrap();
        let (_, g) = gate_with(&exe, dir.path());
        assert_eq!(
            g,
            Gate::Failed("the curl probe was not refused (28)".into())
        );
    }

    #[test]
    fn a_probe_that_stalls_fails_the_gate_in_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let (_, g) = gate_with(&fake(dir.path(), "sleep 60", REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex features list: did not finish in 5 s".into())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn a_descendant_holding_the_output_open_fails_the_gate_in_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let pid = dir.path().join("pid");
        let stalls = format!("(sleep 60 & echo $! > '{}')", pid.display());
        let (_, g) = gate_with(&fake(dir.path(), &stalls, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex features list: did not finish in 5 s".into())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        // The descendant went with it: it no longer runs. An orphan's zombie is its new parent's to
        // reap (PID 1 in a container without an init may never do it).
        let pid = std::fs::read_to_string(&pid).unwrap();
        let alive = || {
            Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .is_ok_and(|o| {
                    let stat = String::from_utf8_lossy(&o.stdout);
                    !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
                })
        };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while alive() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(), "the descendant {} still runs", pid.trim());
    }

    #[test]
    fn a_codex_that_no_longer_reads_web_search_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let ignores = FEATURES.replace("*web_search=*)", "*web_search_renamed=*)");
        let (_, g) = gate_with(&fake(dir.path(), &ignores, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex does not refuse an invalid web_search setting".into())
        );
    }

    #[test]
    fn a_feature_on_that_was_not_reviewed_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features = FEATURES.to_owned() + "; echo 'new_hosted_tool  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex feature new_hosted_tool is on and has not been reviewed".into())
        );
    }

    /// codex 0.160.0 turns `write_stdin_approval` on by default: input to a terminal that was
    /// launched with more than the current permissions waits for an approval (off, it waits for
    /// none). It offers the model no tool, so it is reviewed and the gate passes with it on.
    #[test]
    fn the_stdin_approval_of_codex_0_160_does_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features = FEATURES.to_owned() + "; echo 'write_stdin_approval  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(g, Gate::Passed);
    }

    /// codex 0.163.0 turns eight more features on by default, none of them a tool the model is
    /// offered (`provider::CODEX_ON`): the gate passes with them on.
    #[test]
    fn the_features_of_codex_0_163_do_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let on: String = [
            "api_key_cyber_access_programs",
            "api_key_model_discovery",
            "browser_annotation_api",
            "cli_daybreak",
            "code_mode_interrupt",
            "in_app_voice",
            "instant_interrupt",
            "ultrafast_mode",
        ]
        .iter()
        .map(|f| format!("; echo '{f}  stable  true'"))
        .collect();
        let (_, g) = gate_with(
            &fake(dir.path(), &(FEATURES.to_owned() + &on), REFUSES),
            dir.path(),
        );
        assert_eq!(g, Gate::Passed);
    }

    /// codex 0.164.0 turns three more features on by default, none of them a tool the model is
    /// offered (`provider::CODEX_ON`): the gate passes with them on.
    #[test]
    fn the_features_of_codex_0_164_do_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let on: String = [
            "bedrock_setup_wizard",
            "incremental_tools",
            "mcp_oauth_refresh_coordination",
        ]
        .iter()
        .map(|f| format!("; echo '{f}  stable  true'"))
        .collect();
        let (_, g) = gate_with(
            &fake(dir.path(), &(FEATURES.to_owned() + &on), REFUSES),
            dir.path(),
        );
        assert_eq!(g, Gate::Passed);
    }

    /// codex 0.166.0 turns three more features on by default, none of them a tool the model is
    /// offered (`provider::CODEX_ON`): the gate passes with them on.
    #[test]
    fn the_features_of_codex_0_166_do_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let on: String = [
            "plugin_discovery_prefetch",
            "prediction_preserve_parent_tools",
            "reasoning_effort_override",
        ]
        .iter()
        .map(|f| format!("; echo '{f}  stable  true'"))
        .collect();
        let (_, g) = gate_with(
            &fake(dir.path(), &(FEATURES.to_owned() + &on), REFUSES),
            dir.path(),
        );
        assert_eq!(g, Gate::Passed);
    }

    /// codex 0.167.0 turns `deferred_tool_world_state` on by default: the namespaces of the tools
    /// `tool_search` loads on request are named in the model's world state, not in that tool's
    /// description. It offers the model no tool (`provider::CODEX_ON`): the gate passes with it on.
    #[test]
    fn the_deferred_tool_world_state_of_codex_0_167_does_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features = FEATURES.to_owned() + "; echo 'deferred_tool_world_state  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(g, Gate::Passed);
    }

    #[test]
    fn a_hosted_tool_left_on_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features =
            FEATURES.replace("image_generation;", ";") + "; echo 'image_generation  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex feature image_generation is not off".into())
        );
    }
