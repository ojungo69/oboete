use super::*;

/// claude-mem 13.35's `isRecursiveMemoryTool`: a read of stored memory keeps its call and not
/// its output; another server's tool of the same name keeps both.
#[test]
fn a_memory_read_is_stored_without_its_output() {
    let s = Settings::load(tempfile::tempdir().unwrap().path()).unwrap();
    let call = |tool: &str| {
        let payload = json!({"session_id": "s", "cwd": "/tmp", "tool_name": tool,
                "tool_input": {"query": "tabs"}, "tool_response": "Use tabs (decided)."});
        let c = &events("claude", "PostToolUse", &payload, 1, &s)[0];
        serde_json::from_str::<Value>(&c.event.body).unwrap()
    };
    let read = call("mcp__oboete__search");
    assert_eq!(
        (&read["output"], &read["memory_read"]),
        (&json!(""), &json!(true))
    );
    assert!(read["input"].as_str().unwrap().contains("tabs"));
    let other = call("mcp__notion__search");
    assert!(other["output"].as_str().unwrap().contains("Use tabs"));
    assert_eq!(other.get("memory_read"), None);
    // A note saved to memory keeps neither: it is kept once, as its document.
    let saved = call("mcp__oboete__save_memory");
    assert_eq!(
        (&saved["input"], &saved["output"], &saved["memory_write"]),
        (&json!(""), &json!(""), &json!(true))
    );
}

/// Slice 4a (A28): a checkout's `.oboete.toml` may only stop recording: its `capture` key alone
/// is read, and a file that cannot be read, is not TOML or holds another `capture` stops it
/// too, its text never in the error.
#[test]
fn a_checkouts_oboete_toml_may_only_stop_recording() {
    let top = tempfile::tempdir().unwrap();
    let file = top.path().join(".oboete.toml");
    let read = |text: Option<&str>| {
        let _ = std::fs::remove_file(&file);
        if let Some(text) = text {
            std::fs::write(&file, text).unwrap();
        }
        repo_capture(Some(top.path()))
    };
    assert!(repo_capture(None).unwrap());
    for (text, on) in [
        (None, true),
        (Some("capture = false\n"), false),
        (Some("capture = true\n"), true),
        (Some("[sync]\nother = 1\n"), true),
    ] {
        assert_eq!(read(text).unwrap(), on, "{text:?}");
    }
    for text in ["capture = \"no-4a\"\n", "capture = [\n"] {
        let e = read(Some(text)).unwrap_err().to_string();
        assert!(e.contains(".oboete.toml") && !e.contains("no-4a"), "{e}");
    }
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(repo_capture(Some(top.path())).is_err());
}

/// Security review of #489: why a directory is not recorded is asked as the hook asks it, so a
/// folder stored as given covers a directory below it under a rule that takes its separator.
#[test]
fn why_a_directory_is_not_recorded_reads_its_folder_as_the_hook_does() {
    let top = tempfile::tempdir().unwrap();
    let folder = top.path().join("private");
    let below = folder.join("sub");
    std::fs::create_dir_all(&below).unwrap();
    let mut list = crate::raw::CaptureExclusions::default();
    list.folders.insert(folder_path(&folder.to_string_lossy()));
    let settings = Settings {
        rules: redact::Rules::new(
            &crate::config::parse_capture(Some(
                "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
            ))
            .unwrap()
            .redaction,
        )
        .unwrap(),
        ..Settings::default()
    };
    let why = not_recorded_why(&below.to_string_lossy(), &list, &settings);
    assert!(why.is_some_and(|w| w.contains("excluded folder")));
}

/// Codex on #489: an undo finds a folder stored in another spelling of letter case where the
/// platform ignores it, as `covers` reads one, and only there; an add finds it the same way.
#[test]
fn an_undo_finds_a_folder_in_another_spelling_only_where_case_is_ignored() {
    let (stored, given) = if cfg!(windows) {
        (r"C:\Work\Private", r"c:\work\private")
    } else {
        ("/Users/x/Work/Private", "/users/x/work/private")
    };
    let list = std::collections::BTreeSet::from([stored.to_owned()]);
    let found = undone("cwd", given, &list, &Settings::default()).unwrap();
    let want = if cfg!(any(windows, target_os = "macos")) {
        stored
    } else {
        given
    };
    assert_eq!(found, want);
    // An exclusion added in another spelling keeps that one entry, as the CLI's add takes it.
    let kept = stored_entry("cwd", given, &list).map(String::as_str);
    assert_eq!(kept, (want == stored).then_some(stored));
    assert_eq!(stored_entry("repo", given, &list), None);
}

/// Push security review of #489: a checkout's `.oboete.toml` past the limit, or one that names
/// a device (a link the repository holds), stops recording at once rather than holding the
/// hook while it reads.
#[test]
fn a_checkout_toml_that_is_no_small_file_stops_recording() {
    let top = tempfile::tempdir().unwrap();
    let file = top.path().join(".oboete.toml");
    let big = format!(
        "capture = true\n#{}\n",
        "x".repeat(CHECKOUT_TOML_LIMIT as usize)
    );
    std::fs::write(&file, big).unwrap();
    let e = repo_capture(Some(top.path())).unwrap_err().to_string();
    assert!(e.contains("over"), "{e}");
    std::fs::remove_file(&file).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/dev/zero", &file).unwrap();
        let e = repo_capture(Some(top.path())).unwrap_err().to_string();
        assert!(e.contains("not a regular file"), "{e}");
        // A pipe no one writes to is opened without waiting, and refused (Codex on #489).
        std::fs::remove_file(&file).unwrap();
        let made = std::process::Command::new("mkfifo").arg(&file).status();
        assert!(made.unwrap().success());
        let e = repo_capture(Some(top.path())).unwrap_err().to_string();
        assert!(e.contains("not a regular file"), "{e}");
    }
}

/// Slice 4a: a checkout whose `.oboete.toml` stops recording, or cannot be read, gets no event,
/// whatever its kind; `capture = true` records as before.
#[test]
fn a_checkout_that_stops_recording_gets_no_event() {
    let top = tempfile::tempdir().unwrap();
    std::fs::create_dir(top.path().join(".git")).unwrap();
    let cwd = top.path().join("src");
    std::fs::create_dir(&cwd).unwrap();
    let payload = json!({"session_id": "s", "cwd": cwd, "prompt": "canary-4a"});
    let n = || {
        events(
            "claude",
            "UserPromptSubmit",
            &payload,
            7,
            &Settings::default(),
        )
        .len()
    };
    assert_eq!(n(), 1);
    for (text, want) in [
        ("capture = false\n", 0),
        ("capture =\n", 0),
        ("capture = true\n", 1),
    ] {
        std::fs::write(top.path().join(".oboete.toml"), text).unwrap();
        assert_eq!(n(), want, "{text}");
    }
}

/// Parity row 14: claude-mem's patterns name folders as its `project-filter.ts` reads them,
/// and a name with no `/` names the folders inside one so named too.
#[test]
fn a_pattern_names_folders_as_claude_mems_does() {
    for (pattern, cwd, named) in [
        ("secret", "/work/secret", true),
        ("secret", "/work/secret/src", true),
        ("secret", "/work/secrets", false),
        ("sec*", "/work/secret", true),
        ("sec?et", "/work/secret", true),
        ("sec?et", "/work/sec/et", false),
        ("/work/*/private", "/work/a/private", true),
        ("/work/*/private", "/work/a/b/private", false),
        ("/work/**/private", "/work/a/b/private", true),
        // A whole `**/` folder part also names no folder, as claude-mem 15.0 reads it.
        ("/work/**/private", "/work/private", true),
        ("/**/private", "/private", true),
        ("/work/**/private", "/work/privacy", false),
        ("/work/**", "/work", true),
        ("/work/**", "/work/a/b", true),
        ("/work/**", "/other", false),
        (r"\work\secret", "/work/secret/", true),
        ("/", "/", true),
        ("C:/", "C:/", true),
        ("/", "/work", false),
        ("C:/", "C:/work", false),
        ("  secret ", "/work/secret", true),
        ("", "/work/secret", false),
        // A pattern that does not start at a root names the end of a path (Codex on #563).
        ("secret/**", "/work/secret", true),
        ("secret/**", "/work/secret/src", true),
        ("secret/**", "/work/secrets", false),
        ("a/b/**", "/x/a/b", true),
        ("a/b/**", "/x/a/b/c", true),
        ("a/b", "/x/a/b", true),
        ("a/b", "/x/a/b/c", false),
        ("a/b", "/xa/b", false),
        ("*/private", "/work/a/private", true),
        ("/a/b", "/x/a/b", false),
        ("C:/a/b", "/x/C:/a/b", false),
        ("/server/share", "//server/share", false),
        ("/**", "C:/", false),
        ("/**", "C:/work", false),
    ] {
        assert_eq!(pattern_covers(pattern, cwd), named, "{pattern:?} {cwd}");
    }
    let home = crate::config::home_dir()
        .to_string_lossy()
        .replace('\\', "/");
    let home = home.trim_end_matches('/');
    assert!(pattern_covers("~/secret", &format!("{home}/secret")));
    assert!(pattern_covers("~/**", &format!("{home}/a/b")));
    assert!(!pattern_covers("~/secret", "/elsewhere/secret"));
    // A home that is a root: `~` names that root alone (Codex on #563).
    for (pattern, home, want) in [
        ("~", "/", "/"),
        ("~", "C:/", "C:/"),
        ("~", "/home/u/", "/home/u"),
        ("~/x", "/", "/x"),
        ("~/x", "C:/", "C:/x"),
        ("~x", "/home/u", "~x"),
    ] {
        assert_eq!(expanded(pattern.into(), home), want, "{pattern} {home}");
    }
    assert!(pattern("  a b ").is_ok_and(|p| p == "a b"));
    assert!(pattern(" ").is_err() && pattern("a\nb").is_err());
    assert!(pattern(&"a".repeat(256)).is_ok() && pattern(&"a".repeat(257)).is_err());
}

/// Codex on #489: a folder reads alike however it was written: `.` and `..` by name, a
/// trailing separator dropped, a relative path against the current directory.
#[test]
fn a_folder_reads_alike_however_it_was_written() {
    let here = std::env::current_dir().unwrap();
    let private = here
        .join("x")
        .join("private")
        .to_string_lossy()
        .into_owned();
    let sep = std::path::MAIN_SEPARATOR;
    for written in [
        here.join("x").join("open").join("..").join("private"),
        here.join("x").join(".").join("private"),
    ] {
        assert_eq!(folder_path(&written.to_string_lossy()), private);
    }
    assert_eq!(folder_path(&format!("{private}{sep}")), private);
    assert_eq!(folder_path("x/open/../private"), private);
}

/// Slice 4a: why a directory is not recorded, as doctor and the work state say it: its
/// checkout's `.oboete.toml`, its repository excluded, or a folder around it; nothing when it is
/// recorded.
#[test]
fn why_a_directory_is_not_recorded() {
    let top = tempfile::tempdir().unwrap();
    std::fs::create_dir(top.path().join(".git")).unwrap();
    let dir = top.path().join("deep");
    std::fs::create_dir(&dir).unwrap();
    let (dir, settings) = (dir.to_string_lossy().into_owned(), Settings::default());
    let mut list = crate::raw::CaptureExclusions::default();
    assert_eq!(not_recorded_why(&dir, &list, &settings), None);
    std::fs::write(top.path().join(".oboete.toml"), "capture = false\n").unwrap();
    let why = not_recorded_why(&dir, &list, &settings).unwrap();
    assert!(why.contains("capture = false"), "{why}");
    std::fs::remove_file(top.path().join(".oboete.toml")).unwrap();
    list.folders
        .insert(top.path().to_string_lossy().into_owned());
    let why = not_recorded_why(&dir, &list, &settings).unwrap();
    assert!(why.contains("excluded folder"), "{why}");
    list.folders.clear();
    list.patterns.insert("deep".into());
    let why = not_recorded_why(&dir, &list, &settings).unwrap();
    assert!(why.contains("excluded pattern"), "{why}");
    list.patterns.clear();
    list.repos.insert(repo::key(top.path()));
    let why = not_recorded_why(&dir, &list, &settings).unwrap();
    assert!(why.contains("its repository"), "{why}");
}

/// Slice 4a: an exclusion's values as events labelled now hold them, through today's rules: a
/// rule added after a folder was excluded masks it as it masks the cwd of an event in it.
#[test]
fn exclusions_are_labelled_as_events_are_now() {
    let home = tempfile::tempdir().unwrap();
    let folder = "/w/acme-482915/private";
    let mut list = crate::raw::CaptureExclusions::default();
    list.folders.insert(folder.into());
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let settings = Settings::load(home.path()).unwrap();
    let cwd = exclusion_label("cwd", &format!("{folder}/sub"), &settings);
    assert!(!cwd.contains("acme-482915"));
    assert!(!list.covers(None, Some(&cwd)));
    assert!(never_recorded(&list, &settings).covers(None, Some(&cwd)));
}

fn one(event: &str, payload: Value) -> Event {
    let mut v = events("claude", event, &payload, 7, &Settings::default());
    assert_eq!(v.len(), 1, "{event}: {v:?}");
    v.remove(0).event
}

fn body(e: &Event) -> Value {
    serde_json::from_str(&e.body).unwrap()
}

/// curl's flag, written apart from the word in the lines below: a scanner that reads this
/// file takes `curl`, the flag and a made-up `user:{pw}` on one line for a credential.
const U: &str = "-u";

/// #537 where a text is stored: the credential before a value curl-auth-user passes over is
/// masked in a tool's input and output and in a prompt, and the rest of the field stays. With
/// words after the passed-over value this was stored as written (at a field's end the quote
/// that closes it keeps the rule from that value, and it found the credential before).
#[test]
fn a_credential_before_a_value_a_greedy_rule_passes_over_is_masked_where_it_is_stored() {
    // Since #985 curl-user reads a credential wherever it stands; what #537's looks still add
    // is a value gitleaks' rule takes and curl-user passes over, as a password that starts
    // with the placeholder curl-user knows. The made-up value here is one.
    let pw = format!("パスワード{}", "N3kL7pW2");
    for last in ["bob:pass", "a:b"] {
        for tail in [" && echo ok", ""] {
            let line = format!("curl {U} admin:{pw} https://h.test {U} {last}{tail}");
            let want = format!("curl -u [REDACTED] https://h.test -u {last}{tail}");
            let e = one(
                "PostToolUse",
                json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                           "tool_input": {"command": line},
                           "tool_response": {"stdout": format!("ran {line}\n")}}),
            );
            let b = body(&e);
            assert_eq!(b["input"], json!({"command": want}).to_string());
            assert_eq!(
                b["output"],
                json!({"stdout": format!("ran {want}\n")}).to_string()
            );
            let e = one(
                "UserPromptSubmit",
                json!({"session_id": "s", "cwd": "/", "prompt": line}),
            );
            assert_eq!(body(&e)["prompt"], want);
        }
    }
    // A short field is scanned alone and beside its key, and masked whole where the two scans
    // disagree, as before. With a credential that ends the field (in the pair the closing
    // quote follows it, and the rule does not take it) and one before a passed-over value,
    // they do; two credentials at a field's end did so before #537 too.
    for line in [
        format!("curl {U} admin:{pw} {U} bob:pass {U} ops:{pw}9"),
        format!("curl {U} admin:{pw} {U} ops:{pw}9"),
    ] {
        let e = one(
            "UserPromptSubmit",
            json!({"session_id": "s", "cwd": "/", "prompt": line}),
        );
        assert_eq!(body(&e)["prompt"], "[REDACTED]", "{line}");
    }
}

/// #985: capture's own path stores a credential given to one of curl's user options masked,
/// whatever follows it and whatever it holds: as a tool's input (alone, where the closing
/// quote of the flattened field follows what ends the command, and beside another key), in
/// its output, in a prompt (a short one is scanned alone and beside its key, and the two
/// scans agree).
#[test]
fn a_credential_given_to_curls_user_option_is_masked_where_it_is_stored() {
    let m = format!("N3kL{}", "7pW2"); // split: scanners
    let pw = format!("Zq8v{m}");
    let curl = format!("curl -s https://h.test {U} ");
    for (before, credential, after) in [
        (curl.clone(), format!("admin:{pw}"), ""),
        (curl.clone(), format!("'admin:{pw}'"), ""),
        (curl.clone(), format!("\"admin:{pw}\""), ""),
        (format!("x=$({curl}"), format!("admin:{pw}"), ") && echo $x"),
        (curl.clone(), format!("admin:{pw}"), "; echo ok"),
        (curl.clone(), format!("admin:Zq8v+{m}/="), " && echo ok"),
        (curl.clone(), format!("admin:x{pw}"), " && echo ok"),
        (
            "curl -sSLu ".to_owned(),
            format!("admin:{pw}"),
            " https://h.test && echo ok",
        ),
        (
            format!("curl -s https://h.test {U}"),
            format!("'admin:{pw}'"),
            "",
        ),
        (
            "curl -x http://p.test:3128 --proxy-user ".to_owned(),
            format!("admin:{pw}"),
            " https://h.test",
        ),
        (
            format!("curl https://h.test/v1/x {U} "),
            format!("Zq8v{m}abcd:"),
            "",
        ),
    ] {
        let line = format!("{before}{credential}{after}");
        let want = format!("{before}[REDACTED]{after}");
        let e = one(
            "PostToolUse",
            json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                       "tool_input": {"command": line},
                       "tool_response": {"stdout": format!("+ {line}\n")}}),
        );
        let b = body(&e);
        assert_eq!(b["input"], json!({"command": want}).to_string(), "{line}");
        assert_eq!(
            b["output"],
            json!({"stdout": format!("+ {want}\n")}).to_string(),
            "{line}"
        );
        let e = one(
            "PostToolUse",
            json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                       "tool_input": {"command": line, "description": "Calls: it"},
                       "tool_response": {"stdout": "ok"}}),
        );
        assert_eq!(
            body(&e)["input"],
            json!({"command": want, "description": "Calls: it"}).to_string(),
            "{line}"
        );
        for lead in [
            "",
            &"this is the one that works for me, word for word: ".repeat(6),
        ] {
            let e = one(
                "UserPromptSubmit",
                json!({"session_id": "s", "cwd": "/", "prompt": format!("{lead}{line}")}),
            );
            assert_eq!(body(&e)["prompt"], format!("{lead}{want}"), "{line}");
        }
    }
}

#[test]
fn replay_can_measure_a_cap_candidate_but_not_outside_their_range() {
    assert_eq!(field_cap(None), MAX_FIELD_BYTES);
    assert_eq!(field_cap(Some("262144")), 256 * 1024);
    assert_eq!(field_cap(Some("131072")), 128 * 1024);
    for v in ["262145", "1023", "0", "-1", "lots"] {
        assert_eq!(field_cap(Some(v)), MAX_FIELD_BYTES, "{v}");
    }
}

#[test]
fn a_long_tool_output_is_kept_whole_and_redacted_past_the_old_window() {
    let key = &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split, as in import.rs, so secret scanners pass it
    // 50 KB: past v1's 12,000-character window, within `MAX_FIELD_BYTES`.
    let out = "word ".repeat(10_000) + " Authorization: Bearer " + key;
    let e = one(
        "PostToolUse",
        json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                   "tool_input": {"command": "ls"}, "tool_response": {"stdout": out}}),
    );
    assert_eq!(e.kind, "tool");
    let b = body(&e);
    assert!(
        b["output"]
            .as_str()
            .unwrap()
            .contains(&"word ".repeat(10_000))
    );
    assert!(
        !e.body.contains(key),
        "a secret past today's 12,000-character window"
    );
    assert_eq!(b["input"], r#"{"command":"ls"}"#);
    assert_eq!(b["failed"], false);
    assert_eq!(
        (e.session.as_str(), e.ts, e.source.as_str()),
        ("s", 7, "hook")
    );
}

#[test]
fn prompts_lose_private_blocks_and_envelopes_are_not_typed_prompts() {
    let e = one(
        "UserPromptSubmit",
        json!({"prompt": "keep <private>secret plan</private> this"}),
    );
    assert_eq!(
        (e.kind.as_str(), body(&e)["prompt"].as_str()),
        ("prompt", Some("keep  this"))
    );
    let e = one(
        "UserPromptSubmit",
        json!({"prompt": "shown <private>and the rest is hidden"}),
    );
    assert_eq!(body(&e)["prompt"], "shown");
    assert!(
        events(
            "claude",
            "UserPromptSubmit",
            &json!({"prompt": "<private>x"}),
            0,
            &Settings::default()
        )
        .is_empty()
    );
    for envelope in [
        "<task-notification>done</task-notification>",
        "Another Claude session sent a message: <teammate-message from=\"a\">hi</teammate-message>",
    ] {
        let e = one("UserPromptSubmit", json!({"prompt": envelope}));
        assert_eq!(e.kind, "envelope", "{envelope}");
    }
    assert_eq!(
        one("UserPromptSubmit", json!({"prompt": "hi"})).session,
        "unknown"
    );
}

#[test]
fn a_block_never_pairs_across_two_fields() {
    let out = json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "<private>gone</private>!"});
    let e = one(
        "PostToolUse",
        json!({"tool_name": "t", "tool_input": {}, "tool_response": out}),
    );
    let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        stored,
        json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "!"})
    );
}

#[test]
fn the_ledger_names_the_field_and_the_mask_never_the_value() {
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {"command": format!("curl -H 'Authorization: Bearer {key}'")},
            "tool_response": {"stdout": "ok"}, "reason": {format!("Bearer {key}"): 1}});
    let v = events("claude", "PostToolUse", &payload, 0, &Settings::default());
    let c = &v[0];
    assert!(!format!("{c:?}").contains(&key), "{c:?}");
    let (field, f) = &c.ledger[0];
    assert_eq!((field.as_str(), f.length), ("/input", key.len()));
    let input = body(&c.event)["input"].as_str().unwrap().to_owned();
    assert_eq!(
        &input[f.offset..f.offset + "[REDACTED]".len()],
        "[REDACTED]"
    );
    // A key names its object, from the stored key: the pointer never holds the secret.
    let end = events(
        "claude",
        "SessionEnd",
        &json!({"reason": {format!("Bearer {key}"): 1}}),
        0,
        &Settings::default(),
    );
    assert!(
        end[0]
            .ledger
            .iter()
            .any(|(field, _)| field == "/reason#key"),
        "{:?}",
        end[0].ledger
    );
    assert!(!format!("{:?}", end[0]).contains(&key));
}

/// Codex's security review of #408: a rule that looks for a key before a secret (gitleaks'
/// generic-api-key) matches a string beside its key, the ledger offset in the stored string.
#[test]
fn a_value_is_scanned_with_its_key() {
    let secret = format!("R8m2V5p9{}", "Q1s4H7c0N6x3");
    let payload = json!({"reason": {"api_key": format!("  {secret}"), "note": "violet"}});
    let v = events("claude", "SessionEnd", &payload, 0, &Settings::default());
    assert!(!format!("{:?}", v[0]).contains(&secret), "{:?}", v[0]);
    let (field, f) = &v[0].ledger[0];
    assert_eq!((field.as_str(), f.offset), ("/reason/api_key", 2));
    assert_eq!(body(&v[0].event)["reason"]["api_key"], "  [REDACTED]");
    assert_eq!(body(&v[0].event)["reason"]["note"], "violet");
    // A mask that reaches past the value masks the value whole, and one that reaches into the
    // key the key too, its ledger pointer with it.
    let s = with("[[redaction.extra_rules]]\nid = \"v\"\nregex = 'note = \"violet\"'\n");
    let v = events("claude", "SessionEnd", &payload, 0, &s);
    assert!(body(&v[0].event)["reason"].get("note").is_none());
    assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
    assert!(
        v[0].ledger
            .iter()
            .any(|(field, f)| field == "/reason/[REDACTED]" && f.offset == 0)
    );
    // Each scan looks at the value as written: a rule anchored to the whole value still finds
    // what follows the secret that generic-api-key masks beside its key, and the value two
    // different masks hide is hidden whole.
    let s = with(
        "[[redaction.extra_rules]]\nid = \"t\"\nregex = '^[A-Za-z0-9]{20} (teal-[0-9]{4})$'\n\
             secret_group = 1\n",
    );
    let payload = json!({"reason": {"api_key": format!("{secret} teal-1234")}});
    let v = events("claude", "SessionEnd", &payload, 0, &s);
    assert_eq!(body(&v[0].event)["reason"]["api_key"], "[REDACTED]");
    assert!(v[0].ledger.len() >= 2 && v[0].ledger.iter().all(|(_, f)| f.offset == 0));
    // A mask the pair puts on the key hides the key, its pointer in the ledger too.
    let s = with(
        "[[redaction.extra_rules]]\nid = \"p\"\nregex = '^(teal-[0-9]{4}) = \"private\"$'\n\
             secret_group = 1\n",
    );
    let payload = json!({"reason": {"teal-1234": "private"}});
    let v = events("claude", "SessionEnd", &payload, 0, &s);
    assert!(!format!("{:?}", v[0]).contains("teal-1234"), "{:?}", v[0]);
    assert!(body(&v[0].event)["reason"].get("[REDACTED]").is_some());
    // Where the mask starts tells whether it reaches the key: the masked pair can spell the
    // key again when the key holds the mask's own text (Codex's security review of #408).
    let s = with(
        "[[redaction.extra_rules]]\nid = \"s\"\n\
             regex = '^(\\[REDACTED\\]teal-[0-9]{4} = \")teal-[0-9]{4} = \"private\"$'\n\
             secret_group = 1\n",
    );
    let payload = json!({"reason": {"[REDACTED]teal-1234": "teal-1234 = \"private"}});
    let v = events("claude", "SessionEnd", &payload, 0, &s);
    assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
    // A rule that masks the key leaves the value's scan beside the key as written.
    let s = with("[[redaction.extra_rules]]\nid = \"k\"\nregex = '^api_key$'\n");
    let payload = json!({"reason": {"api_key": secret.clone()}});
    let v = events("claude", "SessionEnd", &payload, 0, &s);
    assert!(!format!("{:?}", v[0]).contains(&secret), "{:?}", v[0]);
    assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
}

/// Codex's security review of #408: a number or a boolean is gated as the text it shows as,
/// and kept as it is where the gate leaves that text.
#[test]
fn a_work_state_number_is_gated_as_its_text() {
    let fields = json!({"authorization": 7_319_462_805u64, "pr": 7, "draft": true});
    let (_, stored) = work_state("l", fields.as_object().unwrap(), &Settings::default());
    let stored = Value::Object(stored);
    assert!(!stored.to_string().contains("7319462805"), "{stored}");
    assert_eq!((&stored["pr"], &stored["draft"]), (&json!(7), &json!(true)));
}

/// Codex on #408: claude-mem's keys stay where a rule matches them, their values gated, and a
/// closing status stays where a rule matches the word.
#[test]
fn a_work_state_write_keeps_task_and_status() {
    let s = with(
        "[[redaction.extra_rules]]\nid = \"k\"\nregex = '^(?:task|status|note)$'\n\n\
             [[redaction.extra_rules]]\nid = \"v\"\nregex = 'teal-[0-9]{4}|(?i)done|wip'\n",
    );
    let fields = json!({"task": "teal-1234", "status": "Done", "note": "x"});
    let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
    assert_eq!(
        Value::Object(stored),
        json!({"task": "[REDACTED]", "status": "Done", "[REDACTED]": "x"})
    );
    let open = json!({"task": "t", "status": "wip"});
    let (_, stored) = work_state("l", open.as_object().unwrap(), &s);
    assert_eq!(stored["status"], "[REDACTED]");
}

/// A list's and a task's name are stored settled, as the fold reads them (`kept_beside`), so a
/// write and its stored form fold as one whatever the passes (Sourcery on #585).
#[test]
fn a_work_state_name_is_stored_settled() {
    for key in ["list", "task"] {
        let s = with(&format!(
            "[[redaction.extra_rules]]\nid = \"a\"\nregex = '^\\[REDACTED\\] (teal-[0-9]{{4}})'\n\
                 secret_group = 1\n\n[[redaction.extra_rules]]\nid = \"k\"\n\
                 regex = '{key} = \"(alpha)'\nsecret_group = 1\n"
        ));
        let fields = json!({"task": "alpha teal-1234"});
        let (list, stored) = work_state("alpha teal-1234", fields.as_object().unwrap(), &s);
        let name = if key == "list" {
            list
        } else {
            stored["task"].as_str().unwrap().to_owned()
        };
        assert_eq!(name, "[REDACTED] [REDACTED]", "{key}");
        assert_eq!(kept_beside(key, &name, &s.rules), name, "{key}");
    }
}

/// A number or a boolean names a task as the text it shows as: stored as that text settles, as
/// a string task is, and kept as it is where the gate leaves it (Sourcery on #589).
#[test]
fn a_numeric_or_boolean_task_is_stored_settled() {
    for (task, head, tail) in [(json!(1234), "12", "34"), (json!(true), "tr", "ue")] {
        let s = with(&format!(
            "[[redaction.extra_rules]]\nid = \"a\"\nregex = '^\\[REDACTED\\]({tail})'\n\
                 secret_group = 1\n\n[[redaction.extra_rules]]\nid = \"k\"\n\
                 regex = 'task = \"({head})'\nsecret_group = 1\n"
        ));
        let fields = json!({ "task": task });
        let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
        assert_eq!(stored["task"], "[REDACTED][REDACTED]", "{task}");
        assert_eq!(
            kept_beside("task", "[REDACTED][REDACTED]", &s.rules),
            "[REDACTED][REDACTED]"
        );
        let (_, kept) = work_state("l", fields.as_object().unwrap(), &with(""));
        assert_eq!(kept["task"], task);
    }
}

/// A number a rule finds in its JSON is stored as the text it shows as, the text the gate saw,
/// a task's and any field's, so no digit the gate masks is kept (#590's review); every other
/// number is kept as the tool call sent it, which My Work ties the write by (Codex on #593).
#[test]
fn a_number_is_stored_as_sent_unless_a_rule_finds_its_digits() {
    let s = with("[[redaction.extra_rules]]\nid = \"n\"\nregex = '1234567890123456789'\n");
    let fields = json!({"task": 1_234_567_890_123_456_789_u64, "n": 1_234_567_890_123_456_789_u64});
    let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
    let op = serde_json::to_string(&stored).unwrap();
    assert!(!op.contains("1234567890123456789"), "{op}");
    assert_eq!(stored["task"], "1234567890123456800");
    assert_eq!(stored["n"], "1234567890123456800");
    let sent = json!({"task": 100.0, "w": 100.0, "big": 9_007_199_254_740_993_u64,
            "e": 1e21, "m": 5, "f": 0.5, "b": false});
    let (_, stored) = work_state("l", sent.as_object().unwrap(), &s);
    assert_eq!(&Value::Object(stored), &sent);
}

/// A list's name is gated beside its key `list`, as a field's value is beside its own: a rule
/// that needs the key masks it in the op.
#[test]
fn a_work_state_list_name_is_gated_beside_its_key() {
    let s = with("[[redaction.extra_rules]]\nid = \"l\"\nregex = 'list = \"canary-[0-9]{4}'\n");
    let fields = json!({"task": "rotate"});
    let (list, _) = work_state("canary-4827", fields.as_object().unwrap(), &s);
    assert_eq!(list, crate::redact::MASK);
}

#[test]
fn secrets_behind_json_escapes_in_tool_fields_are_masked() {
    // A tool field is stored as flattened JSON: its quotes and line breaks become `\"`, `\n`.
    let b64 = format!("dXNyOnE5Wng4{}", "bUwydkI0blI3dFl3");
    let weak = format!("MTExMTExMTE6{}", "MTExMTExMTE="); // 11111111:11111111
    let short = format!("YT{}", "pi"); // a:b
    let pass = format!("q9Zx8mL2{}", "vB4nR7tY1wK3");
    let api = format!("k7Fq2Lp9{}", "Xw3Rt8Vn5Bz1Mc6D");
    let cases = [
        // Bash stdout holding JSON (an API's answer, a config file)
        (
            json!({"command": "cat cfg.json"}),
            json!({"stdout": format!("{{\"Authorization\":\"Basic {b64}\"}}"), "stderr": ""}),
            &b64,
        ),
        // A weak credential after "Authorization: Basic" is still one
        (
            json!({"command": "cat headers.txt"}),
            json!({"stdout": format!("Authorization: Basic {weak}\n")}),
            &weak,
        ),
        (
            json!({"command": "cat headers.txt"}),
            json!({"stdout": format!("Authorization: Basic {short}\n")}),
            &short,
        ),
        // curl's -u and -H in double quotes
        (
            json!({"command": format!("curl -u \"admin:{pass}\" https://x.invalid/")}),
            json!({"stdout": "ok"}),
            &pass,
        ),
        (
            json!({"command": format!("curl -H \"X-Api-Key: {api}\" https://x.invalid/")}),
            json!({"stdout": "ok"}),
            &api,
        ),
        // A key and its value side by side: why tool fields stay flattened
        (
            json!({"api_key": api.clone(), "q": "x"}),
            json!({"stdout": "ok"}),
            &api,
        ),
    ];
    for (input, output, secret) in cases {
        let payload = json!({"session_id": "s", "tool_name": "Bash",
                "tool_input": input, "tool_response": output});
        let c = &events("claude", "PostToolUse", &payload, 0, &Settings::default())[0];
        assert!(
            !format!("{c:?}").contains(secret.as_str()),
            "{}",
            c.event.body
        );
        assert!(!c.ledger.is_empty());
        let b = body(&c.event);
        for (field, f) in &c.ledger {
            let text = b[&field[1..]].as_str().unwrap();
            assert_eq!(&text[f.offset..f.offset + "[REDACTED]".len()], "[REDACTED]");
        }
    }
}

#[test]
fn a_huge_key_is_named_by_its_hash_in_the_ledger() {
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let big = "k".repeat(250_000);
    let inner: Map<String, Value> = (0..50).map(|i| (format!("n{i}"), json!(key))).collect();
    let e = &events(
        "claude",
        "SessionEnd",
        &json!({"reason": {big.clone(): inner}}),
        0,
        &Settings::default(),
    )[0];
    assert_eq!(e.ledger.len(), 50);
    for (field, _) in &e.ledger {
        assert!(
            field.starts_with("/reason/~sha:") && field.len() < 64,
            "{}",
            &field[..60.min(field.len())]
        );
    }
    assert_eq!(segment("a/b~c"), "a~1b~0c");
}

fn with(toml: &str) -> Settings {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), toml).unwrap();
    Settings::load(home.path()).unwrap()
}

#[test]
fn with_prompts_off_the_turn_is_recorded_without_its_text() {
    let s = with("[capture]\nstore_prompts = false\n");
    let payload = json!({"session_id": "s", "prompt": "the zebra plan"});
    let v = events("claude", "UserPromptSubmit", &payload, 0, &s);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].event.kind, "prompt");
    assert_eq!(body(&v[0].event), json!({"omitted": true}));
    assert!(!format!("{:?}", v[0]).contains("zebra"));
    // A turn that was all private is none, as with prompts on.
    let private = json!({"session_id": "s", "prompt": "<private>zebra</private>"});
    assert!(events("claude", "UserPromptSubmit", &private, 0, &s).is_empty());
    let on = events(
        "claude",
        "UserPromptSubmit",
        &private,
        0,
        &Settings::default(),
    );
    assert!(on.is_empty());
}

/// docs/session-start.md: Codex's review template is stored as a program's prompt, from any
/// agent and with prompts kept or not; the owner's own review request is not marked.
#[test]
fn a_codex_review_template_is_stored_as_a_programs_prompt() {
    let template = "Review the code changes introduced by commit 1a2b3c4d. Provide \
                        prioritized, actionable findings.";
    let off = with("[capture]\nstore_prompts = false\n");
    for (agent, s) in [("codex", &Settings::default()), ("claude", &off)] {
        let payload = json!({"session_id": "s", "prompt": template});
        let v = events(agent, "UserPromptSubmit", &payload, 0, s);
        assert_eq!(body(&v[0].event)["agent_sent"], true, "{agent}");
        let own = json!({"session_id": "s", "prompt": "Review the parser changes."});
        let v = events(agent, "UserPromptSubmit", &own, 0, s);
        assert_eq!(body(&v[0].event)["agent_sent"], Value::Null, "{agent}");
    }
}

#[test]
fn head_tail_cuts_a_tool_output_and_leaves_its_input_whole() {
    let s = with("[capture]\ntool_output = \"head-tail\"\n");
    let big = "a".repeat(HEAD_TAIL_BYTES) + &"b".repeat(HEAD_TAIL_BYTES);
    let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {"cmd": big}, "tool_response": big});
    let e = &events("claude", "PostToolUse", &payload, 0, &s)[0].event;
    let b = body(e);
    let out = b["output"].as_str().unwrap();
    assert!(out.len() < HEAD_TAIL_BYTES + 100, "{}", out.len());
    assert!(out.starts_with('a') && out.ends_with('b') && out.contains("[cut: "));
    assert!(b["input"].as_str().unwrap().contains(&big));
}

#[test]
fn head_tail_holds_when_a_rule_masks_the_key_output() {
    let s = with(
        "[capture]\ntool_output = \"head-tail\"\n[[redaction.extra_rules]]\nid = \"o\"\nregex = \"output\"\n",
    );
    let big = "a".repeat(HEAD_TAIL_BYTES) + &"b".repeat(HEAD_TAIL_BYTES);
    let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {}, "tool_response": big});
    let b = body(&events("claude", "PostToolUse", &payload, 0, &s)[0].event);
    let (key, out) = b
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v.as_str().is_some_and(|v| v.starts_with('a')))
        .unwrap();
    assert_ne!(key, "output");
    assert!(out.as_str().unwrap().len() < HEAD_TAIL_BYTES + 100);
}

#[test]
fn an_oversized_field_keeps_head_and_tail_and_its_full_size() {
    let out = "a".repeat(MAX_FIELD_BYTES) + &"b".repeat(MAX_FIELD_BYTES);
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {}, "tool_response": out}),
    );
    let stored = body(&e)["output"].as_str().unwrap().to_owned();
    assert!(stored.len() < MAX_FIELD_BYTES + 100, "{}", stored.len());
    assert!(stored.starts_with('a') && stored.ends_with('b'));
    assert!(stored.contains(&format!("[cut: {} bytes in full]", out.len())));
    assert_eq!(e.original_bytes, Some(out.len() as i64));
    let small = one(
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {}, "tool_response": "x"}),
    );
    assert_eq!(small.original_bytes, None);
}

#[test]
fn a_tool_call_keeps_its_subagent_and_interruption() {
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {}, "tool_response": null,
                   "agent_id": "a1", "interrupted": true}),
    );
    assert_eq!(
        (&body(&e)["agent_id"], &body(&e)["interrupted"]),
        (&json!("a1"), &json!(true))
    );
    let plain = one(
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {}}),
    );
    assert!(body(&plain).get("agent_id").is_none() && body(&plain).get("interrupted").is_none());
}

/// Slice 2 (docs/subagents.md): a subagent's call keeps its type beside its id, bounded and
/// cleaned as its stop's; an `Agent` or `Task` call keeps what it launched beside its text, whole
/// when that text is cut under either cut, and an owner's stop says it is the owner's.
#[test]
fn a_subagents_call_keeps_its_type_and_a_launch_what_it_started() {
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Read", "tool_input": {}, "agent_id": "a1",
                   "agent_type": "Explore"}),
    );
    assert_eq!(body(&e)["agent_type"], "Explore");
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Read", "tool_input": {}, "agent_id": "a1",
                   "agent_type": "t".repeat(129)}),
    );
    assert!(body(&e).get("agent_type").is_none());
    // The response as Claude Code sends it, its prompt long enough to be cut under both cuts.
    let long = "p ".repeat(MAX_FIELD_BYTES);
    let call = |response: Value, input: Value| {
        json!({"session_id": "s", "tool_name": "Agent", "tool_input": input,
                   "tool_response": response})
    };
    let response = json!({"agentId": "a9", "status": "completed", "isAsync": false,
                              "prompt": long, "description": "Count the files"});
    let input = json!({"description": "  Count\n the   files ", "prompt": long,
                           "subagent_type": "Explore"});
    for settings in [
        Settings::default(),
        with("[capture]\ntool_output = \"head-tail\"\n"),
    ] {
        let payload = call(response.clone(), input.clone());
        let b = body(&events("claude", "PostToolUse", &payload, 0, &settings)[0].event);
        // No task: My Work reads it from the input, through the rules of now before its cut.
        assert_eq!(
            b["launched"],
            json!({"agent_id": "a9", "async": false, "status": "completed"})
        );
    }
    // The response as text, async.
    let text = json!({"agentId": "a8", "status": "async_launched", "prompt": "x".repeat(600)});
    let e = one("PostToolUse", call(json!(text.to_string()), json!({})));
    assert_eq!(
        body(&e)["launched"],
        json!({"agent_id": "a8", "async": true, "status": "async_launched"})
    );
    // No id, or another tool, launches nothing.
    let e = one(
        "PostToolUse",
        call(json!({"status": "completed"}), json!({})),
    );
    assert!(body(&e).get("launched").is_none());
    let mut bash = call(response.clone(), input.clone());
    bash["tool_name"] = json!("Bash");
    assert!(body(&one("PostToolUse", bash)).get("launched").is_none());
    // The viewer's Mark finished, through the hook's own path.
    let mut stop = json!({"session_id": "s", "agent_id": "a9"});
    assert!(
        body(&one("SubagentStop", stop.clone()))
            .get("marked")
            .is_none()
    );
    stop[OWNER_MARK] = json!(true);
    assert_eq!(body(&one("SubagentStop", stop))["marked"], true);
}

#[test]
fn stored_text_keeps_its_whitespace() {
    let out = "\n    indented line\n\t";
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Read", "tool_input": " a ", "tool_response": out}),
    );
    assert_eq!(
        (body(&e)["output"].as_str(), body(&e)["input"].as_str()),
        (Some(out), Some(" a "))
    );
    let e = one(
        "Stop",
        json!({"last_assistant_message": "  code:\n    x = 1\n"}),
    );
    assert_eq!(body(&e)["assistant"], "  code:\n    x = 1\n");
}

#[test]
fn private_blocks_leave_every_text_field() {
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "echo <private>zqx-in</private>"},
                   "tool_response": {"stdout": "a <private>zqx-out</private> b"}}),
    );
    assert!(
        !e.body.contains("zqx") && !e.body.contains("private"),
        "{}",
        e.body
    );
    let e = one(
        "Stop",
        json!({"last_assistant_message": "ok <private>reply</private>"}),
    );
    assert_eq!(body(&e)["assistant"], "ok "); // whitespace is kept, the block is not
    let e = one(
        "PostCompact",
        json!({"compact_summary": "sum <private>mary</private>"}),
    );
    assert_eq!(body(&e)["summary"], "sum ");
    let only_private = json!({"last_assistant_message": "<private>all</private>"});
    assert!(events("claude", "Stop", &only_private, 0, &Settings::default()).is_empty());
}

#[test]
fn every_string_in_the_body_passes_the_gate() {
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let bearer = format!("Authorization: Bearer {token} <private>zqx-opt-out</private>");
    for (event, payload) in [
        (
            "PostToolUse",
            json!({"tool_name": bearer, "tool_input": {}, "tool_response": ""}),
        ),
        ("SessionStart", json!({"source": bearer})),
        ("PreCompact", json!({"trigger": {"nested": [bearer]}})),
        ("SessionEnd", json!({"reason": bearer})),
        ("UserPromptSubmit", json!({"prompt": bearer})),
        ("Stop", json!({"last_assistant_message": bearer})),
        ("PostCompact", json!({"compact_summary": bearer})),
    ] {
        let e = one(event, payload);
        assert!(
            !e.body.contains(&token) && e.body.contains("[REDACTED]") && !e.body.contains("zqx"),
            "{event}: {}",
            e.body
        );
    }
}

#[test]
fn labels_are_redacted_too() {
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join(format!("Bearer {token}"));
    std::fs::create_dir(&cwd).unwrap();
    let e = one(
        "SessionStart",
        json!({"session_id": format!("Authorization: Bearer {token}"), "cwd": cwd}),
    );
    for field in [Some(&e.session), e.cwd.as_ref(), e.repo.as_ref()] {
        let field = field.unwrap();
        assert!(
            !field.contains(&token) && field.contains("[REDACTED]"),
            "{field}"
        );
    }
}

#[test]
fn images_become_markers_in_each_shape_seen() {
    let data = "iVBORw0KGgo=";
    let out = json!([
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
        {"type": "image/png", "base64": data, "dimensions": {}},
        {"type": "image", "mimeType": "image/jpeg", "data": data},
        {"type": "text", "text": "caption"},
    ]);
    let e = one(
        "PostToolUse",
        json!({"tool_name": "Read", "tool_input": {}, "tool_response": out}),
    );
    let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
    assert!(!e.body.contains(data));
    for (i, mime) in ["image/png", "image/png", "image/jpeg"].iter().enumerate() {
        assert_eq!(stored[i]["kind"], "image");
        assert_eq!(stored[i]["mime"], *mime);
        assert_eq!(stored[i]["bytes"], 8);
        assert_eq!(stored[i]["sha256"].as_str().unwrap().len(), 64);
    }
    assert_eq!(stored[3]["text"], "caption");
    let tagged =
        json!({"type": "image", "mimeType": "image/<private>zqx</private>png", "data": data});
    let e = one(
        "PostToolUse",
        json!({"tool_name": "t", "tool_input": {}, "tool_response": tagged}),
    );
    assert!(!e.body.contains("zqx"), "{}", e.body);
}

#[test]
fn long_base64_in_any_shape_becomes_a_marker() {
    let blob = "iVBORw0KGgoAAAANSUhEUg".repeat(50); // 1,100 characters
    let e = one(
        "PostToolUse",
        json!({"tool_name": "mcp__x", "tool_input": {},
                   "tool_response": {"encoding": "base64", "content": blob, "note": format!("saved {blob}.")}}),
    );
    assert!(!e.body.contains("SUhEUgiVBOR"), "{}", e.body);
    let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
    assert_eq!(stored["content"]["kind"], "binary");
    assert_eq!(stored["content"]["bytes"], 825);
    assert!(
        stored["note"]
            .as_str()
            .unwrap()
            .starts_with("saved {\"kind\":\"binary\"")
    );
    // Text that only looks like it stays: short runs (a hash, a key id), runs missing a character
    // class (a long lowercase or hex run), and a length base64 cannot have (1,025).
    for kept in [
        "a1B2".repeat(64),
        "x".repeat(2_000),
        "0f".repeat(600),
        "a1B".repeat(341) + "cd",
    ] {
        let e = one(
            "PostToolUse",
            json!({"tool_name": "x", "tool_input": {}, "tool_response": kept}),
        );
        assert!(e.body.contains(&kept), "{}", kept.len());
    }
    // Prompts, replies and summaries get the same pass as tool fields.
    let uri = "data:image/png;base64,iVBORw0KGgo=";
    for (event, payload) in [
        ("UserPromptSubmit", json!({"prompt": format!("see {uri}")})),
        ("Stop", json!({"last_assistant_message": blob})),
        (
            "PostCompact",
            json!({"compact_summary": format!("{uri} and {blob}")}),
        ),
    ] {
        let e = one(event, payload);
        assert!(!e.body.contains("iVBORw0KGgo"), "{event}: {}", e.body);
    }
}

#[test]
fn data_uris_become_markers_whole_or_inside_text() {
    let uri = "data:image/png;base64,iVBORw0KGgo=";
    let out = json!([
        {"type": "input_image", "image_url": uri},
        {"type": "image_url", "image_url": {"url": uri}},
        format!("<img src=\"{uri}\"> after"),
    ]);
    let e = one(
        "PostToolUse",
        json!({"tool_name": "view_image", "tool_input": {}, "tool_response": out}),
    );
    assert!(!e.body.contains("iVBORw0KGgo"), "{}", e.body);
    let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
    assert_eq!(stored[0]["image_url"]["mime"], "image/png");
    assert_eq!(stored[1]["image_url"]["url"]["bytes"], 8);
    let inline = stored[2].as_str().unwrap();
    assert!(
        inline.starts_with("<img src=\"{\"kind\":\"image\"") && inline.ends_with("\"> after"),
        "{inline}"
    );
}

#[test]
fn replies_come_from_the_payload_or_the_codex_transcript() {
    let e = one("Stop", json!({"last_assistant_message": "done"}));
    assert_eq!(
        (e.kind.as_str(), body(&e)["assistant"].as_str()),
        ("reply", Some("done"))
    );
    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("rollout.jsonl");
    std::fs::write(
            &rollout,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"from codex"}]}}"#,
        )
        .unwrap();
    let v = events(
        "codex",
        "Stop",
        &json!({"transcript_path": rollout}),
        0,
        &Settings::default(),
    );
    assert_eq!(body(&v[0].event)["assistant"], "from codex");
    assert!(
        events(
            "claude",
            "Stop",
            &json!({"last_assistant_message": " "}),
            0,
            &Settings::default()
        )
        .is_empty()
    );
    // Claude Code's empty message at a session that ended in a tool call: its transcript's
    // last text, never a subagent's (parity row 17).
    let transcript = dir.path().join("session.jsonl");
    let lines = [
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "text", "text": "Tests pass."}, {"type": "text", "text": "Pushed."}]}}),
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "name": "Bash", "input": {}}]}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"role": "assistant",
                "content": [{"type": "text", "text": "a subagent's"}]}}),
    ];
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    std::fs::write(&transcript, text.join("\n")).unwrap();
    let e = one(
        "Stop",
        json!({"last_assistant_message": "", "transcript_path": transcript}),
    );
    assert_eq!(body(&e)["assistant"], "Tests pass.\nPushed.");
    assert!(
        events(
            "claude",
            "PreToolUse",
            &json!({"tool_name": "Bash"}),
            0,
            &Settings::default()
        )
        .is_empty()
    );
}

/// Milestone 4 D6: an imported event takes `events`' kind and body, and the labels its source
/// kept (session, repo, cwd) through the same gate, never the checkout as it is now (its branch
/// has moved on); v1's
/// repository row is a `touch` record.
#[test]
fn an_imported_event_keeps_its_source_labels_and_reads_no_git() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/now\n").unwrap();
    let payload = json!({"session_id": "s1", "prompt": "keep UTC", "cwd": dir.path()});
    let live = one("UserPromptSubmit", payload.clone());
    assert_eq!(live.branch.as_deref(), Some("now"));
    let settings = Settings {
        source: "oboete-v1",
        ..Settings::default()
    };
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let repo = format!("github.com/o/r/{token}");
    let mut v = imported(
        "claude",
        "UserPromptSubmit",
        &payload,
        7,
        &repo,
        dir.path().to_str(),
        &settings,
    );
    assert_eq!(v.len(), 1);
    let Captured { event: e, ledger } = v.remove(0);
    assert_eq!((e.kind.as_str(), body(&e)), ("prompt", body(&live)));
    assert_eq!((e.branch, e.head, e.gitdir), (None, None, None));
    assert_eq!(
        (e.session.as_str(), e.cwd.as_deref(), e.source.as_str()),
        ("s1", dir.path().to_str(), "oboete-v1")
    );
    let repo = e.repo.unwrap();
    assert!(
        !repo.contains(&token) && repo.contains("[REDACTED]"),
        "{repo}"
    );
    assert_eq!(ledger[0].0, "repo");
    let touch = json!({"session_id": "s1"});
    let v = imported(
        "claude",
        "Touch",
        &touch,
        5,
        "github.com/o/r",
        None,
        &settings,
    );
    let e = &v[0].event;
    assert_eq!(
        (e.kind.as_str(), e.body.as_str(), e.repo.as_deref(), e.ts),
        ("touch", "{}", Some("github.com/o/r"), 5)
    );
    // What a hook records nothing of, an import records nothing of either.
    assert!(imported("claude", "PreToolUse", &payload, 7, "r", None, &settings).is_empty());
}

#[test]
fn git_fields_come_from_files_in_a_worktree_too() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    let git = |args: &[&str], cwd: &Path| {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(ok.status.success(), "{args:?}: {ok:?}");
        String::from_utf8(ok.stdout).unwrap().trim().to_owned()
    };
    std::fs::create_dir(&main).unwrap();
    assert_eq!(super::git(&main), Git::default());
    git(&["init", "-q"], &main);
    let unborn = super::git(&main);
    assert_eq!(
        (unborn.branch.as_deref(), unborn.head),
        (Some("main"), None)
    );
    git(&["commit", "-q", "--allow-empty", "-m", "one"], &main);
    let sha = git(&["rev-parse", "HEAD"], &main);
    let wt = dir.path().join("wt");
    git(
        &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
        &main,
    );
    git(&["pack-refs", "--all"], &main); // refs now only in packed-refs
    let m = super::git(&main.join("sub-dir-that-does-not-exist"));
    assert_eq!(
        (m.branch.as_deref(), m.head.as_deref()),
        (Some("main"), Some(sha.as_str()))
    );
    let w = super::git(&wt);
    assert_eq!(
        (w.branch.as_deref(), w.head.as_deref()),
        (Some("side"), Some(sha.as_str()))
    );
    assert_ne!(w.gitdir, m.gitdir);
    assert!(w.gitdir.unwrap().contains("worktrees"));
    git(&["checkout", "-q", "--detach"], &wt);
    let d = super::git(&wt);
    assert_eq!((d.branch, d.head.as_deref()), (None, Some(sha.as_str())));
}
