    use super::*;
    use crate::{knowledge, raw, worker};

    /// Audit L1: a git that does not end in time is killed and waited for, so none is left a
    /// zombie under a worker that stays. Its index here is a pipe no one writes to, which git
    /// waits on for good.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_git_status_that_does_not_end_is_killed_and_reaped() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let done = Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(done.status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        let index = repo.path().join(".git/index");
        let _ = std::fs::remove_file(&index);
        assert!(
            Command::new("mkfifo")
                .arg(&index)
                .status()
                .unwrap()
                .success()
        );
        let began = Instant::now();
        assert_eq!(status(repo.path()), None);
        assert!(began.elapsed() >= GIT_TIMEOUT, "{:?}", began.elapsed());
        // This process's children named `git` that ended and were not waited for (`Z` in
        // `/proc/<pid>/stat`, whose name may hold spaces and parentheses).
        let zombies = || {
            let me = std::process::id().to_string();
            std::fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .filter_map(|entry| std::fs::read_to_string(entry.path().join("stat")).ok())
                .filter(|stat| {
                    let Some((head, rest)) = stat.rsplit_once(')') else {
                        return false;
                    };
                    let mut fields = rest.split_whitespace();
                    head.ends_with("(git")
                        && fields.next() == Some("Z")
                        && fields.next() == Some(&me)
                })
                .count()
        };
        let until = Instant::now() + Duration::from_secs(10);
        while zombies() > 0 {
            assert!(Instant::now() < until, "the killed git stays a zombie");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn ev(kind: &str, session: &str, ts: i64, cwd: &Path, body: Value) -> Event {
        Event {
            session: session.into(),
            kind: kind.into(),
            ts,
            repo: Some("r".into()),
            branch: Some("main".into()),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            body: body.to_string(),
            ..raw::test_event("")
        }
    }

    fn tool(cwd: &Path, ts: i64, tool: &str, input: Value, output: &str, failed: bool) -> Event {
        let body = serde_json::json!({"tool": tool, "input": input.to_string(), "output": output,
            "failed": failed});
        ev("tool", "s1", ts, cwd, body)
    }

    fn manifest(home: &Path) -> (String, i64) {
        let k = knowledge::open(home).unwrap();
        let device = raw::open(home).unwrap().device().to_owned();
        k.query_row(
            "SELECT text, built_at FROM manifests WHERE repo = 'r' AND branch = 'main' AND device = ?1",
            [device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// A session's records: a directive, a failing and a fixed command, a todo list, an edit.
    fn session(home: &Path, cwd: &Path) {
        let mut raw = raw::open(home).unwrap();
        let min = 60_000;
        for e in [
            ev(
                "prompt",
                "s1",
                min,
                cwd,
                serde_json::json!({"prompt": "from now on run the linter first"}),
            ),
            tool(
                cwd,
                2 * min,
                "Bash",
                serde_json::json!({"command": "cargo lint"}),
                "no such command",
                true,
            ),
            tool(
                cwd,
                3 * min,
                "Bash",
                serde_json::json!({"command": "cargo test"}),
                "Exit code: 101\nfailed",
                false,
            ),
            tool(
                cwd,
                4 * min,
                "Bash",
                serde_json::json!({"command": "cargo lint", "description": "again"}),
                "ok",
                false,
            ),
            tool(
                cwd,
                5 * min,
                "TodoWrite",
                serde_json::json!({"todos": [{"content": "fix the test", "status": "in_progress"}]}),
                "",
                false,
            ),
            tool(
                cwd,
                6 * min,
                "Edit",
                serde_json::json!({"file_path": format!("{}/src/lib.rs", cwd.display())}),
                "",
                false,
            ),
            ev(
                "reply",
                "s1",
                7 * min,
                cwd,
                serde_json::json!({"assistant": "The test still fails."}),
            ),
            ev(
                "prompt",
                "s2",
                8 * min,
                cwd,
                serde_json::json!({"prompt": "from now on run the linter first"}),
            ),
            ev(
                "prompt",
                "s2",
                9 * min,
                cwd,
                serde_json::json!({"prompt": "look at it"}),
            ),
        ] {
            raw.append(&e).unwrap();
        }
    }

    /// MUST-M9 at read time (D3): the checkout's records past the curation checkpoint, once the
    /// oldest has waited two minutes, with the minutes it has waited when SessionStart reads, from
    /// the same stored row. A window op over some of them leaves the rest, a record a window stops
    /// inside of still counts, one over all of them leaves none, and a restore that lost window
    /// ops counts them again, each read with no worker run between (#204).
    #[test]
    fn the_backlog_gains_its_minutes_when_read_later() {
        use crate::raw::OpKind;
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let store = raw::open(home.path()).unwrap();
        let min = 60_000;
        let backlog = |now: i64| {
            lines_of(
                &shown_at(home.path(), &store, "none", now).unwrap().text,
                "Backlog",
            )
        };
        // With summaries off, as a new home is, nothing waits to be curated: no backlog, however
        // long the records have been there. One whose `[summary]` does not load stops curation
        // too, and its backlog is shown.
        assert_eq!(backlog(60 * min), Vec::<String>::new());
        let config = home.path().join("config.toml");
        std::fs::write(&config, "[summary]\ncurate = \"yes\"\n").unwrap();
        assert_eq!(backlog(60 * min), ["9 record(s) / 59 min not yet curated"]);
        curates(home.path());
        // The first record is at minute 1.
        assert_eq!(backlog(3 * min), Vec::<String>::new());
        assert_eq!(
            backlog(3 * min + 1),
            ["9 record(s) / 2 min not yet curated"]
        );
        assert_eq!(backlog(60 * min), ["9 record(s) / 59 min not yet curated"]);
        let window = |to: i64, to_offset: Option<i64>| {
            let op = serde_json::json!({"from_seq": 1, "from_offset": null, "to_seq": to,
                "to_offset": to_offset, "outcome": "curated"});
            raw::open(home.path())
                .unwrap()
                .append_ops(&[(OpKind::Window, op)])
                .unwrap();
            backlog(60 * min)
        };
        assert_eq!(window(5, Some(3)), ["5 record(s) / 55 min not yet curated"]);
        assert_eq!(window(5, None), ["4 record(s) / 54 min not yet curated"]);
        assert_eq!(window(9, None), Vec::<String>::new());
        // A restore that lost the last two window ops while knowledge.db stays.
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq > 1", [])
            .unwrap();
        assert_eq!(backlog(60 * min), ["5 record(s) / 55 min not yet curated"]);
    }

    /// MUST-M8 at read time (D3): the repository's other sessions with a record in the 30 minutes
    /// before SessionStart reads and no end, with their agent, branch and minutes since; never the
    /// reader's own, an ended one or one idle longer, and none once all are.
    #[test]
    fn other_live_sessions_are_named_at_read_time() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path()); // s1 until minute 7, s2 at minutes 8 and 9
        let mut store = raw::open(home.path()).unwrap();
        let min = 60_000;
        let codex = Event {
            agent: "codex".into(),
            branch: Some("feature".into()),
            ..ev(
                "prompt",
                "s4",
                10 * min,
                cwd.path(),
                serde_json::json!({"prompt": "x"}),
            )
        };
        store.append(&codex).unwrap();
        store
            .append(&ev(
                "end",
                "s5",
                10 * min,
                cwd.path(),
                serde_json::json!({}),
            ))
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let others = |session: &str, now: i64| {
            let store = raw::open(home.path()).unwrap();
            let start = shown_at(home.path(), &store, session, now);
            lines_of(
                &start.map(|s| s.text).unwrap_or_default(),
                "Other active sessions",
            )
        };
        assert_eq!(
            others("s2", 12 * min),
            [
                "- codex on feature, 2 min ago",
                "- claude on main, 5 min ago"
            ]
        );
        assert_eq!(
            others("s4", 38 * min),
            ["- claude on main, 29 min ago"] // s1's last record is past 30 minutes
        );
        assert_eq!(others("none", 41 * min), Vec::<String>::new());
        // Gated with the rules of the read.
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"b\", regex = 'feat[a-z]+' }]\n",
        )
        .unwrap();
        assert_eq!(
            others("s2", 12 * min)[0],
            "- codex on [REDACTED], 2 min ago"
        );
        // A tombstone the worker has not applied may change the facts: none until it has.
        store
            .append_tombstone(Target::Record {
                device: store.device().to_owned(),
                seq: 10,
            })
            .unwrap();
        assert_eq!(others("s2", 12 * min), Vec::<String>::new());
        worker::run_once(home.path()).unwrap();
        assert_eq!(others("s2", 12 * min), ["- claude on main, 5 min ago"]);
    }

    /// MUST-M21 at SessionStart (D4): the words of what the last failing command ran choose the
    /// claims with bodies too, so an old lesson about it gets its body while a lesson about another
    /// failure stays an index line. `session` leaves `cargo test` failing.
    #[test]
    fn a_lesson_about_the_failing_command_gets_its_body() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let mut ops = vec![
                    claimed(
                        store,
                        said(cwd, DAY, "Run cargo test with the full features."),
                        "lesson",
                        "decided",
                        vec![],
                    )
                    .0,
                    claimed(
                        store,
                        said(cwd, 2 * DAY, "npm audit needs the lockfile."),
                        "lesson",
                        "decided",
                        vec![],
                    )
                    .0,
                ];
                for i in 1..=11 {
                    let e = said(cwd, (2 + i) * DAY, &format!("Rule {i:02}."));
                    ops.push(claimed(store, e, "decision", "decided", vec![]).0);
                }
                ops
            },
            None,
        );
        assert!(
            text.contains("## Last failing command\nBash: cargo test"),
            "{text}"
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10);
        assert!(
            bodies.contains(&"- 1970-01-02 lesson: Run cargo test with the full features.".into()),
            "{bodies:?}"
        );
        let more = lines_of(&text, "More from memory");
        assert!(
            more.iter()
                .any(|l| l.ends_with("lesson: npm audit needs the lockfile.")),
            "{more:?}"
        );
    }

    /// Codex's security review of Task 7: each claim body is gated before it is formatted, so a
    /// rule anchored to the whole field, or to a line of it, still matches it behind the line's
    /// date and kind, and once its lines are joined into one.
    #[test]
    fn a_rule_anchored_to_a_claim_body_or_its_line_still_hides_it() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (key, _) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Use key zq-778899."),
            "decision",
            "decided",
            vec![],
        );
        let (pin, _) = claimed(
            &mut store,
            said(cwd.path(), 2 * DAY, "Deploy with the pin.\npin: zq-112233"),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[key, pin]).unwrap();
        worker::run_once(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        for code in ["778899", "112233"] {
            assert!(before.contains(code), "{before}");
        }
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [\
             { id = \"key\", regex = '^Use key (zq-[0-9]{6})\\.$', secret_group = 1 }, \
             { id = \"pin\", regex = '^pin: (zq-[0-9]{6})$', secret_group = 1 }]\n",
        )
        .unwrap();
        let after = shown(home.path(), &store).unwrap();
        for code in ["778899", "112233"] {
            assert!(!after.contains(code), "{after}");
        }
        assert!(after.contains("Use key "), "{after}");
    }

    #[test]
    fn interacting_field_and_formatted_rules_hide_claim_lines() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let body = "Header\nalpha code 654321";
        let (claim, _) = claimed(
            &mut store,
            said(cwd.path(), DAY, body),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[claim]).unwrap();
        worker::run_once(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        assert!(before.contains("Header alpha code 654321"), "{before}");
        let before = lines_of(&before, "Decisions and open items").join("\n");
        for regex in [
            "Header alpha code ([0-9]{6})",
            "(?m)^alpha code ([0-9]{6})$",
            "(?m)^- (?:1970-01-02 decision: )?Header alpha code ([0-9]{6})$",
        ] {
            std::fs::write(
                home.path().join("config.toml"),
                format!(
                    "[redaction]\nextra_rules = [\
                     {{ id = 'field', regex = '^Header\\n(alpha) code [0-9]{{6}}$', secret_group = 1 }}, \
                     {{ id = 'other', regex = '{regex}', secret_group = 1 }}]\n"
                ),
            )
            .unwrap();
            let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
            if !regex.starts_with("(?m)^alpha") {
                assert!(!crate::redact::outbound_with(&before, &rules).contains("654321"));
            }
            let after = shown_at(home.path(), &store, "none", NOW).unwrap();
            assert_eq!(
                after
                    .text
                    .matches("Header [REDACTED] code [REDACTED]")
                    .count(),
                1,
                "{}",
                after.text
            );
            if regex.starts_with("(?m)^- ") {
                assert!(after.shown.is_empty(), "{after:?}");
            }
        }
    }

    /// #329: the field-only masked packet remains an independent view. A packet rule masking
    /// `code` must not take away the context of a cascade that sees the field's masked `alpha`.
    #[test]
    fn the_field_masked_packets_cascade_keeps_its_findings() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (claim, _) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Header\nalpha code 654321"),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[claim]).unwrap();
        worker::run_once(home.path()).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            r#"[redaction]
extra_rules = [
  { id = "field", regex = '^Header\n(alpha) code [0-9]{6}$', secret_group = 1 },
  { id = "packet", regex = '(?m)^- 1970-01-02 decision: Header alpha (code) [0-9]{6}$', secret_group = 1 },
  { id = "cascade", regex = '(?m)^- 1970-01-02 decision: Header \[REDACTED\] code ([0-9]{6})$', secret_group = 1 },
]
"#,
        )
        .unwrap();
        let after = shown_at(home.path(), &store, "none", NOW).unwrap();
        assert!(
            after
                .text
                .contains("Header [REDACTED] [REDACTED] [REDACTED]"),
            "{}",
            after.text
        );
        assert!(!after.text.contains("654321"), "{}", after.text);
        assert!(after.shown.is_empty());
    }

    #[test]
    fn a_packet_rule_keeps_the_original_clipped_view() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let name = "a".repeat(60);
        let body = format!("Header\n{name} code 654321 {}", "x ".repeat(300));
        let (claim, _) = claimed(
            &mut store,
            said(cwd.path(), DAY, &body),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[claim]).unwrap();
        worker::run_once(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        let line = lines_of(&before, "Decisions and open items").remove(0);
        assert!(line.contains("654321") && line.ends_with('…'), "{line}");
        let packet = regex::escape(&line).replace("654321", "([0-9]{6})");
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[redaction]\nextra_rules = [\
                 {{ id = 'field', regex = '^Header\\n({name}) code', secret_group = 1 }}, \
                 {{ id = 'packet', regex = '(?m)^{packet}$', secret_group = 1 }}]\n"
            ),
        )
        .unwrap();
        let after = shown(home.path(), &store).unwrap();
        assert!(!after.contains("654321"), "{after}");
    }

    #[test]
    fn a_packet_rule_keeps_spaces_around_a_field_mask() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let name = "a".repeat(60);
        let body = format!("Head\n{name} code 654321 {}code 112233", "x ".repeat(163));
        let (claim, _) = claimed(
            &mut store,
            said(cwd.path(), DAY, &body),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[claim]).unwrap();
        worker::run_once(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        assert!(!before.contains("112233"), "{before}");
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[redaction]\nextra_rules = [\
                 {{ id = 'field', regex = '^Head(\\n{name} )code', secret_group = 1 }}, \
                 {{ id = 'packet', regex = '(?m)^- 1970-01-02 decision: Head {name} code 654321 (?:x )+code ([0-9]{{6}})$', secret_group = 1 }}]\n"
            ),
        )
        .unwrap();
        let after = shown(home.path(), &store).unwrap();
        assert!(!after.contains("112233"), "{after}");
        assert!(after.contains("code [REDACTED]"), "{after}");
    }

    #[test]
    fn field_projection_preserves_private_blocks_whitespace_and_utf8() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let mut ops = Vec::new();
        for (i, body) in [
            "左 <private>hidden</private> 右",
            "a   b",
            "山田\nalpha code 654321",
        ]
        .into_iter()
        .enumerate()
        {
            ops.push(
                claimed(
                    &mut store,
                    said(cwd.path(), DAY + i as i64, body),
                    "decision",
                    "decided",
                    vec![],
                )
                .0,
            );
        }
        store.append_ops(&ops).unwrap();
        worker::run_once(home.path()).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [\
             { id = 'space', regex = 'a  ( b)', secret_group = 1 }, \
             { id = 'name', regex = '^山田\\n(alpha) code [0-9]{6}$', secret_group = 1 }, \
             { id = 'code', regex = '山田 alpha code ([0-9]{6})', secret_group = 1 }]\n",
        )
        .unwrap();
        let after = shown(home.path(), &store).unwrap();
        let lines = lines_of(&after, "Decisions and open items");
        for body in ["左 右", "a [REDACTED]", "山田 [REDACTED] code [REDACTED]"] {
            assert!(
                lines.contains(&format!("- 1970-01-02 decision: {body}")),
                "{after}"
            );
        }
        assert!(!after.contains("hidden"), "{after}");
    }

    /// Spec 4.7, 4.8: `Start::shown` names each claim shown, with its body or only its index line,
    /// and its body's fingerprint, never one the cut dropped.
    #[test]
    fn start_names_the_claims_it_shows_and_how() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let mut ops = Vec::new();
        let mut uids = Vec::new();
        for i in 1..=12 {
            let e = said(cwd.path(), i * DAY, &format!("Rule {i:02}."));
            let (op, uid) = claimed(&mut store, e, "decision", "decided", vec![]);
            ops.push(op);
            uids.push(uid);
        }
        store.append_ops(&ops).unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let start = |cap| {
            text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap()
        };
        let all = start(usize::MAX);
        let shown = |start: &Start, body: bool| {
            let mut uids: Vec<String> = start
                .shown
                .iter()
                .filter(|s| s.body == body)
                .map(|s| s.uid.clone())
                .collect();
            uids.sort();
            uids
        };
        let mut newest: Vec<String> = uids[2..].to_vec();
        newest.sort();
        assert_eq!(shown(&all, true), newest);
        // The 2 oldest are index lines.
        let mut oldest = uids[..2].to_vec();
        oldest.sort();
        assert_eq!(shown(&all, false), oldest);
        for (i, uid) in uids.iter().enumerate() {
            let s = all.shown.iter().find(|s| s.uid == *uid).unwrap();
            assert_eq!(s.fp, fingerprint(&format!("Rule {:02}.", i + 1)));
        }
        // A cap that ends inside the bodies: the claims past it, and the index, are not shown.
        let at = all.text.find("- 1970-01-09 decision").unwrap();
        let cut = start(all.text[..at].chars().count());
        let mut kept = uids[8..].to_vec();
        kept.sort();
        assert_eq!((shown(&cut, true), shown(&cut, false)), (kept, Vec::new()));
    }

    #[test]
    fn identical_body_lines_count_only_the_occurrence_before_the_cut() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let common = "Shared words ".repeat(40);
        let (older, _) = claimed(
            &mut store,
            said(cwd.path(), DAY + 1, &format!("{common}older tail")),
            "decision",
            "decided",
            vec![],
        );
        let (newer, uid) = claimed(
            &mut store,
            said(cwd.path(), DAY + 2, &format!("{common}newer tail")),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[older, newer]).unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let start = |cap| {
            text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap()
        };
        let all = start(usize::MAX);
        let lines = lines_of(&all.text, "Decisions and open items");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], lines[1]);
        let end = all.text.find(&lines[0]).unwrap() + lines[0].len() + 1;
        let cut = start(all.text[..end].chars().count());
        let shown: Vec<_> = cut.shown.iter().map(|s| s.uid.as_str()).collect();
        assert_eq!(shown, [uid.as_str()], "{}", cut.text);
    }

    /// Task 8: a row built before the read-time lines carries the ruleset without the format tag:
    /// SessionStart leaves it out, and the worker's next pass builds it again, once.
    #[test]
    fn old_manifest_rows_are_rebuilt() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let k = knowledge::open(home.path()).unwrap();
        k.execute(
            "UPDATE manifests SET ruleset = ?1, text = '## As of\nold\n', built_at = 0",
            [rules.version()],
        )
        .unwrap();
        // Nothing of it is shown: with summaries off there is no backlog line either.
        assert!(!shown(home.path(), &store).is_some_and(|text| text.contains("old")));
        worker::run_once(home.path()).unwrap();
        let (text, built) = manifest(home.path());
        assert!(text.contains("prompt: look at it") && built > 0, "{text}");
        let ruleset: String = k
            .query_row("SELECT ruleset FROM manifests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ruleset, stamp(rules.version()));
        worker::run_once(home.path()).unwrap();
        assert_eq!(manifest(home.path()), (text, built));
    }

    #[test]
    fn the_manifest_is_the_same_bytes_for_the_same_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap(); // no repository: the git state stays empty
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let (first, built) = manifest(home.path());
        // Said twice, shown once.
        assert_eq!(
            first
                .matches("\"from now on run the linter first\"")
                .count(),
            1,
            "{first}"
        );
        assert!(
            first.contains("cargo test") && !first.contains("cargo lint:"),
            "{first}"
        );
        assert!(first.contains("[in_progress] fix the test"), "{first}");
        assert!(first.contains("- src/lib.rs"), "{first}");
        assert!(first.contains("prompt: look at it"), "{first}");
        worker::run_once(home.path()).unwrap(); // nothing new: nothing built
        assert_eq!(manifest(home.path()), (first.clone(), built));
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            std::fs::remove_file(home.path().join(f)).ok();
        }
        worker::run_once(home.path()).unwrap(); // built again from raw
        assert_eq!(manifest(home.path()).0, first);
    }

    #[test]
    fn a_rule_anchored_to_a_field_is_applied_to_each_field_shown() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let input = serde_json::json!({"command": "run it"});
        store
            .append(&tool(cwd.path(), 1, "acme-secret", input, "no", true))
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^acme-secret$' }]\n",
        )
        .unwrap();
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        assert!(
            shown.contains("failed with: no") && !shown.contains("acme"),
            "{shown}"
        );
    }

    /// When `shown` reads: after most tests' records, so no session is live then, and 14 days
    /// after the epoch, so no claim of theirs is older than 14 days and loses its body (decision
    /// 76).
    const NOW: i64 = 14 * DAY;

    /// The manifest SessionStart shows for checkout (r, main) under the rules the home has now.
    fn shown(home: &Path, store: &Raw) -> Option<String> {
        shown_at(home, store, "none", NOW).map(|s| s.text)
    }

    /// `[summary] curate` on, with no AI to call: a home that curates has a backlog to show.
    fn curates(home: &Path) {
        let config = "providers = []\n[summary]\ncurate = true\n";
        std::fs::write(home.join("config.toml"), config).unwrap();
    }

    /// `text` for checkout (r, main), uncut, read by `session` at `now`.
    fn shown_at(home: &Path, store: &Raw, session: &str, now: i64) -> Option<Start> {
        let rules = crate::capture::Settings::load(home).unwrap().rules;
        text(
            home,
            store,
            "r",
            "main",
            session,
            &rules,
            usize::MAX,
            INDEX,
            now,
        )
        .unwrap()
    }

    #[test]
    fn session_start_reads_the_manifest_without_writing_knowledge_db() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = raw::open(home.path()).unwrap();
        assert_eq!(shown(home.path(), &store), None);
        assert!(!home.path().join("knowledge.db").exists());
        curates(home.path());
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        let (stored, _) = manifest(home.path());
        assert!(
            shown.starts_with(&stored) && shown[stored.len()..].starts_with("## Backlog\n"),
            "{shown}"
        );
    }

    /// A claim op for an event appended now: quoting all of its text, of `kind` and `status`.
    fn claimed(
        store: &mut Raw,
        e: Event,
        kind: &str,
        status: &str,
        supersedes: Vec<String>,
    ) -> ((crate::raw::OpKind, Value), String) {
        let seq = store.append(&e).unwrap();
        let quote = crate::curate::long_text(&e).unwrap();
        let evidence = crate::claims::Evidence {
            device: store.device().to_owned(),
            seq,
            offset: 0,
            length: quote.len() as i64,
            sentence: 0,
            quote: quote.clone(),
            claim_at: None,
        };
        let uid = crate::claims::uid(kind, &evidence);
        let op = crate::claims::ClaimOp {
            id: format!("c{seq}"),
            kind: kind.into(),
            status: status.into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: quote,
            evidence: vec![evidence],
            supersedes,
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
            support: Vec::new(),
        };
        let op = (crate::raw::OpKind::Claim, serde_json::to_value(op).unwrap());
        (op, uid)
    }

    /// Spec 1.7: `oboete rebuild` makes knowledge.db again from raw.db and the op log: the same
    /// current claims and the same manifest, no provider called, no old file left.
    #[test]
    fn rebuilding_gives_the_same_claims_with_no_call() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, old) = claimed(
            &mut store,
            said(1, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, _) = claimed(
            &mut store,
            said(2, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (flaky, _) = claimed(
            &mut store,
            said(3, "The CI test is flaky."),
            "open item",
            "decided",
            vec![],
        );
        store.append_ops(&[tabs, spaces, flaky]).unwrap();
        worker::run_once(home.path()).unwrap();
        let knowledge = home.path().join("knowledge.db");
        // Each read holds raw.db open only while it reads, as a hook does: a rebuild moves
        // knowledge.db aside only while no store is open.
        let snapshot = || {
            let k = rusqlite::Connection::open(&knowledge).unwrap();
            let claims = crate::claims::current(&k, "r").unwrap();
            let store = raw::open(home.path()).unwrap();
            (claims, shown(home.path(), &store).unwrap())
        };
        drop(store);
        // A home that curates, before the first look so both show its backlog: the rebuild
        // still asks no provider (the phase would open providers.db for its pending window, here
        // with no budget, so nothing leaves).
        let config = "[summary]\ncurate = true\n[[providers]]\nkind = \"openai\"\n\
                      name = \"spent\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                      daily_budget = 0\n";
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        let before = snapshot();
        assert_eq!(before.0.len(), 2);
        // Only the old file has it.
        rusqlite::Connection::open(&knowledge)
            .unwrap()
            .execute_batch("CREATE TABLE marker(x)")
            .unwrap();
        worker::rebuild(home.path()).unwrap();
        assert_eq!(snapshot(), before);
        let k = rusqlite::Connection::open(&knowledge).unwrap();
        let marker: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marker, 0);
        assert!(!home.path().join("providers.db").exists());
        let left = std::fs::read_dir(home.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("rebuilding-")
        });
        assert!(!left);
    }

    /// docs/work-state.md L4: work state ops among the claims are no knowledge: the claims are the
    /// ones they would be without them, a rebuild gives the same, and the ops stay.
    #[test]
    fn work_state_ops_are_no_knowledge() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, _) = claimed(
            &mut store,
            said(1, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[tabs]).unwrap();
        let phase = serde_json::json!({"phase": "rc2"});
        store
            .work_state("r", "release", phase.as_object().unwrap(), None)
            .unwrap();
        let (flaky, _) = claimed(
            &mut store,
            said(2, "The CI test is flaky."),
            "open item",
            "decided",
            vec![],
        );
        store.append_ops(&[flaky]).unwrap();
        let entries = store.work_state_entries("r").unwrap();
        drop(store);
        worker::run_once(home.path()).unwrap();
        let claims = || {
            let k = rusqlite::Connection::open(home.path().join("knowledge.db")).unwrap();
            crate::claims::current(&k, "r").unwrap()
        };
        let before = claims();
        assert_eq!(before.len(), 2);
        worker::rebuild(home.path()).unwrap();
        assert_eq!(claims(), before);
        let store = raw::open(home.path()).unwrap();
        assert_eq!(store.work_state_entries("r").unwrap(), entries);
    }

    /// Spec 4.4, 4.9: the repository's delivered decisions and open items, the newest first, read
    /// when the text is: an earlier decision that a later decision of the owner ended is listed
    /// after it, both dated (owner decision 31); a retracted, merely proposed or other
    /// repository's claim is not among them, and reading writes nothing.
    #[test]
    fn the_manifest_lists_the_current_decisions_of_its_repo() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let min = 60_000;
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, old) = claimed(
            &mut store,
            said(10 * min, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, _) = claimed(
            &mut store,
            said(11 * min, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (flaky, _) = claimed(
            &mut store,
            said(12 * min, "Fix the flaky CI test."),
            "open item",
            "decided",
            vec![],
        );
        let (cache, _) = claimed(
            &mut store,
            said(13 * min, "Maybe cache it."),
            "decision",
            "proposed",
            vec![],
        );
        let (gone, _) = claimed(
            &mut store,
            said(14 * min, "Drop the old API."),
            "decision",
            "retracted",
            vec![],
        );
        let other = Event {
            repo: Some("other".into()),
            ..said(15 * min, "Other repo rule.")
        };
        let (elsewhere, _) = claimed(&mut store, other, "decision", "decided", vec![]);
        store
            .append_ops(&[tabs, spaces, flaky, cache, gone, elsewhere])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let knowledge = home.path().join("knowledge.db");
        let before = std::fs::metadata(&knowledge).unwrap().modified().unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            std::fs::metadata(&knowledge).unwrap().modified().unwrap(),
            before
        );
        let section = text
            .split("## Decisions and open items\n")
            .nth(1)
            .unwrap()
            .split("\n## ")
            .next()
            .unwrap();
        assert_eq!(
            section,
            "- 1970-01-01 open item: Fix the flaky CI test.\n\
             - 1970-01-01 decision: Use spaces instead.\n\
             - 1970-01-01 decision, superseded by the 1970-01-01 decision above: Use tabs."
        );
        // Before the owner's directives, as spec 4.9 orders them.
        assert!(
            text.find("## Decisions and open items").unwrap()
                < text.find("## Owner's directives").unwrap()
        );
        // At most the limit, the newest first: the query stops there, not the caller.
        let k = rusqlite::Connection::open(&knowledge).unwrap();
        let newest: Vec<String> = crate::claims::decisions(&k, "r", 1)
            .unwrap()
            .into_iter()
            .map(|c| c.body)
            .collect();
        assert_eq!(newest, ["Fix the flaky CI test."]);
        // Through the indexes, newest first: no scan of the claims, however many there are.
        let sql = format!(
            "EXPLAIN QUERY PLAN {} {}",
            crate::claims::delivered("a.repo = ?1", crate::claims::LINKS_END_DECISIONS),
            crate::claims::DECIDED
        );
        let plan: Vec<String> = k
            .prepare(&sql)
            .unwrap()
            .query_map(("r", 1), |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(plan.iter().all(|p| !p.starts_with("SCAN")), "{plan:?}");
    }

    #[test]
    fn a_tombstone_the_worker_has_not_applied_hides_the_saved_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let device = store.device().to_owned();
        assert!(shown(home.path(), &store).unwrap().contains("look at it"));
        let seq = store
            .after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .find(|r| matches!(&r.item, Item::Event(e) if e.body.contains("look at it")))
            .unwrap()
            .seq;
        store
            .append_tombstone(Target::Record {
                device: device.clone(),
                seq,
            })
            .unwrap();
        // No worker step yet: the saved text still has the prompt, so it is not shown.
        let before = shown(home.path(), &store).unwrap_or_default();
        assert!(!before.contains("## As of"), "{before}");
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        assert!(!shown.contains("look at it"), "{shown}");
    }

    #[test]
    fn a_rewind_takes_the_lost_records_out_of_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        assert!(manifest(home.path()).0.contains("look at it"));
        let c = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > 8", []).unwrap(); // lost commits (MUST-M14)
        drop(c);
        worker::run_once(home.path()).unwrap();
        let text = manifest(home.path()).0;
        assert!(!text.contains("look at it"), "{text}");
        assert!(text.contains("prompt: from now on"), "{text}");
    }

    #[test]
    fn a_lost_tombstone_gives_its_target_back_to_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let device = store.device().to_owned();
        let seq = store
            .after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .find(|r| matches!(&r.item, Item::Event(e) if e.body.contains("look at it")))
            .unwrap()
            .seq;
        store
            .append_tombstone(Target::Record { device, seq })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(!manifest(home.path()).0.contains("look at it"));
        let top = store.max_seq().unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top]) // lost (MUST-M14)
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let text = manifest(home.path()).0;
        assert!(text.contains("look at it"), "{text}"); // as a rebuild of raw now gives
    }

    /// docs/retention.md: an expired record gives the facts its labels give (an event, a prompt)
    /// and none its text gave (the owner's line), and its checkout is built again.
    #[test]
    fn an_expired_record_gives_its_labels_facts() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let event = Event {
            repo: Some("github.com/o/r".into()),
            branch: Some("main".into()),
            ..raw::test_event(&serde_json::json!({"prompt": "Always use tabs."}).to_string())
        };
        let seq = store.append(&event).unwrap();
        let facts = |k: &Connection| -> Vec<String> {
            k.prepare("SELECT fact FROM manifest_facts WHERE seq = ?1 ORDER BY fact")
                .unwrap()
                .query_map([seq], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let mut consumers: Vec<Box<dyn worker::Consumer>> =
            vec![Box::new(Manifest::new(home.path()))];
        let mut k = crate::knowledge::open(home.path()).unwrap();
        worker::drain(&store, &mut k, &mut consumers).unwrap();
        assert!(facts(&k).contains(&"owner".to_owned()), "{:?}", facts(&k));
        store.expire_for_test(&[seq]);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        worker::drain(&store, &mut k, &mut consumers).unwrap();
        assert_eq!(facts(&k), ["event", "prompt"]);
        // Its checkout was marked, so a rebuild with only expired records still builds its text.
        let built: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM manifests WHERE repo = 'github.com/o/r' AND branch = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(built, 1);
    }

    /// X17 (docs/subagents.md): a subagent's stop is no fact of its checkout, live or expired: it
    /// would make an ended session active again, and count as a record to curate.
    #[test]
    fn a_subagent_stop_gives_no_fact() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let stop = Event {
            kind: SUBAGENT_STOP.into(),
            repo: Some("github.com/o/r".into()),
            branch: Some("main".into()),
            ..raw::test_event(&serde_json::json!({"agent_id": "a1"}).to_string())
        };
        let seq = store.append(&stop).unwrap();
        let facts = |k: &Connection| -> i64 {
            k.query_row(
                "SELECT COUNT(*) FROM manifest_facts WHERE seq = ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap()
        };
        let mut consumers: Vec<Box<dyn worker::Consumer>> =
            vec![Box::new(Manifest::new(home.path()))];
        let mut k = crate::knowledge::open(home.path()).unwrap();
        worker::drain(&store, &mut k, &mut consumers).unwrap();
        assert_eq!(facts(&k), 0);
        store.expire_for_test(&[seq]);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        worker::drain(&store, &mut k, &mut consumers).unwrap();
        assert_eq!(facts(&k), 0);
    }

    #[test]
    fn the_counts_that_matter_come_from_git_status() {
        let status = "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... x\n1 M. N... y\nu UU N... z\n? new\n";
        assert_eq!(
            porcelain(status),
            vec![
                "1 file(s) with merge conflicts",
                "2 uncommitted change(s)",
                "1 untracked file(s)",
                "2 commit(s) not pushed"
            ]
        );
        assert!(porcelain("# branch.ab +0 -3\n").is_empty());
    }

    #[test]
    fn a_merge_in_progress_and_changes_are_risky_state() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
        };
        if !git(&["init", "-q", "-b", "main"]).status.success() {
            return; // no git here
        }
        std::fs::write(dir.path().join("a"), "x").unwrap();
        std::fs::write(dir.path().join(".git/MERGE_HEAD"), "0\n").unwrap();
        let state = risky(dir.path(), "main");
        assert!(
            state.contains(&"a merge is in progress".to_owned()),
            "{state:?}"
        );
        assert!(
            state.contains(&"1 untracked file(s)".to_owned()),
            "{state:?}"
        );
        assert!(risky(dir.path(), "other").is_empty()); // the checkout is on another branch now
    }

    #[test]
    fn a_failure_is_read_from_each_agent_shape() {
        let b =
            |output: &str, failed: bool| serde_json::json!({"output": output, "failed": failed});
        assert!(failed(&b("x", true)));
        assert!(failed(&b("Exit code: 1\nWall time: 0 seconds", false)));
        assert!(!failed(&b("Exit code: 0\nWall time: 0 seconds", false)));
        assert!(failed(&b(r#"{"exit_code": 2}"#, false)));
        assert!(!failed(&b(r#"{"stdout": "ok"}"#, false)));
        let patch = serde_json::json!({"command": "*** Begin Patch\n*** Update File: src/a.rs\n*** Add File: b.md\n"});
        assert_eq!(paths(&patch, None), vec!["b.md", "src/a.rs"]);
        // Codex's newer header, and a command's own output that only looks like one.
        let newer =
            "Chunk ID: a1\nWall time: 1.2 seconds\nProcess exited with code 101\nOutput:\nx";
        assert!(failed(&b(newer, false)));
        assert!(!failed(&b(
            "Exit code: 0\nWall time: 0 seconds\nOutput:\nExit code: 1",
            false
        )));
        // Codex's transcript: the patch under `input`, the command under `cmd`.
        let custom = serde_json::json!({"input": "*** Begin Patch\n*** Update File: src/http.ts\n*** End Patch"});
        assert_eq!(paths(&custom, None), vec!["src/http.ts"]);
        assert_eq!(what_ran(r#"{"cmd": "rg fetchJson"}"#), "rg fetchJson");
        let windows = serde_json::json!({"file_path": "C:\\repo\\src\\a.rs"});
        assert_eq!(paths(&windows, Some("C:\\repo\\")), vec!["src/a.rs"]);
    }

    #[test]
    fn a_clipped_field_never_cuts_a_token() {
        assert_eq!(one_line("aaaa acme-123456 zzz", 10), "aaaa…");
        assert_eq!(one_line("aaaa acme-123456 zzz", 16), "aaaa acme-123456…");
        assert_eq!(one_line("acme-123456", 4), "…"); // longer than the clip: left out
        assert_eq!(one_line("a  b\nc", 10), "a b c");
        assert_eq!(one_line("日本語 の本文です", 5), "日本語…");
        // Text with no spaces is cut after a clause mark, never inside a clause.
        assert_eq!(
            one_line("鍵を分けた。次に山田太郎さんへ送る", 12),
            "鍵を分けた。…"
        );
        assert_eq!(one_line("鍵を分けた、次に送る", 8), "鍵を分けた、…");
        assert_eq!(one_line("山田太郎さんへ送る", 4), "…");
    }

    #[test]
    fn a_session_that_changed_branch_shows_the_last_one() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for (ts, branch, session) in [
            (60_000, "main", "s1"),
            (120_000, "feature", "s1"),
            (180_000, "main", "s2"),
        ] {
            store
                .append(&Event {
                    branch: Some(branch.into()),
                    ..ev(
                        "prompt",
                        session,
                        ts,
                        cwd.path(),
                        serde_json::json!({"prompt": "x"}),
                    )
                })
                .unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let start = shown_at(home.path(), &store, "s2", 240_000).unwrap();
        assert_eq!(
            lines_of(&start.text, "Other active sessions"),
            ["- claude on feature, 2 min ago"]
        );
    }

    #[test]
    fn a_failure_a_new_rule_masks_is_paired_with_the_success_stored_masked() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let run = |c: &str| serde_json::json!({"command": format!("deploy {c}")});
        // The failure stored before the rule, its retry after it (masked at capture).
        store
            .append(&tool(cwd.path(), 1, "Bash", run("acme-123456"), "no", true))
            .unwrap();
        store
            .append(&tool(
                cwd.path(),
                2,
                "Bash",
                run("***********"),
                "ok",
                false,
            ))
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(shown(home.path(), &store).unwrap().contains("failed with"));
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(home.path()).unwrap(); // the rescan masks the failure: same call now
        let shown = shown(home.path(), &store).unwrap();
        assert!(!shown.contains("failed with"), "{shown}");
    }

    #[test]
    fn a_tombstoned_directive_leaves_every_branch_of_its_repo() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        store
            .append(&ev(
                "prompt",
                "s1",
                60_000,
                cwd.path(),
                serde_json::json!({"prompt": "look at main"}),
            ))
            .unwrap();
        let on_feature = store
            .append(&Event {
                branch: Some("feature".into()),
                ..ev(
                    "prompt",
                    "s2",
                    120_000,
                    cwd.path(),
                    serde_json::json!({"prompt": "always run zqxlint first"}),
                )
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(manifest(home.path()).0.contains("zqxlint"));
        let device = store.device().to_owned();
        store
            .append_tombstone(Target::Record {
                device,
                seq: on_feature,
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let main = manifest(home.path()).0;
        assert!(!main.contains("zqxlint"), "{main}");
    }

    /// #273: a prompt another agent sent is no directive of the owner's, whatever it says; the
    /// owner's own line of the same words is one.
    #[test]
    fn a_prompt_another_agent_sent_is_no_owner_directive() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let said = "always run zqxlint first";
        let shown = format!("\"{said}\"");
        let prompt =
            |session: &str, ts: i64, body: Value| ev("prompt", session, ts, cwd.path(), body);
        let sent = serde_json::json!({"prompt": said, "agent_sent": true});
        store.append(&prompt("s1", 60_000, sent)).unwrap();
        worker::run_once(home.path()).unwrap();
        let m = manifest(home.path()).0;
        assert!(!m.contains(&shown), "{m}");
        let typed = serde_json::json!({"prompt": said});
        store.append(&prompt("s2", 120_000, typed)).unwrap();
        worker::run_once(home.path()).unwrap();
        let m = manifest(home.path()).0;
        assert!(
            m.contains("## Owner's directives") && m.contains(&shown),
            "{m}"
        );
    }

    #[test]
    fn an_interrupted_retry_fixes_nothing_and_a_retraction_takes_back_only_its_line() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let build = serde_json::json!({"command": "cargo build"}).to_string();
        let mut store = raw::open(home.path()).unwrap();
        for e in [
            ev(
                "prompt",
                "s1",
                60_000,
                cwd.path(),
                serde_json::json!({"prompt": "from now on run clippy\nalways write tests first"}),
            ),
            tool(
                cwd.path(),
                120_000,
                "Bash",
                serde_json::json!({"command": "cargo build"}),
                "E0308",
                true,
            ),
            ev(
                "tool",
                "s1",
                180_000,
                cwd.path(),
                serde_json::json!({"tool": "Bash", "input": build, "output": "", "failed": false,
                    "interrupted": true}),
            ),
        ] {
            store.append(&e).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let before = manifest(home.path()).0;
        assert!(
            before.contains("cargo build") && before.contains("\"from now on run clippy\""),
            "{before}"
        );
        // Taken back on another branch: the directives are the repo's, so main is built again.
        store
            .append(&Event {
                branch: Some("feature".into()),
                ..ev(
                    "prompt",
                    "s2",
                    240_000,
                    cwd.path(),
                    serde_json::json!({"prompt": "cancel that clippy rule"}),
                )
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let after = manifest(home.path()).0;
        assert!(
            after.contains("\"always write tests first\"")
                && !after.contains("\"from now on run clippy\""),
            "{after}"
        );
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            std::fs::remove_file(home.path().join(f)).ok();
        }
        worker::run_once(home.path()).unwrap();
        assert_eq!(manifest(home.path()).0, after); // the same records, the same bytes
    }

    /// Milestone 4 D6: imported records are history, not the checkout's state: the manifest's
    /// last prompt, its sessions and its count of records not yet curated come from live records
    /// only.
    #[test]
    fn the_manifest_ignores_imported_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        let prompt = serde_json::json!({"prompt": "An imported prompt."});
        let imported = Event {
            source: "oboete-v1".into(),
            ..ev("prompt", "v1", 100 * 60_000, cwd.path(), prompt)
        };
        store.append(&imported).unwrap();
        worker::run_once(home.path()).unwrap();
        assert_eq!(shown(home.path(), &store).unwrap(), before);
    }

    /// Milestone 4 D6, MUST-M9: the backlog line counts live records not yet curated, never the
    /// imported ones that wait for `recurate --source`.
    #[test]
    fn the_backlog_line_does_not_count_parked_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        curates(home.path());
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let backlog = |store: &Raw| lines_of(&shown(home.path(), store).unwrap(), "Backlog");
        let before = backlog(&store);
        assert!(before[0].starts_with("9 record(s) / "), "{before:?}");
        for i in 0..3 {
            let body = serde_json::json!({"prompt": format!("Imported {i}.")});
            let e = Event {
                source: "transcript".into(),
                ..ev("prompt", "t", (200 + i) * 60_000, cwd.path(), body)
            };
            store.append(&e).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        assert_eq!(backlog(&store), before);
    }

    const DAY: i64 = 86_400_000;

    fn said(cwd: &Path, ts: i64, text: &str) -> Event {
        ev("prompt", "s3", ts, cwd, serde_json::json!({"prompt": text}))
    }

    /// `op` as `speaker` said it.
    fn by(mut op: (crate::raw::OpKind, Value), speaker: &str) -> (crate::raw::OpKind, Value) {
        op.1["speaker"] = speaker.into();
        op
    }

    /// The lines under `text`'s heading `title`.
    /// docs/session-start.md: "Last exchange" is the owner's last typed prompt and the reply that
    /// ended its turn. A later prompt another program sent, in its session or another, Codex's
    /// review template among them, and its reply are not shown, nor is a reply before the prompt;
    /// an unanswered prompt shows alone, and a template's words are no directive of the owner's.
    #[test]
    fn the_last_exchange_is_the_owners_last_typed_prompt_and_its_turns_reply() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let min = 60_000;
        let template = "Review the code changes introduced by commit 1a2b3c4d (\"Always run the \
                        linter first\"). Provide prioritized, actionable findings.";
        let prompt = |s: &str, ts: i64, body: Value| ev("prompt", s, ts, cwd.path(), body);
        let reply = |s: &str, ts: i64, text: &str| {
            ev(
                "reply",
                s,
                ts,
                cwd.path(),
                serde_json::json!({ "assistant": text }),
            )
        };
        let exchange = |store: &Raw| {
            worker::run_once(home.path()).unwrap();
            lines_of(&shown(home.path(), store).unwrap(), "Last exchange")
        };
        // Only programs' prompts: no exchange, and the template's "always" is no directive.
        for e in [
            prompt(
                "s1",
                min,
                serde_json::json!({"prompt": "Check it.", "agent_sent": true}),
            ),
            reply("s1", 2 * min, "Checked by request."),
            Event {
                agent: "codex".into(),
                ..prompt("s2", 3 * min, serde_json::json!({ "prompt": template }))
            },
            reply("s2", 4 * min, "A review's findings."),
        ] {
            store.append(&e).unwrap();
        }
        assert!(exchange(&store).is_empty());
        let text = shown(home.path(), &store).unwrap();
        assert!(!text.contains("linter"), "{text}");
        for e in [
            reply("s3", 5 * min, "An answer before the owner's prompt."),
            prompt(
                "s3",
                6 * min,
                serde_json::json!({"prompt": "Fix the parser."}),
            ),
            reply("s3", 7 * min, "The parser is fixed."),
            reply("s3", 8 * min, "A second reply in the same turn."),
            prompt(
                "s3",
                9 * min,
                serde_json::json!({"prompt": "Check it.", "agent_sent": true}),
            ),
            reply("s3", 10 * min, "Checked by request."),
            Event {
                agent: "codex".into(),
                ..prompt("s4", 11 * min, serde_json::json!({ "prompt": template }))
            },
            reply("s4", 12 * min, "A review's findings."),
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(
            exchange(&store),
            ["- prompt: Fix the parser.", "- reply: The parser is fixed."]
        );
        // Unanswered, then a program's prompt in its session and that prompt's reply: alone.
        for e in [
            prompt(
                "s5",
                13 * min,
                serde_json::json!({"prompt": "Now the lexer."}),
            ),
            prompt(
                "s5",
                14 * min,
                serde_json::json!({"prompt": "Lint it.", "agent_sent": true}),
            ),
            reply("s5", 15 * min, "Linted by request."),
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(exchange(&store), ["- prompt: Now the lexer."]);
        // A turn whose session moved to another branch: its reply there, before the program's
        // prompt on that branch, not the later reply here (Codex's review of #854).
        let on = |branch: &str, e: Event| Event {
            branch: Some(branch.into()),
            ..e
        };
        for e in [
            prompt(
                "s6",
                16 * min,
                serde_json::json!({"prompt": "Split the module."}),
            ),
            on("feature", reply("s6", 17 * min, "Split on a branch.")),
            on(
                "feature",
                prompt(
                    "s6",
                    18 * min,
                    serde_json::json!({"prompt": "Check.", "agent_sent": true}),
                ),
            ),
            reply("s6", 19 * min, "Checked by request."),
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(
            exchange(&store),
            ["- prompt: Split the module.", "- reply: Split on a branch."]
        );
        // A session that moved to another repository, or another agent's session of the same id:
        // no reply from there (Codex and cubic on #854).
        let elsewhere = |e: Event| Event {
            repo: Some("other".into()),
            ..e
        };
        for e in [
            prompt(
                "s7",
                20 * min,
                serde_json::json!({"prompt": "Move the tests."}),
            ),
            elsewhere(reply("s7", 21 * min, "Moved in the other repository.")),
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(exchange(&store), ["- prompt: Move the tests."]);
        // The turn ends at the session's next prompt in any repository: a reply recorded here after
        // the session's prompt in another repository is not this prompt's (cubic on #854, #856).
        for e in [
            prompt(
                "s9",
                20 * min + 30_000,
                serde_json::json!({"prompt": "Rename the crate."}),
            ),
            elsewhere(prompt(
                "s9",
                20 * min + 40_000,
                serde_json::json!({"prompt": "Look at the other repository."}),
            )),
            reply("s9", 20 * min + 50_000, "A reply after the other prompt."),
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(exchange(&store), ["- prompt: Rename the crate."]);
        for e in [
            prompt(
                "s8",
                22 * min,
                serde_json::json!({"prompt": "Name the types."}),
            ),
            Event {
                agent: "grok".into(),
                ..reply("s8", 23 * min, "Another agent's answer.")
            },
        ] {
            store.append(&e).unwrap();
        }
        assert_eq!(exchange(&store), ["- prompt: Name the types."]);
    }

    /// The turn's reply, and the session's next prompt in any repository that ends its turn, are
    /// each found by a seek, not a scan of the device's facts (cubic on #854, #856).
    #[test]
    fn the_turns_reply_is_a_seek() {
        let k = rusqlite::Connection::open_in_memory().unwrap();
        schema(&k).unwrap();
        for (query, index) in [
            (
                "SELECT MIN(seq) FROM manifest_facts
                 WHERE device = 'd' AND repo = 'r' AND session = 's' AND fact = 'reply'
                   AND seq > 1",
                "manifest_facts_turn",
            ),
            (
                "SELECT MIN(seq) FROM manifest_facts
                 WHERE device = 'd' AND session = 's' AND fact = 'prompt' AND seq > 1",
                "manifest_facts_session",
            ),
        ] {
            let plan: Vec<String> = k
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(plan.iter().any(|l| l.contains(index)), "{plan:?}");
        }
    }

    /// Decision 76: a decision or an open item whose unit's newest claim is older than 14 days at
    /// the read has no body; it is an index line, where its rank put it. A lesson keeps its body at
    /// any age, a pair whose later claim is fresh keeps both bodies, a pair whose later claim is
    /// stale is two index lines, and a claim of 14 days exactly is not older.
    #[test]
    fn a_decision_or_open_item_older_than_14_days_is_an_index_line() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let now = 100 * DAY;
        let mut ops = Vec::new();
        let mut claim = |ago: i64, text: &str, kind: &str, supersedes: Vec<String>| {
            let e = said(cwd.path(), now - ago * DAY, text);
            let (op, uid) = claimed(&mut store, e, kind, "decided", supersedes);
            ops.push(op);
            uid
        };
        claim(15, "Ship the beta on Friday.", "decision", vec![]);
        claim(14, "Keep the parser small.", "decision", vec![]);
        claim(30, "Logs rotate daily.", "lesson", vec![]);
        claim(15, "Fix the flaky CI test.", "open item", vec![]);
        let tabs = claim(40, "Use tabs.", "decision", vec![]);
        claim(1, "Use spaces instead.", "decision", vec![tabs]);
        let mondays = claim(50, "Deploy on Mondays.", "decision", vec![]);
        claim(20, "Deploy on Tuesdays.", "decision", vec![mondays]);
        store.append_ops(&ops).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown_at(home.path(), &store, "none", now).unwrap().text;
        let bodies = lines_of(&text, "Decisions and open items");
        let index = lines_of(&text, "More from memory");
        let body = |t: &str| bodies.iter().any(|l| l.contains(t));
        let line = |t: &str| index.iter().any(|l| l.contains(t));
        for t in [
            "Keep the parser small.",
            "Logs rotate daily.",
            "Use tabs.",
            "Use spaces instead.",
        ] {
            assert!(body(t) && !line(t), "{t}: {text}");
        }
        for t in [
            "Ship the beta on Friday.",
            "Fix the flaky CI test.",
            "Deploy on Mondays.",
            "Deploy on Tuesdays.",
        ] {
            assert!(line(t) && !body(t), "{t}: {text}");
        }
    }

    /// Decision 75: the cards taken back are the most that fit, whatever their sizes: a block
    /// without its savings line can be shorter with more cards (Codex's review of #854).
    #[test]
    fn the_cards_taken_back_are_the_most_that_fit_whatever_their_sizes() {
        let size = |m: usize| [0, 0, 0, 0, 462, 485, 507, 470, 494][m];
        let room = |most: usize| move |m: usize| (size(m) <= most).then_some(m);
        assert_eq!(fill_back(4, 8, room(470)), Some((7, 7)));
        assert_eq!(fill_back(4, 8, room(490)), Some((7, 7)));
        assert_eq!(fill_back(4, 8, room(486)), Some((7, 7)));
        assert_eq!(fill_back(4, 8, room(469)), None);
        assert_eq!(fill_back(4, 5, |_| Some(())), None);
    }

    /// Decision 75: the index lists as many claims as it is given; with none, the bodies and the
    /// line naming the tools stay.
    #[test]
    fn the_index_lists_as_many_claims_as_it_is_given() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let mut ops = Vec::new();
        for i in 0..BODIES as i64 + 5 {
            let e = said(cwd.path(), DAY + i, &format!("Rule {i} holds."));
            ops.push(claimed(&mut store, e, "decision", "decided", vec![]).0);
        }
        store.append_ops(&ops).unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let read = |index: usize| {
            let s = text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                usize::MAX,
                index,
                NOW,
            );
            let s = s.unwrap().unwrap().text;
            let lines = |t: &str| lines_of(&s, t).into_iter().filter(|l| l.starts_with("- "));
            (
                lines("Decisions and open items").count(),
                lines("More from memory").count(),
                s,
            )
        };
        assert_eq!(read(INDEX).1, 5);
        assert_eq!(read(3).1, 3);
        let (bodies, lines, s) = read(0);
        assert_eq!((bodies, lines), (BODIES, 0), "{s}");
        assert!(s.contains(TOOLS), "{s}");
    }

    fn lines_of(text: &str, title: &str) -> Vec<String> {
        text.split(&format!("## {title}\n"))
            .nth(1)
            .map(|s| {
                s.split("\n## ")
                    .next()
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A home whose checkout (r, main) has `session`'s records and the claims `ops` makes, applied,
    /// then a last prompt `prompt` when given (the words SessionStart ranks by). What SessionStart
    /// shows.
    fn start(
        claims: impl FnOnce(&mut Raw, &Path) -> Vec<(crate::raw::OpKind, Value)>,
        prompt: Option<&str>,
    ) -> (tempfile::TempDir, tempfile::TempDir, String) {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let ops = claims(&mut store, cwd.path());
        store.append_ops(&ops).unwrap();
        if let Some(p) = prompt {
            store.append(&said(cwd.path(), 100 * DAY, p)).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        (home, cwd, text)
    }

    /// A curated window op over records `from` to `to` with a card for each of `titles`.
    fn cards_op(from: i64, to: i64, titles: &[&str]) -> (crate::raw::OpKind, Value) {
        let cards: Vec<Value> = titles
            .iter()
            .map(|t| {
                serde_json::json!({"type": "bugfix", "title": t, "subtitle": "", "narrative": "",
                    "facts": [], "concepts": [], "files_read": [], "files_modified": []})
            })
            .collect();
        let op = serde_json::json!({"outcome": "curated", "summary": "", "from_seq": from,
            "from_offset": null, "to_seq": to, "to_offset": null, "elided": [],
            "observations": cards});
        (crate::raw::OpKind::Window, op)
    }

    /// Parity row X1: a card of a window that is a subagent's work alone is not shown at session
    /// start, as claude-mem's `CONTEXT_MAIN_AGENT_ONLY` keeps its rows out; the main agent's are.
    #[test]
    fn session_start_leaves_out_a_subagents_work() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let sub = |output: &str| {
                    let body = serde_json::json!({"tool": "Read", "input": "{}", "output": output,
                        "failed": false, "agent_id": "a1"});
                    ev("tool", "s1", DAY, cwd, body)
                };
                let from = store.append(&sub("one")).unwrap();
                let to = store.append(&sub("two")).unwrap();
                vec![
                    cards_op(1, 7, &["Ours"]),
                    cards_op(from, to, &["The subagent's"]),
                ]
            },
            None,
        );
        // The packet holds uids: not printed (CodeQL on #561).
        assert!(text.contains(" ● Ours\n"));
        assert!(!text.contains("The subagent's"));
    }

    /// docs/cards.md S1, S4: the repository's cards, as claude-mem's block, after the owner's
    /// decisions and before the checkout's state lines; another repository's are not shown.
    #[test]
    fn session_start_shows_the_repositorys_cards_after_the_decisions() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (decision, _) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let elsewhere = Event {
                    repo: Some("other".into()),
                    ..ev("tool", "s9", DAY, cwd, serde_json::json!({}))
                };
                let other = store.append(&elsewhere).unwrap();
                vec![
                    decision,
                    cards_op(1, 7, &["Ours"]),
                    cards_op(other, other, &["Theirs"]),
                ]
            },
            None,
        );
        let block = text.find("# [r] recent context, ").expect(&text);
        assert!(
            text.find("## Decisions and open items").unwrap() < block,
            "{text}"
        );
        assert!(
            block < text.find("## Owner's directives").unwrap(),
            "{text}"
        );
        assert!(text.contains(" ● Ours\n"), "{text}");
        assert!(!text.contains("Theirs"), "{text}");
    }

    /// Parity row 13: session start counts what the curator spent on the windows of the cards it
    /// shows, from this device's ledger, which it reads and never makes.
    #[test]
    fn session_start_counts_what_the_cards_windows_cost() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (kind, mut op) = cards_op(1, 7, &["Ours", "And ours"]);
        op["provider"] = "p".into();
        store.append_ops(&[(kind, op)]).unwrap();
        worker::run_once(home.path()).unwrap();
        // "Ours" and "[]" -> 2 tokens, "And ours" and "[]" -> 3. The packet holds uids: no
        // message prints it (CodeQL).
        let text = shown(home.path(), &store).unwrap();
        assert!(text.contains("\nStats: 2 obs (5t read) | 0t work\n"));
        assert!(!home.path().join("providers.db").exists());
        let db = crate::providers_db::open(home.path()).unwrap();
        db.execute(
            "INSERT INTO provider_calls(ts, provider, role, span, outcome, ms, prompt_tokens,
               completion_tokens) VALUES(0, 'p', 'curator', '1-7', 'ok', 0, 9000, 1000)",
            [],
        )
        .unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(text.contains("\nStats: 2 obs (5t read) | 10,000t work | 100% savings\n"));
        assert!(text.contains("\nAccess 10k tokens of past work via get(ID) or search(query).\n"));
    }

    /// Codex on slice 3: a card's title is gated as its row shows it, on one line, so a rule
    /// anchored to the whole flattened title hides its value behind the row's ID and time.
    #[test]
    fn a_cards_title_is_gated_as_its_row_shows_it() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"otp\", regex = '^otp= ([0-9]{6})$', \
             secret_group = 1 }]\n",
        )
        .unwrap();
        let mut store = raw::open(home.path()).unwrap();
        store
            .append_ops(&[cards_op(1, 7, &["otp=\n654321"])])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(text.contains("recent context"), "{text}");
        assert!(!text.contains("654321"), "{text}");
    }

    /// Codex on slice 3: the gate can make the rest of the packet longer than it was read (a
    /// short value becomes `[REDACTED]`), and the cards are fitted to the packet as it leaves,
    /// so they still never cut the rest.
    #[test]
    fn the_cards_leave_room_for_what_the_gate_adds() {
        let values: Vec<String> = (0..20).map(|i| format!("otp={}", i % 10)).collect();
        let (home, _c, _) = start(
            |store, cwd| {
                let (decision, _) = claimed(
                    store,
                    said(cwd, DAY, &format!("Keep {}.", values.join(" "))),
                    "decision",
                    "decided",
                    vec![],
                );
                let titles: Vec<String> = (0..8).map(|i| format!("Card number {i}")).collect();
                let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
                vec![decision, cards_op(1, 7, &titles)]
            },
            None,
        );
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"otp\", regex = 'otp=([0-9])', \
             secret_group = 1 }]\n",
        )
        .unwrap();
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let at = |cap: usize| {
            text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap()
            .text
        };
        let whole = at(usize::MAX);
        assert!(
            whole.contains("[REDACTED]") && !whole.contains("otp=3"),
            "{whole}"
        );
        let start = whole.find("# [r] recent context, ").expect(&whole);
        let end = start + whole[start..].find("\n## ").unwrap() + 1;
        let fitted = at(whole.chars().count() - 1);
        assert!(fitted.contains("recent context"), "{fitted}");
        assert!(fitted.ends_with(&whole[end..]), "{fitted}");
    }

    /// Codex on #370: the gate can make the rest of the packet shorter than it was read (a long
    /// value becomes `[REDACTED]`), and the cards take the room it frees.
    #[test]
    fn the_cards_take_the_room_the_gate_frees() {
        let value = "7".repeat(60);
        let (home, _c, _) = start(
            |store, cwd| {
                let (decision, _) = claimed(
                    store,
                    said(cwd, DAY, &format!("Keep otp={value} here.")),
                    "decision",
                    "decided",
                    vec![],
                );
                let titles: Vec<String> = (0..8).map(|i| format!("Card number {i}")).collect();
                let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
                vec![decision, cards_op(1, 7, &titles)]
            },
            None,
        );
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"otp\", regex = 'otp=([0-9]+)', \
             secret_group = 1 }]\n",
        )
        .unwrap();
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let shows = |cap: usize| {
            let s = text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap();
            (s.text, s.cards)
        };
        let (whole, cards) = shows(usize::MAX);
        assert!(
            whole.contains("[REDACTED]") && !whole.contains(&value),
            "{whole}"
        );
        assert_eq!(cards, 8);
        // The packet's text as the fence holds it, without the fence's own.
        let units = manifest::fenced(whole.trim()).encode_utf16().count()
            - manifest::fenced("").encode_utf16().count();
        assert_eq!(shows(units), (whole.clone(), 8));
    }

    /// Codex on #370: a rule that masks a card's row only as the block composes it (here it needs
    /// the row's type icon) frees its room too: each count is judged on the gated packet.
    #[test]
    fn a_rule_that_masks_a_composed_card_row_frees_its_room() {
        let long = "Z".repeat(60);
        let (home, _c, _) = start(
            |store, cwd| {
                let (decision, _) = claimed(
                    store,
                    said(cwd, DAY, "Keep the parser small."),
                    "decision",
                    "decided",
                    vec![],
                );
                let mut titles: Vec<String> = (0..7).map(|i| format!("Card number {i}")).collect();
                titles.push(long.clone());
                let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
                vec![decision, cards_op(1, 7, &titles)]
            },
            None,
        );
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"row\", regex = '● (Z{60})', \
             secret_group = 1 }]\n",
        )
        .unwrap();
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let shows = |cap: usize| {
            let s = text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap();
            (s.text, s.cards)
        };
        let (whole, cards) = shows(usize::MAX);
        assert!(
            whole.contains("[REDACTED]") && !whole.contains(&long),
            "{whole}"
        );
        assert_eq!(cards, 8);
        let units = manifest::fenced(whole.trim()).encode_utf16().count()
            - manifest::fenced("").encode_utf16().count();
        assert_eq!(shows(units), (whole.clone(), 8));
    }

    /// Codex on slice 3: the cards are fitted in UTF-16 units inside the fence, the measure the
    /// agents cut by (Cursor), with each closing tag a title holds escaped as the fence escapes it.
    #[test]
    fn the_cards_are_fitted_inside_the_fence_in_utf16_units() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let titles: Vec<String> = (0..8)
            .map(|i| format!("{i} {} </oboete-memory>", "🚀".repeat(50)))
            .collect();
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        store.append_ops(&[cards_op(1, 7, &titles)]).unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let at = |cap: usize| {
            text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap()
            .text
        };
        // The packet's text as the fence holds it, in UTF-16 units, without the fence's own.
        let units = |t: &str| {
            manifest::fenced(t.trim()).encode_utf16().count()
                - manifest::fenced("").encode_utf16().count()
        };
        let whole = at(usize::MAX);
        let start = whole.find("# [r] recent context, ").expect(&whole);
        let end = start + whole[start..].find("\n## ").unwrap() + 1;
        let cap = units(&whole) - 1;
        assert!(whole.chars().count() <= cap);
        let fitted = at(cap);
        assert!(units(&fitted) <= cap, "{} > {cap}", units(&fitted));
        assert!(fitted.contains("recent context"), "{fitted}");
        assert!(fitted.ends_with(&whole[end..]), "{fitted}");
    }

    /// S5: the stored manifest keeps its own 6,000 characters under the packet's 9,000, so the
    /// state lines never take all of the cards' room.
    #[test]
    fn the_stored_manifest_keeps_its_own_size_under_the_packets() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let todos: Vec<Value> = (0..TODOS)
            .map(|i| {
                serde_json::json!({"content": format!("{i} {}", "a long step ".repeat(40)),
                "status": "pending"})
            })
            .collect();
        let seq = store
            .append(&tool(
                cwd.path(),
                60_000,
                "TodoWrite",
                serde_json::json!({ "todos": todos }),
                "",
                false,
            ))
            .unwrap();
        store
            .append_ops(&[cards_op(seq, seq, &["The todo list was written"])])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let (stored, _) = manifest(home.path());
        assert!(
            stored.chars().count() <= MANIFEST,
            "{}",
            stored.chars().count()
        );
        const { assert!(MANIFEST < CAP) };
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let packet = text(
            home.path(),
            &store,
            "r",
            "main",
            "none",
            &rules,
            CAP,
            INDEX,
            NOW,
        )
        .unwrap()
        .unwrap()
        .text;
        assert!(
            packet.contains(" ● The todo list was written\n"),
            "{packet}"
        );
    }

    /// S5: the cards take only the room the rest of the packet leaves, halved until they fit and
    /// then as many back as fit (decision 75), and the rest is not cut for them.
    #[test]
    fn the_cards_take_only_the_room_the_packet_leaves() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let titles: Vec<String> = (0..8).map(|i| format!("Card number {i}")).collect();
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        store.append_ops(&[cards_op(1, 7, &titles)]).unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        // The packet, and how many cards it shows (the terminal line counts them).
        let shows = |cap: usize| {
            let s = text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap();
            (s.text, s.cards)
        };
        let at = |cap: usize| shows(cap).0;
        let whole = at(usize::MAX);
        assert_eq!(shows(usize::MAX).1, 8);
        let start = whole.find("# [r] recent context, ").expect(&whole);
        let end = start + whole[start..].find("\n## ").unwrap() + 1;
        let block = &whole[start..end];
        assert!(block.contains(".7 ") && block.contains(".0 "), "{block}");
        let rest = whole.chars().count() - block.chars().count();
        let fitted = at(rest + block.chars().count() - 1);
        // One character short of the 8: halved to 4, then 3 of the 4 left out taken back.
        assert!(
            fitted.contains(".6 ") && !fitted.contains(".7 "),
            "{fitted}"
        );
        assert_eq!(shows(rest + block.chars().count() - 1).1, 7);
        assert!(fitted.ends_with(&whole[end..]), "{fitted}");
        assert!(fitted.chars().count() < rest + block.chars().count());
        // No room for one card: no block, and the rest as it was.
        let none = at(rest);
        assert!(!none.contains("recent context"), "{none}");
        assert_eq!(shows(rest).1, 0);
        assert_eq!(none, format!("{}{}", &whole[..start], &whole[end..]));
    }

    /// A turn op of session s1 in repository r over records `from` to `through`, at `ts`, that
    /// asked `request`, with `fields` (docs/summaries.md T5).
    fn turn_op(
        (from, through): (i64, i64),
        ts: i64,
        request: &str,
        fields: &[(&str, &str)],
    ) -> (crate::raw::OpKind, Value) {
        let mut all = serde_json::Map::new();
        all.insert("request".into(), request.into());
        for (f, text) in fields {
            all.insert((*f).into(), (*text).into());
        }
        let op = serde_json::json!({"agent": "claude", "session": "s1", "repo": "r", "ts": ts,
            "from": from, "through": through, "read": [], "goals": [], "removed": [],
            "fields": all, "skipped": false});
        (crate::raw::OpKind::Turn, op)
    }

    /// docs/summaries.md S7, S8: session start shows the repository's newest summaries among its
    /// cards, and the newest one's fields after them.
    #[test]
    fn session_start_shows_the_summaries_among_the_cards() {
        let (_h, _c, text) = start(
            |_, _| {
                vec![
                    cards_op(1, 7, &["Ours"]),
                    turn_op(
                        (1, 7),
                        10 * DAY,
                        "Fix the parser",
                        &[("completed", "It reads to the end.")],
                    ),
                ]
            },
            None,
        );
        let block = text.find("# [r] recent context, ").expect(&text);
        assert!(text[block..].contains(" ● Ours\n"), "{text}");
        assert!(
            text[block..].contains(" Fix the parser (Jan 11, "),
            "{text}"
        );
        assert!(
            text[block..].contains("**Completed**: It reads to the end.\n"),
            "{text}"
        );
    }

    /// S9: claude-mem's order when the block does not fit: the newest summary's fields go first,
    /// then the summary rows halve, the newest kept, then the cards.
    #[test]
    fn the_fields_go_first_then_the_summary_rows_then_the_cards() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        store
            .append_ops(&[
                cards_op(1, 7, &["Card one", "Card two"]),
                turn_op((1, 3), 9 * DAY, "Older turn", &[]),
                turn_op(
                    (4, 7),
                    10 * DAY,
                    "Newer turn",
                    &[("completed", "It reads to the end.")],
                ),
            ])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let at = |cap: usize| {
            text(
                home.path(),
                &store,
                "r",
                "main",
                "none",
                &rules,
                cap,
                INDEX,
                NOW,
            )
            .unwrap()
            .unwrap()
            .text
        };
        let less = |shown: &str| at(shown.chars().count() - 1);
        let whole = at(usize::MAX);
        let has = |s: &str, parts: &[&str]| parts.iter().all(|p| s.contains(p));
        let all = ["Older turn", "Newer turn", "Card one", "Card two"];
        assert!(
            has(&whole, &all) && whole.contains("**Completed**"),
            "{whole}"
        );
        let no_fields = less(&whole);
        assert!(
            has(&no_fields, &all) && !no_fields.contains("**Completed**"),
            "{no_fields}"
        );
        let one_row = less(&no_fields);
        assert!(
            has(&one_row, &["Newer turn", "Card one", "Card two"]) && !one_row.contains("Older"),
            "{one_row}"
        );
        let no_rows = less(&one_row);
        assert!(
            has(&no_rows, &["Card one", "Card two"]) && !no_rows.contains(" turn ("),
            "{no_rows}"
        );
        let one_card = less(&no_rows);
        assert!(
            one_card.contains("Card one") && !one_card.contains("Card two"),
            "{one_card}"
        );
    }

    /// Codex on #372: the summary rows the packet hands over are counted with its cards, the
    /// recent work the terminal line names.
    #[test]
    fn the_summary_rows_are_counted_with_the_cards() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        store
            .append_ops(&[
                turn_op((1, 3), 9 * DAY, "Older turn", &[]),
                turn_op((4, 7), 10 * DAY, "Newer turn", &[]),
            ])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let s = text(
            home.path(),
            &store,
            "r",
            "main",
            "none",
            &rules,
            usize::MAX,
            INDEX,
            NOW,
        )
        .unwrap()
        .unwrap();
        assert!(s.text.contains("Older turn"), "{}", s.text);
        assert_eq!(s.cards, 2);
    }

    /// Spec 3.4, owner decision 31 (#295 row 1): an earlier decision that a curator link from a
    /// later decision of the owner ended is delivered after it, each with its date.
    #[test]
    fn an_earlier_decision_ended_by_a_later_owner_decision_is_delivered_after_it_with_both_dates() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, spaces]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            [
                "- 1970-01-04 decision: Use spaces instead.",
                "- 1970-01-02 decision, superseded by the 1970-01-04 decision above: Use tabs.",
            ]
        );
    }

    /// D1: only a curator link from a later claim the owner backs, decided or done, leaves the
    /// earlier decision delivered: the user's own words, or a proposal the user accepted (Codex on
    /// #303). One from a proposal still open, a retracted claim, an earlier claim or a claim the
    /// owner does not back still ends it.
    #[test]
    fn a_link_from_a_proposal_a_retracted_claim_or_an_earlier_claim_delivers_nothing_new() {
        let cases = [
            ("proposed", "assistant proposal", 1),
            ("retracted", "user", 1),
            ("decided", "user", -1),
            ("decided", "assistant", 1),
            ("decided", "assistant proposal", 1),
        ];
        let (_h, _c, text) = start(
            |store, cwd| {
                let mut ops = Vec::new();
                for (i, (status, speaker, after)) in (0_i64..).zip(cases) {
                    let day = 10 * (i + 1);
                    let (old, uid) = claimed(
                        store,
                        said(cwd, day * DAY, &format!("Old rule {i}.")),
                        "decision",
                        "decided",
                        vec![],
                    );
                    let (new, _) = claimed(
                        store,
                        said(cwd, (day + after) * DAY, &format!("New rule {i}.")),
                        "decision",
                        status,
                        vec![uid],
                    );
                    ops.extend([old, by(new, speaker)]);
                }
                ops
            },
            None,
        );
        let shown = lines_of(&text, "Decisions and open items").join("\n");
        for ended in 0..4 {
            assert!(!shown.contains(&format!("Old rule {ended}.")), "{shown}");
        }
        assert!(
            shown.contains("New rule 2.") && shown.contains("New rule 3."),
            "{shown}"
        );
        // The accepted proposal is the owner's later decision: the earlier one follows it.
        assert!(
            shown.contains(
                "- 1970-02-21 decision: New rule 4.\n\
                 - 1970-02-20 decision, superseded by the 1970-02-21 decision above: Old rule 4."
            ),
            "{shown}"
        );
    }

    /// #261, #302 item 3: a restatement quotes the same words of the same event, so its link is
    /// not from a later claim, and it ends the claim it restates as before.
    #[test]
    fn a_restatement_ends_a_claim_as_before() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (old, uid) = claimed(
                    store,
                    said(cwd, DAY, "Keep the cache small."),
                    "decision",
                    "decided",
                    vec![],
                );
                let mut again = old.clone();
                again.1["id"] = "r1".into();
                again.1["kind"] = "preference".into();
                again.1["supersedes"] = serde_json::json!([uid]);
                vec![old, again]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 preference: Keep the cache small."]
        );
    }

    /// Review Focus 1 (#295 row 2): a pair takes two places. With one place left, the earlier
    /// decision and the later one that ended it go to the index together, the later first, and the
    /// next claim that fits takes the place. The earlier decision never stands alone.
    #[test]
    fn a_picked_earlier_claim_brings_its_later_claim_and_the_pair_takes_two_places() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (early, old) = claimed(
                    store,
                    said(cwd, DAY, "Zebra builds use the old runner."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (later, _) = claimed(
                    store,
                    said(cwd, 30 * DAY, "Use the new runner."),
                    "decision",
                    "decided",
                    vec![old],
                );
                let (short, _) = claimed(
                    store,
                    said(cwd, 5 * DAY, "Keep logs short."),
                    "decision",
                    "decided",
                    vec![],
                );
                let mut ops = vec![early, later, short];
                for day in 11..20 {
                    let text = format!("Zebra rule {day}.");
                    ops.push(
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0,
                    );
                }
                ops
            },
            // "zebra" is the only word it shares with them.
            Some("What about zebra?"),
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10, "{bodies:?}");
        assert!(
            bodies.iter().take(9).all(|l| l.contains("Zebra rule")),
            "{bodies:?}"
        );
        assert_eq!(bodies[9], "- 1970-01-06 decision: Keep logs short.");
        let more = lines_of(&text, "More from memory");
        assert_eq!(more.len(), 3, "{more:?}");
        assert!(
            more[1].ends_with("1970-01-31 decision: Use the new runner."),
            "{more:?}"
        );
        let later_id = more[1][2..14].to_owned();
        assert!(
            more[2].ends_with(&format!(
                "1970-01-02 decision, superseded by {later_id}: Zebra builds use the old runner."
            )),
            "{more:?}"
        );
    }

    /// Spec 3.4's switch (D2): once curator links end decisions, the earlier decision leaves
    /// delivery as it does from the tips.
    #[test]
    fn with_links_ending_decisions_only_the_later_claim_is_delivered() {
        let (home, _c, _text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, spaces]
            },
            None,
        );
        let k = rusqlite::Connection::open(home.path().join("knowledge.db")).unwrap();
        let bodies = |links_end: bool| -> Vec<String> {
            k.prepare(&crate::claims::delivered("a.repo = ?1", links_end))
                .unwrap()
                .query_map(["r"], |r| r.get(5))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let mut delivered = bodies(false);
        delivered.sort();
        assert_eq!(delivered, ["Use spaces instead.", "Use tabs."]);
        assert_eq!(bodies(true), ["Use spaces instead."]);
    }

    /// Row 54-6: a decision and its retraction in different chunks: SessionStart never goes back
    /// to before the retraction.
    #[test]
    fn a_retraction_in_another_chunk_never_brings_the_decision_back() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (ship, uid) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Ship the beta on Friday."),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[ship]).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 decision: Ship the beta on Friday."]
        );
        let (retract, _) = claimed(
            &mut store,
            said(
                cwd.path(),
                2 * DAY,
                "Do not ship the beta on Friday after all.",
            ),
            "decision",
            "retracted",
            vec![uid],
        );
        store.append_ops(&[retract]).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(!text.contains("## Decisions and open items"), "{text}");
        assert!(!text.contains("## More from memory"), "{text}");
    }

    /// #743: the owner's rules of the repository leave the decisions their pool: with 200 of them,
    /// a decision still has its body.
    #[test]
    fn the_owners_rules_leave_the_decisions_their_pool() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let mut ops = Vec::new();
        for i in 0..POOL as i64 {
            let c = crate::capture::directive(&format!("Rule {i}."), DAY + i, &settings, Some("r"));
            let seq = store.append(&c.event).unwrap();
            let quote = crate::curate::long_text(&c.event).unwrap();
            let evidence = crate::claims::Evidence {
                device: store.device().to_owned(),
                seq,
                offset: 0,
                length: quote.len() as i64,
                sentence: 0,
                quote,
                claim_at: None,
            };
            let op = crate::claims::preference_op(evidence, true);
            ops.push((raw::OpKind::Claim, serde_json::to_value(op).unwrap()));
        }
        let decided = "Ship the beta on Friday.";
        ops.push(
            claimed(
                &mut store,
                // Older than every rule: the pool's 200 newest are the rules.
                said(cwd.path(), DAY / 2, decided),
                "decision",
                "decided",
                vec![],
            )
            .0,
        );
        store.append_ops(&ops).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            lines_of(&text, "Preferences of this repository").len(),
            POOL
        );
        let decisions = lines_of(&text, "Decisions and open items");
        assert!(
            decisions.iter().any(|l| l.ends_with(decided)),
            "{decisions:?}"
        );
    }

    /// Spec 4.4: the owner's global preferences first, then about 10 claims with their bodies, and
    /// the rest one line each after the tools that find more, last.
    #[test]
    fn global_preferences_come_first_and_about_ten_claims_have_bodies() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let ops: Vec<_> = (1..=12)
            .map(|day| {
                let text = format!("Rule {day:02}.");
                claimed(
                    &mut store,
                    said(cwd.path(), day * DAY, &text),
                    "decision",
                    "decided",
                    vec![],
                )
                .0
            })
            .collect();
        store.append_ops(&ops).unwrap();
        crate::claims::pref_add(home.path(), "Answer in Japanese.").unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(text.starts_with("## Global preferences\n"), "{text}");
        let prefs = lines_of(&text, "Global preferences");
        assert_eq!(prefs.len(), 1);
        assert!(
            prefs[0].ends_with(" preference: Answer in Japanese."),
            "{prefs:?}"
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10);
        assert_eq!(bodies[0], "- 1970-01-13 decision: Rule 12.");
        assert_eq!(bodies[9], "- 1970-01-04 decision: Rule 03.");
        let more = lines_of(&text, "More from memory");
        assert_eq!(more[0], TOOLS);
        assert_eq!(more.len(), 3);
        assert!(
            more[1].ends_with(" 1970-01-03 decision: Rule 02."),
            "{more:?}"
        );
        assert!(
            more[2].ends_with(" 1970-01-02 decision: Rule 01."),
            "{more:?}"
        );
        // The gate trims the text's end.
        assert!(text.ends_with(&more[2]), "{text}");
    }

    /// Spec 4.4: claims are chosen by the words they share with the manifest's last prompt and
    /// files, then by recency, and listed newest first whatever chose them.
    #[test]
    fn the_chosen_claims_are_listed_newest_first_whatever_chose_them() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let mut ops = vec![
                    claimed(
                        store,
                        said(cwd, DAY, "Walrus names stay short."),
                        "decision",
                        "decided",
                        vec![],
                    )
                    .0,
                ];
                for day in 2..=11 {
                    let text = format!("Rule {day:02}.");
                    ops.push(
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0,
                    );
                }
                ops
            },
            Some("Rename the walrus module"),
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10);
        assert_eq!(bodies[0], "- 1970-01-12 decision: Rule 11.");
        assert_eq!(bodies[9], "- 1970-01-02 decision: Walrus names stay short.");
        let more = lines_of(&text, "More from memory");
        assert!(
            more[1].ends_with(" 1970-01-03 decision: Rule 02."),
            "{more:?}"
        );
    }

    /// Spec 4.4: a pair is listed as one unit, dated by its later claim: a claim dated between the
    /// two never separates them.
    #[test]
    fn a_claim_dated_between_a_pair_never_separates_it() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (width, _) = claimed(
                    store,
                    said(cwd, 2 * DAY, "Keep lines under 100."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, width, spaces]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            [
                "- 1970-01-04 decision: Use spaces instead.",
                "- 1970-01-02 decision, superseded by the 1970-01-04 decision above: Use tabs.",
                "- 1970-01-03 decision: Keep lines under 100.",
            ]
        );
    }

    /// D4: a checkout with no manifest to show (here a branch with no records) still gets the
    /// repository's delivered claims.
    #[test]
    fn a_checkout_with_claims_and_no_manifest_row_still_gets_them() {
        let (home, _c, _text) = start(
            |store, cwd| {
                vec![
                    claimed(
                        store,
                        said(cwd, DAY, "Use tabs."),
                        "decision",
                        "decided",
                        vec![],
                    )
                    .0,
                ]
            },
            None,
        );
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let text = text(
            home.path(),
            &store,
            "r",
            "feature",
            "none",
            &rules,
            usize::MAX,
            INDEX,
            NOW,
        )
        .unwrap()
        .unwrap()
        .text;
        assert!(!text.contains("## As of"), "{text}");
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 decision: Use tabs."]
        );
    }

    /// D3, #302 item 2: what raw holds and the worker has not applied yet is never shown: a claim
    /// the owner retracted or corrected, and one that quotes a removed record, with the claim a
    /// retracted one ended. Once the worker applies them, what they say
    /// is shown.
    #[test]
    fn an_owner_retraction_the_worker_has_not_applied_is_not_shown() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (tabs, old) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (width, width_uid) = claimed(
            &mut store,
            said(cwd.path(), 2 * DAY, "Keep lines under 100."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, spaces_uid) = claimed(
            &mut store,
            said(cwd.path(), 3 * DAY, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (pin, _) = claimed(
            &mut store,
            said(cwd.path(), 4 * DAY, "Pin the toolchain."),
            "decision",
            "decided",
            vec![],
        );
        let anchor = |op: &(crate::raw::OpKind, Value)| op.1["evidence"][0].clone();
        let (spaces_at, width_at, pin_at) = (anchor(&spaces), anchor(&width), anchor(&pin));
        store.append_ops(&[tabs, width, spaces, pin]).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(lines_of(&text, "Decisions and open items").len(), 4);
        let correct = |uid: &str, at: &Value, status: Value, body: Value| {
            let op = serde_json::json!({"uid": uid, "status": status, "body": body,
                "anchor": {"device": at["device"], "seq": at["seq"]}});
            (crate::raw::OpKind::Correction, op)
        };
        store
            .append_ops(&[
                correct(&spaces_uid, &spaces_at, "retracted".into(), Value::Null),
                correct(
                    &width_uid,
                    &width_at,
                    Value::Null,
                    "Keep lines under 80.".into(),
                ),
            ])
            .unwrap();
        let device = store.device().to_owned();
        store
            .append_tombstone(Target::Record {
                device,
                seq: pin_at["seq"].as_i64().unwrap(),
            })
            .unwrap();
        let text = shown(home.path(), &store).unwrap_or_default();
        assert!(!text.contains("## Decisions and open items"), "{text}");
        assert!(!text.contains("## More from memory"), "{text}");
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-03 decision: Keep lines under 80."]
        );
    }

    /// Spec 4.9's drop order under an agent's size cap: the hook cuts from the end, so the index
    /// goes first and the manifest's own sections stay.
    #[test]
    fn the_index_goes_first_under_the_size_cap() {
        let (_h, _c, text) = start(
            |store, cwd| {
                (1..=14)
                    .map(|day| {
                        let text = format!("Rule {day:02}.");
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0
                    })
                    .collect()
            },
            None,
        );
        let at = text.find("## More from memory").unwrap();
        let cut = crate::manifest::cut(&text, at + 1);
        assert!(!cut.contains("## More from memory"), "{cut}");
        assert!(
            cut.contains("## Files touched") && cut.contains("## As of"),
            "{cut}"
        );
    }
