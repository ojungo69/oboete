use super::*;
use serde_json::json;

const MIN: i64 = 60_000;

fn entry(list: &str, fields: Value, ts: i64) -> Entry {
    Entry {
        list: list.into(),
        fields: fields.as_object().unwrap().clone(),
        ts,
    }
}

/// claude-mem's fold and lines: state merged key by key, a null clearing a key, a task's
/// fields merged in place, `done` and `dropped` in any case closing a task, the state's
/// status closing the list while its open task still shows, and the newest list first.
#[test]
fn lists_fold_as_claude_mem_folds_them() {
    let entries = [
        entry("release", json!({"phase": "rc1", "owner": "me"}), 0),
        entry(
            "release",
            json!({"task": "changelog", "status": "todo"}),
            MIN,
        ),
        entry(
            "release",
            json!({"task": "notes", "status": "doing", "pr": 7}),
            2 * MIN,
        ),
        entry("auth", json!({"task": "tokens", "status": "DONE"}), 3 * MIN),
        entry("release", json!({"phase": "rc2", "owner": null}), 4 * MIN),
        entry(
            "release",
            json!({"task": "changelog", "status": "doing", "by": 1.0}),
            5 * MIN,
        ),
        entry(
            "auth",
            json!({"task": "login", "status": "Dropped"}),
            6 * MIN,
        ),
        entry(
            "cleanup",
            json!({"status": "done", "why": "merged"}),
            7 * MIN,
        ),
        entry(
            "cleanup",
            json!({"task": "branches", "status": "todo"}),
            8 * MIN,
        ),
    ];
    let now = 128 * MIN;
    assert_eq!(
        lines(&entries, now, false),
        [
            "- cleanup",
            "  - [todo] branches, updated about 2 hours ago",
            "- release: phase=rc2, updated about 2 hours ago",
            "  - [doing] changelog (by=1), updated about 2 hours ago",
            "  - [doing] notes (pr=7), updated about 2 hours ago",
        ]
    );
    assert_eq!(
        lines(&entries, now, true)[..5],
        [
            "- cleanup: status=done, why=merged, updated about 2 hours ago",
            "  - [todo] branches, updated about 2 hours ago",
            "- auth",
            "  - [DONE] tokens, updated about 2 hours ago",
            "  - [Dropped] login, updated about 2 hours ago",
        ]
    );
    // An empty or null `task` is the list's own state.
    let state = [
        entry("l", json!({"task": "", "k": 1}), 0),
        entry("l", json!({"task": null, "j": true}), 0),
    ];
    assert_eq!(
        lines(&state, 0, false),
        ["- l: k=1, j=true, updated 1 minute ago"]
    );
}

/// Codex on #408: names and keys that differ only in the spaces around them are as many as
/// claude-mem keeps.
#[test]
fn names_differing_in_spaces_stay_apart() {
    let entries = [
        entry("l", json!({"task": "ship", "status": "todo"}), 0),
        entry("l", json!({"task": " ship ", "status": "done"}), 0),
        entry("l", json!({"a": 1, " a": 2}), 0),
    ];
    assert_eq!(
        lines(&entries, 0, false),
        [
            "- l: a=1,  a=2, updated 1 minute ago",
            "  - [todo] ship, updated 1 minute ago"
        ]
    );
}

/// A number reads as JavaScript's `String(JSON.parse(s))` writes it (node 24's answers), so
/// one number sent in two spellings names one task (Codex on #408).
#[test]
fn numbers_read_as_javascript_writes_them() {
    for (sent, js) in [
        ("1.0", "1"),
        ("-0.0", "0"),
        ("0.1", "0.1"),
        ("123.456", "123.456"),
        ("0.000001", "0.000001"),
        ("2.5e-6", "0.0000025"),
        ("1e-7", "1e-7"),
        ("123e-20", "1.23e-18"),
        ("1e16", "10000000000000000"),
        ("10000000000000000", "10000000000000000"),
        ("1e20", "100000000000000000000"),
        ("1e21", "1e+21"),
        ("-1.2345e25", "-1.2345e+25"),
        ("9007199254740993", "9007199254740992"),
        ("1152921504606846976", "1152921504606847000"),
        ("18446744073709551615", "18446744073709552000"),
        ("-9223372036854775808", "-9223372036854776000"),
        ("5e-324", "5e-324"),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
        // Two shortest spellings read back as this double: JavaScript takes the nearer one
        // (Codex on #408).
        ("652282746268236.2", "652282746268236.2"),
    ] {
        let v: Value = serde_json::from_str(sent).unwrap();
        assert_eq!(text(&v), js, "{sent}");
    }
    let task = |s: &str, ts| entry("l", serde_json::from_str(s).unwrap(), ts);
    let open = [task(r#"{"task": 10000000000000000, "status": "todo"}"#, 0)];
    assert!(!lines(&open, 0, false).is_empty());
    let closed = [
        task(r#"{"task": 10000000000000000, "status": "todo"}"#, 0),
        task(r#"{"task": 1e16, "status": "done"}"#, 0),
    ];
    let named = [
        task(r#"{"task": "n", "status": "todo"}"#, 0),
        task(r#"{"task": "n", "status": "done"}"#, 0),
    ];
    assert_eq!(lines(&closed, 0, false), lines(&named, 0, false));
}

/// claude-mem's `describeDuration`.
#[test]
fn times_read_as_claude_mem_writes_them() {
    for (ms, said) in [
        (0, "1 minute"),
        (89_999, "1 minute"),
        (90_000, "2 minutes"),
        (59 * MIN, "59 minutes"),
        (60 * MIN, "about 1 hour"),
        (47 * 60 * MIN, "about 47 hours"),
        (48 * 60 * MIN, "about 2 days"),
        (-5 * MIN, "1 minute"),
    ] {
        assert_eq!(ago(ms), said, "{ms}");
    }
}

/// Each check refuses as claude-mem's do, the fields' size in UTF-16 units of their JSON.
#[test]
fn a_write_is_checked_as_claude_mem_checks_it() {
    let ok = json!({"task": "a"});
    let fields = |v: Value| v.as_object().unwrap().clone();
    assert_eq!(
        check("  release ", &fields(ok.clone())),
        Ok("release".into())
    );
    for list in ["", "   ", &"x".repeat(201)] {
        assert!(check(list, &fields(ok.clone())).is_err(), "{list:?}");
    }
    assert!(check(&"界".repeat(200), &fields(ok)).is_ok());
    for bad in [
        json!({}),
        json!({"": 1}),
        json!({"a": [1]}),
        json!({"a": {"b": 1}}),
    ] {
        assert!(check("l", &fields(bad.clone())).is_err(), "{bad}");
    }
    // {"a":"…"} is 8 units around the value.
    assert!(check("l", &fields(json!({"a": "界".repeat(1_992)}))).is_ok());
    assert!(check("l", &fields(json!({"a": "界".repeat(1_993)}))).is_err());
}

/// claude-mem's cut: as many lines as fit with room for the line that says how many were left
/// out, which the last line needs none of.
#[test]
fn lines_are_cut_with_how_many_were_left_out() {
    let lines: Vec<String> = (0..5).map(|i| format!("- list{i}")).collect();
    let fit = |heading: &str, n: usize, limit: usize| super::fit(heading, &lines[..n], limit);
    assert_eq!(
        fit("head", 5, 1_000),
        "head\n- list0\n- list1\n- list2\n- list3\n- list4"
    );
    let cut = fit("head", 5, 70);
    assert_eq!(
        cut,
        "head\n- list0\n- list1\n- ...3 more lines; read them with work_state_read"
    );
    assert!(cut.encode_utf16().count() <= 70);
    assert_eq!(
        fit("", 1, 3),
        "- ...1 more line; read them with work_state_read"
    );
    // Codex's security review of #408: at every limit, the cut stays within it, but where
    // not even the first line fits and the count alone is left.
    for limit in 0..200 {
        let cut = fit("", 5, limit);
        assert!(units(&cut) <= limit || cut == more(5, ""), "{limit}: {cut}");
    }
}

/// Codex's security review of #408: the section is measured as its fence holds it, so closing
/// tags the fence quotes, which are longer then, never take it past its limit.
#[test]
fn quoted_closing_tags_keep_the_section_within_its_limit() {
    let tags = "</oboete-memory>".repeat(100);
    let e: Vec<Entry> = (0..3)
        .map(|i| entry(&format!("l{i}"), json!({"note": tags}), 0))
        .collect();
    for limit in [1_000, 2_000, SECTION] {
        let s = section(&e, 0, limit, &Rules::default());
        assert!(s.units() <= limit, "{} > {limit}", s.units());
    }
    // A line too long once quoted is left out, and the count of lines stays in the fence.
    let s = section(&e, 0, SECTION, &Rules::default());
    assert_eq!(s.open.as_deref(), Some(more(3, "").as_str()));
}

/// Codex's security review of #408: at every size around the cut, recorded text stays inside
/// the fence: what stands outside it is the rule, or the rule and a count.
#[test]
fn no_size_puts_recorded_text_outside_the_fence() {
    for long in (1_850..2_100).step_by(3) {
        let entries = [
            entry("other", json!({"task": "x"}), 0),
            entry("release", json!({"note": "n".repeat(long)}), MIN),
        ];
        for limit in [1_000, 1_150, 1_250, 1_350, SECTION] {
            let s = section(&entries, MIN, limit, &Rules::default());
            assert!(
                [RULE.to_owned(), format!("{RULE}\n\n{}", more(3, ""))].contains(&s.rule)
                    || [
                        SHORT_RULE.to_owned(),
                        format!("{SHORT_RULE}\n\n{}", more(3, ""))
                    ]
                    .contains(&s.rule),
                "{long} {limit}"
            );
            assert!(s.units() <= limit, "{long} {limit}: {}", s.units());
        }
    }
}

/// The rule is claude-mem's, with oboete's tool and its repository; with claude-mem 15.3's line
/// on the deliverables it copies (`RULE_COPIES`, row G01) in oboete's words: oboete copies
/// when the turn ends, into its own folder, and links the copy on the request's card, as
/// claude-mem's line says (slice 2b).
#[test]
fn the_rule_is_claude_mems() {
    let theirs = [
            "# Work state: your to-do lists and working state",
            "Use claude-mem's work_state_write tool to track all to-do lists and multi-step work. It is your canonical to-do list: use it instead of any built-in to-do tool. Also use it to track the state of anything you need an ongoing understanding of. Whatever is still open is shown here at the start of every session in this project.",
            "- One list per to-do list or tracked thing: work_state_write with list=\"<name>\" and the fields to set",
            "- To-do item: fields {\"task\": \"<name>\", \"status\": \"todo\" | \"doing\" | \"done\" | \"dropped\", ...any details}",
            "- State: fields {\"<key>\": <value>} (the latest value of each key wins; null clears a key; \"status\": \"done\" closes the list)",
            "- Made something a person can open (PR, deck, page, image, report)? Attach it to that item: fields {\"work_product\": \"https://…\" or \"/absolute/file\"}; several, with titles, as one JSON string: \"[{\\\"title\\\": \\\"<name>\\\", \\\"url\\\": \\\"https://…\\\"}]\"",
            "- Read every list, closed items included: work_state_read with includeClosed=true",
        ]
        .join("\n");
    let ours = theirs
        .replace("claude-mem's work_state_write", "oboete's work_state_write")
        .replace("in this project.", "in this repository.");
    assert_eq!(RULE, ours);
    let theirs_copies = "- Deliverables: write a deck, report, page or other file the person asked for as a file and attach it as work_product before you finish. When you stop, claude-mem copies each attached file (and any deck, document, page, image or spreadsheet you wrote outside the source tree) into this project's deliverables bucket, ~/.claude-mem/deliverables/<project>/, and links it on the request's card in My Work.";
    let ours_copies = theirs_copies
        .replace("When you stop, claude-mem", "When your turn ends, oboete")
        .replace(
            "into this project's deliverables bucket, ~/.claude-mem/deliverables/<project>/, and",
            "into this repository's folder under ~/.oboete/deliverables/ and",
        );
    let read = "\n- Read every list";
    assert_eq!(
        RULE_COPIES,
        ours.replace(read, &format!("\n{ours_copies}{read}"))
    );
    // The least size keeps neither attach line, with the line on copies or without.
    for copies in [false, true] {
        let small = section_with(&[], 0, 1_000, &Rules::default(), copies);
        assert_eq!(
            small.rule,
            format!("{SHORT_RULE}\n\n{NOTHING_OPEN}"),
            "{copies}"
        );
    }
    let whole = section_with(&[], 0, SECTION, &Rules::default(), true);
    assert!(whole.rule.starts_with(RULE_COPIES), "{}", whole.rule);
}

/// The section: the rule alone with nothing open, else the rule and the open lines, gated
/// and fitted, the whole within the limit.
#[test]
fn the_section_leads_with_the_rule() {
    let none = section(&[], 0, SECTION, &Rules::default());
    assert_eq!(
        none,
        Section {
            rule: format!("{RULE}\n\nNothing open yet."),
            open: None
        }
    );
    let entries: Vec<Entry> = (0..200)
        .map(|i| entry(&format!("list {i}"), json!({"note": "x".repeat(40)}), i))
        .collect();
    let s = section(
        &entries,
        0,
        SECTION,
        &user(r#"{ id = "n", regex = '199' }"#),
    );
    let open = s.open.as_deref().unwrap();
    assert!(open.starts_with("- list [REDACTED]: note="), "{open}");
    assert!(
        open.ends_with("more lines; read them with work_state_read"),
        "{open}"
    );
    assert_eq!(s.rule, RULE);
    assert!(s.units() <= SECTION, "{}", s.units());
    assert!(s.units() > SECTION - 80, "{}", s.units());
    assert!(s.text().contains("<oboete-memory>\nWhat agents wrote"));
    // `session_start_chars` at its least leaves no room for the fence: the count alone, after
    // the rule without its attach line (row G01).
    let least = section(&entries, 0, 1_000, &Rules::default());
    assert_eq!(
        least,
        Section {
            rule: format!("{SHORT_RULE}\n\n- ...200 more lines; read them with work_state_read"),
            open: None
        }
    );
    assert!(least.units() <= 1_000, "{}", least.units());
    // Where the whole rule leaves no room for a line and the short rule does, the open lines
    // follow the short rule (#703).
    let fence = units(&crate::manifest::fence(OPEN, ""));
    let small = units(RULE) + 3 + fence + 10;
    let s = section(&entries, 0, small, &Rules::default());
    let open = s.open.as_deref().unwrap_or_else(|| panic!("{s:?}"));
    assert_eq!(s.rule, SHORT_RULE);
    assert!(
        open.starts_with("- list ") && open.contains("more lines"),
        "{open}"
    );
    assert!(s.units() <= small, "{} {small}", s.units());
    // Where the whole rule leaves room for the count line alone, the short rule's lines still
    // win: a fence of the count alone is no try (#711).
    let count = units(&more(entries.len(), ""));
    let counted = units(RULE) + 3 + fence + count;
    let s = section(&entries, 0, counted, &Rules::default());
    let open = s.open.as_deref().unwrap_or_else(|| panic!("{s:?}"));
    assert_eq!(s.rule, SHORT_RULE);
    assert!(open.starts_with("- list "), "{open}");
    assert!(s.units() <= counted, "{} {counted}", s.units());
    // Where no line fits after either rule, the count follows the whole rule when it fits.
    let long: Vec<Entry> = (0..3)
        .map(|i| entry(&format!("list {i}"), json!({"note": "y".repeat(600)}), i))
        .collect();
    let s = section(&long, 0, 1_200, &Rules::default());
    assert_eq!(
        s,
        Section {
            rule: format!("{RULE}\n\n{}", more(3, "")),
            open: None
        }
    );
    assert_eq!(
        section(&[], 0, 1_000, &Rules::default()).rule,
        format!("{SHORT_RULE}\n\nNothing open yet.")
    );
    // The whole rule as soon as it fits, with the newline that joins it to what follows.
    let whole = format!("{RULE}\n\nNothing open yet.");
    let fits = units(&whole) + 1;
    assert_eq!(section(&[], 0, fits, &Rules::default()).rule, whole);
    assert_eq!(section(&[], 0, fits, &Rules::default()).units(), fits);
    assert!(
        section(&[], 0, fits - 1, &Rules::default())
            .rule
            .starts_with(SHORT_RULE)
    );
}

fn user(extra: &str) -> Rules {
    let toml = format!("[redaction]\nextra_rules = [{extra}]\n");
    let capture = crate::config::parse_capture(Some(&toml)).unwrap();
    Rules::new(&capture.redaction).unwrap()
}

/// Codex's security review of #408 (6eec12b): a rule added after a write sees each name, key
/// and value as written: a key or a list's name it masks alone still gives it the context it
/// needs around a value, and a mask that starts in a key hides the key even where the masked
/// text spells the key again.
#[test]
fn a_rule_added_later_sees_names_and_keys_as_written() {
    let key = user(r#"{ id = "k", regex = '^phase(?: = "teal-[0-9]{4}")?$' }"#);
    let entries = [entry("release", json!({"phase": "teal-1234"}), 0)];
    let text = read(None, false, "r", &entries, 0, &key);
    assert!(!text.contains("teal-1234"), "{text}");
    let name = user(r#"{ id = "n", regex = '(?s)FOO(?:.*(teal-1234))?', secret_group = 1 }"#);
    let entries = [entry("FOO", json!({"task": "teal-1234"}), 0)];
    let open = section(&entries, 0, SECTION, &name).text();
    assert!(
        !open.contains("teal-1234") && !open.contains("FOO"),
        "{open}"
    );
    // Two keys a rule masks alike are one key, shown as written with its value, which a rule
    // that needs that key then sees (Codex's security review of #408).
    let both = user(r#"{ id = "b", regex = '^key[12]$|key2 = "teal-[0-9]{4}"' }"#);
    let entries = [
        entry("l", json!({"key1": "harmless"}), 0),
        entry("l", json!({"key2": "teal-1234"}), 0),
    ];
    let text = read(None, false, "r", &entries, 0, &both);
    assert!(!text.contains("teal-1234"), "{text}");
    let spoof = user(
        r#"{ id = "s", regex = '^(\[REDACTED\]teal-[0-9]{4} = ")teal-[0-9]{4} = "private"$', secret_group = 1 }"#,
    );
    let entries = [entry(
        "l",
        json!({"[REDACTED]teal-1234": "teal-1234 = \"private"}),
        0,
    )];
    let text = read(None, false, "r", &entries, 0, &spoof);
    assert!(text.starts_with("- l: [REDACTED]=[REDACTED],"), "{text}");
    // A mask that starts in the ` = "` between them hides the key and the value, as capture's
    // does (Codex on #408).
    let between = user(r#"{ id = "b", regex = 'phase( = ")teal-1234', secret_group = 1 }"#);
    let entries = [entry("release", json!({"phase": "teal-1234"}), 0)];
    let text = read(None, false, "r", &entries, 0, &between);
    assert!(
        text.starts_with("- release: [REDACTED]=[REDACTED],"),
        "{text}"
    );
}

/// A list's name is read beside its key, as capture stores it (`capture::work_state`): a rule
/// added later that needs `list = "…"` hides the name in a read, the section and a write's
/// answer, the writes before the rule and the one after it (stored under the masked name) are
/// one list, a read by the name finds that list, and a list written only before the rule shows
/// its name hidden.
#[test]
fn a_list_name_a_rule_masks_beside_its_key_is_hidden_and_one_list() {
    let rules = user(r#"{ id = "l", regex = 'list = "canary-[0-9]{4}' }"#);
    let entries = [
        entry(
            "canary-4827",
            json!({"task": "rotate", "status": "todo"}),
            0,
        ),
        entry(
            "canary-4827",
            json!({"task": "audit", "status": "doing"}),
            MIN,
        ),
        entry(
            redact::MASK,
            json!({"task": "rotate", "status": "done"}),
            2 * MIN,
        ),
    ];
    let now = 3 * MIN;
    for text in [
        read(None, false, "r", &entries, now, &rules),
        section(&entries, now, SECTION, &rules).text(),
        read(Some("canary-4827"), false, "r", &entries, now, &rules),
        written("canary-4827", "r", &entries, now, &rules),
    ] {
        assert!(
            !text.contains("canary-") && !text.contains("rotate") && text.contains("audit"),
            "{text}"
        );
    }
    let before = read(None, false, "r", &entries[..2], now, &rules);
    assert!(
        !before.contains("canary-") && before.contains("rotate"),
        "{before}"
    );
    // The name shows as the mask, not left out (Sourcery on #579).
    let text = read(None, false, "r", &entries, now, &rules);
    assert!(text.starts_with("- [REDACTED]\n"), "{text}");
    assert!(before.starts_with("- [REDACTED]\n"), "{before}");
    let saved = written("canary-4827", "r", &entries, now, &rules);
    assert!(
        saved.starts_with("Saved to \"[REDACTED]\" in r. Still open in it:\n- [REDACTED]\n"),
        "{saved}"
    );
    let none = read(Some("canary-1111"), false, "r", &[], now, &rules);
    assert_eq!(
        none,
        "Nothing open in \"[REDACTED]\" for r. Pass includeClosed to see closed items."
    );
}

/// A name is told apart as capture keeps it now: a list's and a task's writes before rules
/// that mask its name and the write after them, stored as capture kept it, are one, and a later
/// `done` closes the task, kept in the closed history (#580, #582): where the name alone and
/// beside its key are masked in different parts (Codex and Greptile on #579), where they are
/// masked in different parts that read alike (Codex on #581), and where a rule anchored on the
/// name's start matches it only trimmed (CodeRabbit on #581).
#[test]
fn a_name_is_one_list_or_task_before_and_after_rules_as_capture_keeps_it() {
    let pair = |key: &str, rule: &str| {
        format!(r#"{{ id = "k", regex = '{key} = "{rule}', secret_group = 1 }}"#)
    };
    // Each case: the key, the name, the rule on the name alone, the rule beside its key, and
    // the name shown before the write after the rules and after it, where the name settles;
    // where a rule matches the gate's own mask, the name before may show as [REDACTED] whole.
    let cases = [
        (
            "list",
            "alpha teal-1234",
            r#"{ id = "a", regex = '^alpha' }"#,
            r"alpha (teal-[0-9]{4})",
            Some("[REDACTED]"),
        ),
        (
            "task",
            "alpha teal-1234",
            r#"{ id = "a", regex = '^alpha' }"#,
            r"alpha (teal-[0-9]{4})",
            Some("[REDACTED]"),
        ),
        (
            "task",
            "[REDACTED]:q:q:[REDACTED]",
            r#"{ id = "a", regex = '^\[REDACTED\]:q' }"#,
            r"\[REDACTED\]:q:(q:\[REDACTED\])",
            None,
        ),
        (
            "task",
            " alpha teal-1234",
            r#"{ id = "a", regex = '^alpha' }"#,
            r" alpha (teal-[0-9]{4})",
            Some(" [REDACTED] [REDACTED]"),
        ),
        // A rule that matches what an earlier mask made (Greptile on #583).
        (
            "list",
            "alpha teal-1234",
            r#"{ id = "a", regex = '^\[REDACTED\] (teal-[0-9]{4})', secret_group = 1 }"#,
            r"(alpha)",
            None,
        ),
        (
            "task",
            "alpha teal-1234",
            r#"{ id = "a", regex = '^\[REDACTED\] (teal-[0-9]{4})', secret_group = 1 }"#,
            r"(alpha)",
            None,
        ),
    ];
    // A read's list name and task name.
    let names = |text: &str| -> (String, String) {
        let lines: Vec<&str> = text.lines().collect();
        let list = lines[0].strip_prefix("- ").unwrap().to_owned();
        let task = lines[1].split_once("] ").unwrap().1;
        (list, task.rsplit_once(", updated ").unwrap().0.to_owned())
    };
    for (key, name, alone, keyed, settled) in cases {
        let extra = format!("{alone}, {}", pair(key, keyed));
        let home = tempfile::tempdir().unwrap();
        let config = format!("[redaction]\nextra_rules = [{extra}]\n");
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let (list, task) = match key {
            "list" => (name, "rotate"),
            _ => ("l", name),
        };
        let done = json!({"task": task, "status": "done"});
        let (later_list, later) =
            crate::capture::work_state(list, done.as_object().unwrap(), &settings);
        let entries = [
            entry(list, json!({"task": task, "status": "todo"}), 0),
            Entry {
                list: later_list,
                fields: later,
                ts: MIN,
            },
        ];
        let rules = user(&extra);
        let open = read(None, false, "r", &entries, 2 * MIN, &rules);
        assert!(
            open.starts_with("Nothing open for r."),
            "{key} {name:?}: {open}"
        );
        let all = read(None, true, "r", &entries, 2 * MIN, &rules);
        let lines: Vec<&str> = all.lines().collect();
        assert!(
            lines.len() == 2 && lines[0].starts_with("- ") && lines[1].starts_with("  - [done] "),
            "{key} {name:?}: {all}"
        );
        assert!(!all.contains("teal-"), "{key} {name:?}: {all}");
        if key == "list" {
            let named = read(Some(name), true, "r", &entries, 2 * MIN, &rules);
            assert_eq!(named, all, "{name:?}");
        }
        // Before the write after the rules, a name shows as after it where it settles, else no
        // more than after it; and a read by the list's name as shown finds it (#584, #586).
        let before = read(None, true, "r", &entries[..1], 2 * MIN, &rules);
        assert!(!before.contains("teal-"), "{key} {name:?}: {before}");
        let ((bl, bt), (al, at)) = (names(&before), names(&all));
        let (b, a) = if key == "list" { (bl, al) } else { (bt, at) };
        match settled {
            Some(shown) => assert!(b == shown && a == shown, "{key} {name:?}: {before}\n{all}"),
            None => assert!(
                b == a || b == redact::MASK,
                "{key} {name:?}: {before}\n{all}"
            ),
        }
        let (shown, _) = names(&before);
        let named = read(Some(&shown), true, "r", &entries[..1], 2 * MIN, &rules);
        assert_eq!(named, before, "{key} {name:?} {shown:?}");
    }
}

/// A read by name matches lists once folded, by the name the list's line shows or by its fold
/// name, and keeps the matching list's whole history: a list folded from two names, whose line
/// a rule written against it masks (`^- `, as #411's rules are written), read by the name the
/// line shows (Greptile, Sourcery and CodeRabbit on #585).
#[test]
fn a_read_by_the_name_a_line_shows_finds_the_whole_list() {
    let rules = user(
        r#"{ id = "t", regex = 'teal-[0-9]{4}' }, { id = "l", regex = '^- (alpha) ', secret_group = 1 }"#,
    );
    let entries = [
        entry("alpha teal-1234", json!({"task": "a", "status": "todo"}), 0),
        entry(
            "alpha teal-5678",
            json!({"task": "b", "status": "todo"}),
            MIN,
        ),
    ];
    let now = 2 * MIN;
    let all = read(None, false, "r", &entries, now, &rules);
    assert_eq!(
        all,
        "- [REDACTED] [REDACTED]\n  - [todo] a, updated 2 minutes ago\n  \
             - [todo] b, updated 1 minute ago"
    );
    for asked in [
        "[REDACTED] [REDACTED]",
        "alpha teal-1234",
        "alpha [REDACTED]",
    ] {
        let named = read(Some(asked), false, "r", &entries, now, &rules);
        assert_eq!(named, all, "{asked:?}");
    }
}

/// A list's shown name is the name its line shows in a read of that list alone, so no other
/// list's history changes it: an archive whose own lines pass the gate's finding limit leaves
/// `alpha beta` found by `[REDACTED] beta`, the name an open read shows (Greptile on #589).
#[test]
fn a_list_is_found_by_its_shown_name_whatever_the_other_lists_hold() {
    let rules = user(
        r#"{ id = "l", regex = '^- (alpha) ', secret_group = 1 }, { id = "s", regex = 'secret-[0-9]+' }"#,
    );
    let note: String = (0..10).map(|j| format!("secret-{j} ")).collect();
    let mut entries = vec![entry(
        "alpha beta",
        json!({"task": "a", "status": "todo"}),
        0,
    )];
    for i in 0..101 {
        let fields = json!({"task": format!("t{i}"), "status": "done", "note": note});
        entries.push(entry("archive", fields, MIN));
    }
    let now = 2 * MIN;
    let open = read(None, false, "r", &entries, now, &rules);
    assert_eq!(
        open,
        "- [REDACTED] beta\n  - [todo] a, updated 2 minutes ago"
    );
    let named = read(Some("[REDACTED] beta"), true, "r", &entries, now, &rules);
    assert_eq!(named, open);
    // A line the gate hides from its bullet on shows no name: every line of every list read
    // together is one mask here.
    let all = read(None, true, "r", &entries, now, &rules);
    assert_eq!(all, vec![redact::MASK; 104].join("\n"));
    assert_eq!(
        read(Some("[REDACTED]"), true, "r", &entries, now, &rules),
        "Nothing recorded in \"[REDACTED]\" for r."
    );
}

/// A rule that needs a keyword masks a name where the keyword is among the lines read with it:
/// a read of every list shows `alpha beta` as `[REDACTED] beta` where another list's line holds
/// the keyword, and a read by that name finds the list (#590's review).
#[test]
fn a_list_is_found_by_the_name_a_read_of_every_list_shows() {
    let rules = user(r#"{ id = "l", regex = 'alpha', keywords = ["trigger"] }"#);
    let entries = [
        entry("alpha beta", json!({"task": "a", "status": "todo"}), 0),
        entry(
            "other",
            json!({"task": "trigger it", "status": "todo"}),
            MIN,
        ),
    ];
    let now = 2 * MIN;
    let all = read(None, false, "r", &entries, now, &rules);
    assert_eq!(
        all,
        "- other\n  - [todo] trigger it, updated 1 minute ago\n\
             - [REDACTED] beta\n  - [todo] a, updated 2 minutes ago"
    );
    // Read alone, its lines hold no keyword, so the rule masks nothing in them, as on main.
    let named = read(Some("[REDACTED] beta"), false, "r", &entries, now, &rules);
    assert_eq!(named, "- alpha beta\n  - [todo] a, updated 2 minutes ago");
}

/// A name counts as shown only where the line, as a read writes it, begins with its bullet and
/// that name: a rule on the masked line that hides the name again leaves no line showing it,
/// so a read by the name masked once finds nothing; the list's fold name is `alpha beta`, as
/// the first rule needs the line (Codex on #593).
#[test]
fn a_name_counts_only_where_the_line_as_written_shows_it() {
    let rules = user(
        r#"{ id = "a", regex = '^- (alpha) ', secret_group = 1 }, { id = "m", regex = '^- \[REDACTED\] beta' }"#,
    );
    let entries = [entry(
        "alpha beta",
        json!({"task": "a", "status": "todo"}),
        0,
    )];
    let now = MIN;
    let all = read(None, false, "r", &entries, now, &rules);
    assert!(all.starts_with("[REDACTED]\n"), "{all}");
    assert_eq!(
        read(Some("[REDACTED] beta"), false, "r", &entries, now, &rules),
        "Nothing open in \"[REDACTED] beta\" for r. Pass includeClosed to see closed items."
    );
}

/// A read by a masked name reads no list name in a task's line: a list name longer than its
/// last task's line, or one of several bytes a character, is found by the name its line shows
/// (Greptile and Codex on #593).
#[test]
fn a_long_or_wide_list_name_is_read_by_its_shown_name() {
    let rules = user(r#"{ id = "l", regex = '^- (alpha) ', secret_group = 1 }"#);
    for rest in ["x".repeat(80), "ä".repeat(40)] {
        let name = format!("alpha {rest}");
        let entries = [entry(&name, json!({"task": "a", "status": "todo"}), 0)];
        let shown = format!("[REDACTED] {rest}");
        let named = read(Some(&shown), false, "r", &entries, MIN, &rules);
        assert_eq!(
            named,
            format!("- {shown}\n  - [todo] a, updated 1 minute ago")
        );
    }
}

/// A numeric task written before rules that need two passes and again after them reads as one
/// task, as a string task does (Sourcery on #589).
#[test]
fn a_numeric_task_written_before_and_after_rules_is_one_task() {
    let extra = r#"{ id = "k", regex = 'task = "(12)', secret_group = 1 }, { id = "a", regex = '^\[REDACTED\](34)', secret_group = 1 }"#;
    let home = tempfile::tempdir().unwrap();
    let config = format!("[redaction]\nextra_rules = [{extra}]\n");
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let settings = crate::capture::Settings::load(home.path()).unwrap();
    let rules = user(extra);
    for task in [json!(1234), json!("1234")] {
        let doing = json!({"task": task, "status": "doing"});
        let (list, later) = crate::capture::work_state("l", doing.as_object().unwrap(), &settings);
        let entries = [
            entry("l", json!({"task": task, "status": "todo"}), 0),
            Entry {
                list,
                fields: later,
                ts: MIN,
            },
        ];
        let text = read(None, false, "r", &entries, 2 * MIN, &rules);
        assert_eq!(
            text, "- l\n  - [doing] [REDACTED][REDACTED], updated 1 minute ago",
            "{task}"
        );
    }
}

/// A value the gate masks more of beside its key once masked is hidden whole: what it would
/// mask there does not show (Sourcery on #585).
#[test]
fn a_masked_value_the_gate_masks_more_of_beside_its_key_is_hidden_whole() {
    let rules = user(
        r#"{ id = "a", regex = '^alpha' }, { id = "k", regex = 'note = "\[REDACTED\] (teal-[0-9]{4})', secret_group = 1 }"#,
    );
    let entries = [entry("l", json!({"note": "alpha teal-1234"}), 0)];
    let text = read(None, false, "r", &entries, MIN, &rules);
    assert_eq!(text, "- l: note=[REDACTED], updated 1 minute ago");
}

/// A rule that masks the closing quote of `task = "…"` leaves capture no value to keep, so
/// a write after it stores the task as [REDACTED]; an earlier write of that task is shown the
/// same way, so the later `done` closes it (Codex on #408).
#[test]
fn a_mask_on_the_closing_quote_hides_the_value_whole() {
    let rule = r#"{ id = "q", regex = 'task = "teal-[0-9]{4}(")', secret_group = 1 }"#;
    let home = tempfile::tempdir().unwrap();
    let config = format!("[redaction]\nextra_rules = [{rule}]\n");
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let settings = crate::capture::Settings::load(home.path()).unwrap();
    let done = json!({"task": "teal-1234", "status": "done"});
    let (_, later) = crate::capture::work_state("l", done.as_object().unwrap(), &settings);
    assert_eq!(later["task"], "[REDACTED]");
    let entries = [
        entry("l", json!({"task": "teal-1234", "status": "todo"}), 0),
        Entry {
            list: "l".into(),
            fields: later,
            ts: MIN,
        },
    ];
    let text = read(None, false, "r", &entries, 2 * MIN, &user(rule));
    assert!(
        !text.contains("teal-1234") && !text.contains("[todo]"),
        "{text}"
    );
}

/// The fields' size is their JSON as JavaScript writes it, numbers included: `1e20` is 21
/// characters there and `1.0` one (Codex on #408).
#[test]
fn the_fields_are_measured_as_javascript_writes_them() {
    let with = |pad: usize, n: Value| {
        let mut f = Map::new();
        f.insert("pad".into(), Value::String("x".repeat(pad)));
        f.insert("n".into(), n);
        check("l", &f)
    };
    let big: Value = serde_json::from_str("1e20").unwrap();
    // {"pad":"…","n":100000000000000000000} is 36 units around the pad.
    assert!(with(1_964, big.clone()).is_ok());
    assert!(with(1_965, big).is_err());
    // {"pad":"…","n":1} is 16.
    let one: Value = serde_json::from_str("1.0").unwrap();
    assert!(with(1_984, one.clone()).is_ok());
    assert!(with(1_985, one).is_err());
}

/// Each line is a view of its own, as written: a rule anchored to a line's start, or written
/// against its `key=value`, sees it whole where another rule masks a value in it (#411).
#[test]
fn a_rule_sees_each_line_as_written() {
    let rules = user(
        r#"{ id = "x", regex = '^- (teal-[0-9]{4}) \(note=ZZ', secret_group = 1 }, { id = "y", regex = '^ZZ$' }"#,
    );
    let entries = [entry("l", json!({"task": "teal-1234", "note": "ZZ"}), 0)];
    let text = read(None, false, "r", &entries, 0, &rules);
    assert!(
        !text.contains("teal-1234") && !text.contains("ZZ"),
        "{text}"
    );
    // #411: a rule written against the line's `key=value` keeps its context where another
    // masks the start of the value.
    let rules = user(
        r#"{ id = "a", regex = '^alpha' }, { id = "b", regex = 'note=alpha (teal-[0-9]{4})', secret_group = 1 }"#,
    );
    let entries = [entry("notes", json!({"note": "alpha teal-1234"}), 0)];
    let text = read(None, false, "r", &entries, 0, &rules);
    assert_eq!(
        text,
        "- notes: note=[REDACTED] [REDACTED], updated 1 minute ago"
    );
}

/// Codex's security review of #408 (6eec12b): a line the cut leaves out still gives a rule
/// its context, since the lines are gated whole before they are cut.
#[test]
fn a_rule_sees_the_lines_a_cut_leaves_out() {
    let rules =
        user(r#"{ id = "f", regex = '(?s)^- (teal-[0-9]{4}).*\n.*FOLLOW', secret_group = 1 }"#);
    let list = format!("teal-1234{}", "a".repeat(100));
    let task = format!("FOLLOW{}", "x".repeat(1_970));
    let entries = [entry(&list, json!({ "task": task }), 0)];
    let open = section(&entries, 0, SECTION, &rules).open.unwrap();
    assert!(!open.contains("FOLLOW"), "the task's line is cut: {open}");
    assert!(!open.contains("teal-1234"), "{open}");
}

/// The answers' words are claude-mem's, with the repository's key.
#[test]
fn the_answers_say_what_claude_mem_says() {
    let open = [entry("release", json!({"task": "notes"}), 0)];
    assert_eq!(
        written("release", "github.com/o/r", &open, MIN, &Rules::default()),
        "Saved to \"release\" in github.com/o/r. Still open in it:\n- release\n  - notes, updated 1 minute ago"
    );
    let done = [entry("release", json!({"status": "done"}), 0)];
    assert_eq!(
        written("release", "github.com/o/r", &done, 0, &Rules::default()),
        "Saved to \"release\" in github.com/o/r. Nothing in it is open now."
    );
    assert_eq!(
        read(
            Some("release"),
            false,
            "github.com/o/r",
            &done,
            0,
            &Rules::default()
        ),
        "Nothing open in \"release\" for github.com/o/r. Pass includeClosed to see closed items."
    );
    assert_eq!(
        read(None, true, "github.com/o/r", &[], 0, &Rules::default()),
        "Nothing recorded for github.com/o/r."
    );
    assert_eq!(
        read(None, true, "github.com/o/r", &done, 0, &Rules::default()),
        "- release: status=done, updated 1 minute ago"
    );
    // A read is cut at `READ` with how many lines were left out (Codex's security review of
    // #408), where claude-mem's is not.
    let long: Vec<Entry> = (0..15)
        .map(|i| entry(&format!("l{i}"), json!({"note": "n".repeat(1_900)}), 0))
        .collect();
    let text = read(None, false, "github.com/o/r", &long, 0, &Rules::default());
    assert!(units(&text) <= READ, "{}", units(&text));
    assert!(text.ends_with("more lines; read them with work_state_read"));
}
