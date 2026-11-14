use super::*;

fn home_with(text: Option<&str>) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    if let Some(t) = text {
        std::fs::write(home.path().join("config.toml"), t).unwrap();
    }
    home
}

/// Parity row 50: only `resident = true` is taken out, and every other line stays as it was.
#[test]
fn resident_off_takes_out_only_a_resident_worker() {
    let none = home_with(None);
    assert!(!resident_off(none.path()).unwrap());
    assert_eq!(file(&none), None);
    let kept = "# mine\n[worker]\nresident = false # off\n";
    let off = home_with(Some(kept));
    assert!(!resident_off(off.path()).unwrap());
    assert_eq!(file(&off).as_deref(), Some(kept));
    let on = home_with(Some(
        "# mine\n[worker]\nresident = true\n[view]\nport = 17374\n",
    ));
    assert!(resident_off(on.path()).unwrap());
    assert_eq!(
        file(&on).as_deref(),
        Some("# mine\n[worker]\n[view]\nport = 17374\n")
    );
    assert!(!crate::config::worker(on.path()).unwrap().resident);
}

fn file(home: &tempfile::TempDir) -> Option<String> {
    std::fs::read_to_string(home.path().join("config.toml")).ok()
}

/// Codex on decision 81: setup's language is not written into a file one of its readers refuses,
/// which neither stops setup (a `[summary]` of another type) nor shows a value of the file (a
/// line that does not parse).
#[test]
fn the_computers_language_is_not_written_into_a_file_a_reader_refuses() {
    for text in [
        "summary = \"wrong\"\n",
        "[summary]\ncurate = true\nnote = \"made-up-canary-5523\" more\n",
        "[redaction]\nrules = 'made-up-canary-5523'\n",
    ] {
        let home = home_with(Some(text));
        let error = format!("{:#}", pin_language(home.path()).unwrap_err());
        assert!(!error.contains("canary"), "{error}");
        assert_eq!(file(&home).as_deref(), Some(text));
    }
}

/// Decision 81: setup writes the computer's language into a config.toml without one, or with
/// one left blank, keeps a value set, and makes no config.toml; the page is told the computer's
/// language, and a language saved empty takes the key out, its comment kept, back to the
/// computer's.
#[test]
fn the_computers_language_is_written_once_and_an_empty_one_is_its_default() {
    let none = home_with(None);
    assert_eq!(pin_language(none.path()).unwrap(), None);
    assert_eq!(file(&none), None);
    let set = "[summary]\nlanguage = \"French\" # mine\n";
    let kept = home_with(Some(set));
    assert_eq!(pin_language(kept.path()).unwrap(), None);
    assert_eq!(file(&kept).as_deref(), Some(set));
    // Left blank, it is not set (cubic on #864): the computer's is written over it.
    let blank = home_with(Some("[summary]\nlanguage = \" \" # mine\n"));
    assert_eq!(
        pin_language(blank.path()).unwrap().as_deref(),
        Some("Japanese")
    );
    assert_eq!(
        file(&blank).as_deref(),
        Some("[summary]\nlanguage = \"Japanese\" # mine\n")
    );
    let shown = show(kept.path());
    let body = posted(&shown, |v| v["summary"]["language"] = json!(""));
    save_to(&kept, &body).unwrap();
    assert_eq!(file(&kept).as_deref(), Some("[summary]\n\n # mine\n"));
    let unset = home_with(Some("# mine\n[summary]\ncurate = true\n"));
    assert_eq!(
        pin_language(unset.path()).unwrap().as_deref(),
        Some("Japanese")
    );
    assert_eq!(
        file(&unset).as_deref(),
        Some("# mine\n[summary]\ncurate = true\nlanguage = \"Japanese\"\n")
    );
    let shown = show(unset.path());
    assert_eq!(shown["language_default"], "Japanese");
    let body = posted(&shown, |v| v["summary"]["language"] = json!(" "));
    let saved = save_to(&unset, &body).unwrap();
    assert_eq!(saved["summary"]["language"], "Japanese");
    assert_eq!(
        file(&unset).as_deref(),
        Some("# mine\n[summary]\ncurate = true\n")
    );
    let body = posted(&saved, |v| v["summary"]["language"] = json!("conversation"));
    save_to(&unset, &body).unwrap();
    assert!(
        file(&unset)
            .unwrap()
            .contains("language = \"conversation\"")
    );
}

/// The page's body for the values `show` gave, with `change` applied to it.
fn posted(shown: &Value, change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let chain: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
        .map(|e| {
            json!({"name": e["name"], "on": e["on"], "daily_budget": e["daily_budget"],
                    "timeout_s": e["timeout_s"], "model": e["model"]})
        })
        .collect();
    let mut v = json!({"version": shown["version"], "inject": shown["inject"],
            "summary": shown["summary"], "paid_usd_per_month": shown["paid_usd_per_month"],
            "gemini": shown["gemini"], "capture": shown["capture"], "chain": chain});
    change(&mut v);
    serde_json::to_vec(&v).unwrap()
}

fn save_to(home: &tempfile::TempDir, body: &[u8]) -> Result<Value, Refusal> {
    save(home.path(), &Mutex::new(()), body)
}

#[test]
fn terminal_note_is_read_only_and_an_omitted_old_field_keeps_its_choice() {
    let original = "providers = []\n[inject]\nsession_start_note = false # quiet\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["inject"]["session_start_note"], false);
    assert_eq!(file(&home).as_deref(), Some(original));
    for name in ["raw.db", "knowledge.db", "providers.db"] {
        assert!(!home.path().join(name).exists());
    }
    let body = posted(&shown, |value| {
        value["inject"]
            .as_object_mut()
            .unwrap()
            .remove("session_start_note");
        value["summary"]["language"] = json!("English");
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["inject"]["session_start_note"], false);
    assert!(
        file(&home)
            .unwrap()
            .contains("session_start_note = false # quiet")
    );
    let body = posted(&saved, |value| {
        value["inject"]["session_start_note"] = json!(true);
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["inject"]["session_start_note"], true);
    assert!(
        crate::config::inject(home.path())
            .unwrap()
            .session_start_note
    );
    // Decision 51: the list above the line, on unless saved off; an older page that omits it
    // keeps the saved choice.
    assert_eq!(saved["inject"]["session_start_list"], true);
    let body = posted(&saved, |value| {
        value["inject"]["session_start_list"] = json!(false);
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["inject"]["session_start_list"], false);
    let body = posted(&saved, |value| {
        value["inject"]
            .as_object_mut()
            .unwrap()
            .remove("session_start_list");
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["inject"]["session_start_list"], false);
    assert!(
        !crate::config::inject(home.path())
            .unwrap()
            .session_start_list
    );
}

#[test]
fn backup_choices_preserve_old_requests_and_move_no_existing_data() {
    let original = "providers = []\n# backup note\n[backup]\ndir = 'old' # keep\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["backup"]["dir"], "old");
    assert_eq!(file(&home).as_deref(), Some(original));
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    let no_change = posted(&shown, |v| v["backup"] = json!({"dir": "old"}));
    let saved = save_to(&home, &no_change).unwrap();
    assert_eq!(file(&home).as_deref(), Some(original));
    let old_page = posted(&saved, |v| v["summary"]["language"] = json!("English"));
    let saved = save_to(&home, &old_page).unwrap();
    assert_eq!(saved["backup"]["dir"], "old");
    assert!(file(&home).unwrap().contains("dir = 'old' # keep"));
    std::fs::create_dir(home.path().join("old")).unwrap();
    std::fs::write(home.path().join("old/segment.zst"), "retained backup").unwrap();
    std::fs::write(home.path().join("forget.jsonl"), "retained forget log").unwrap();
    let stale = posted(&saved, |v| v["backup"] = json!({"dir": "stale-choice"}));
    let relative = posted(&saved, |v| v["backup"] = json!({"dir": "next/future"}));
    let saved = save_to(&home, &relative).unwrap();
    assert_eq!(saved["backup"]["dir"], "next/future");
    assert_eq!(
        crate::backup::dir(home.path()).unwrap(),
        home.path().join("next/future")
    );
    assert!(!home.path().join("next").exists());
    assert_eq!(save_to(&home, &stale).unwrap_err().code, "stale");
    let destination = tempfile::tempdir().unwrap();
    let absolute = destination.path().join("not-created");
    let body = posted(&saved, |v| v["backup"] = json!({"dir": absolute}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(crate::backup::dir(home.path()).unwrap(), absolute);
    assert!(!absolute.exists());
    let body = posted(&saved, |v| v["backup"] = json!({"dir": null}));
    let saved = save_to(&home, &body).unwrap();
    assert!(saved["backup"]["dir"].is_null());
    assert!(
        file(&home).unwrap().contains("# keep"),
        "removing the override lost its comment"
    );
    assert_eq!(
        crate::backup::dir(home.path()).unwrap(),
        home.path().join("backups")
    );
    assert!(!home.path().join("backups").exists());
    assert_eq!(
        std::fs::read_to_string(home.path().join("old/segment.zst")).unwrap(),
        "retained backup"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("forget.jsonl")).unwrap(),
        "retained forget log"
    );
    for name in [
        "raw.db",
        "knowledge.db",
        "providers.db",
        "state/worker.lock",
    ] {
        assert!(!home.path().join(name).exists());
    }
}

/// Owner decision 43: the page turns redaction off and on; a request that does not name the
/// switch, as an older page's, keeps what the file says.
#[test]
fn the_redaction_switch_saves_and_a_request_without_it_keeps_it() {
    let home = home_with(Some("providers = []\n"));
    let shown = show(home.path());
    assert_eq!(shown["redaction"]["enabled"], true);
    let off = posted(&shown, |v| v["redaction"]["enabled"] = json!(false));
    let saved = save_to(&home, &off).unwrap();
    assert_eq!(saved["redaction"]["enabled"], false);
    assert!(file(&home).unwrap().contains("enabled = false"));
    let rules = crate::redact::Rules::load(home.path()).unwrap();
    assert!(rules.is_off());
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    assert_eq!(crate::redact::outbound_with(&key, &rules), key);
    let older = posted(&saved, |v| {
        v["redaction"] = json!({"extra_rules": [], "allowlist": []});
    });
    let saved = save_to(&home, &older).unwrap();
    assert_eq!(saved["redaction"]["enabled"], false);
    let on = posted(&saved, |v| v["redaction"]["enabled"] = json!(true));
    let saved = save_to(&home, &on).unwrap();
    assert_eq!(saved["redaction"]["enabled"], true);
    let rules = crate::redact::Rules::load(home.path()).unwrap();
    assert!(!rules.is_off());
    assert_eq!(
        crate::redact::outbound_with(&key, &rules),
        crate::redact::MASK
    );
}

/// Owner decision 43 (Codex on #502): a save that changes the redaction rules waits for a call
/// already admitted to send, and replaces config.toml only once that call is out; a save that
/// leaves the rules as they are does not wait.
#[test]
fn a_save_that_changes_the_rules_waits_for_a_call_already_admitted() {
    let home = home_with(Some("providers = []\n"));
    let shown = show(home.path());
    let admitted = crate::dispatch::Admission::shared(home.path()).unwrap();
    let other = posted(&shown, |v| v["capture"]["store_prompts"] = json!(false));
    let shown = save_to(&home, &other).unwrap();
    let body = posted(&shown, |v| v["redaction"] = json!({"enabled": false}));
    let path = home.path().to_owned();
    let saving = std::thread::spawn(move || save(&path, &Mutex::new(()), &body));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !crate::dispatch::blocked(home.path()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the save did not wait"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!file(&home).unwrap().contains("enabled = false"));
    drop(admitted);
    let saved = saving.join().unwrap().unwrap();
    assert_eq!(saved["redaction"]["enabled"], false);
}

#[test]
fn redaction_saves_keep_all_rule_fields_and_reject_invalid_candidates_without_side_effects() {
    let original = "providers = []\n[redaction] # mandatory bundled rules stay on\n\
            extra_rules = [\n  { id = 'stable', regex = 'fixture-([A-Z]+)', keywords = ['fixture'], entropy = 0.1, secret_group = 1 }, # stable rule\n]\n\
            allowlist = [] # exceptions\n[backup]\ndir = 'retained' # other setting\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["redaction"]["extra_rules"][0]["secret_group"], 1);
    assert_eq!(
        shown["redaction"]["extra_rules"][0]["keywords"],
        json!(["fixture"])
    );
    assert_eq!(file(&home).as_deref(), Some(original));
    let unchanged = posted(&shown, |v| v["redaction"] = shown["redaction"].clone());
    let saved = save_to(&home, &unchanged).unwrap();
    assert_eq!(file(&home).as_deref(), Some(original));
    let old_page = posted(&saved, |v| v["summary"]["language"] = json!("English"));
    let saved = save_to(&home, &old_page).unwrap();
    assert!(file(&home).unwrap().contains("# stable rule"));
    let rules_before = crate::redact::Rules::load(home.path())
        .unwrap()
        .version()
        .to_owned();
    // secret_group hashes the extracted secret, rather than the surrounding regex match.
    let kept = format!("{:x}", Sha256::digest(b"PRESERVE"));
    let body = posted(&saved, |v| {
        v["redaction"] = saved["redaction"].clone();
        v["redaction"]["allowlist"] = json!([kept]);
        v["redaction"]["extra_rules"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "second", "regex": "second-([A-Z]+)", "keywords": ["second", "UPPER"],
                "entropy": 0.25, "secret_group": 1,
            }));
    });
    let saved = save_to(&home, &body).unwrap();
    let rules = crate::redact::Rules::load(home.path()).unwrap();
    assert_ne!(rules.version(), rules_before);
    assert_eq!(
        saved["redaction"]["extra_rules"][1]["keywords"],
        json!(["second", "UPPER"])
    );
    assert_eq!(saved["redaction"]["extra_rules"][1]["entropy"], 0.25);
    assert_eq!(saved["redaction"]["extra_rules"][1]["secret_group"], 1);
    assert_eq!(
        crate::redact::outbound_with("fixture-PRESERVE", &rules),
        "fixture-PRESERVE"
    );
    assert_eq!(
        crate::redact::outbound_with("second-PRIVATE", &rules),
        "second-[REDACTED]"
    );
    assert!(file(&home).unwrap().contains("# stable rule"));
    assert!(
        file(&home)
            .unwrap()
            .contains("dir = 'retained' # other setting")
    );
    let preserved = file(&home).unwrap();
    for invalid in [
        json!({"extra_rules": [{"id": "bad", "regex": "("}], "allowlist": []}),
        json!({"extra_rules": [{"id": "bad", "regex": "x", "secret_group": 1}], "allowlist": []}),
        json!({"extra_rules": [{"id": "same", "regex": "x"}, {"id": "same", "regex": "y"}], "allowlist": []}),
        json!({"extra_rules": [], "allowlist": ["not-a-hash-sensitive-sentinel"]}),
    ] {
        let body = posted(&saved, |v| v["redaction"] = invalid);
        let refusal = save_to(&home, &body).unwrap_err();
        assert_eq!(refusal.code, "redaction_invalid");
        assert_eq!(refusal.field, "redaction");
        assert!(!format!("{refusal:?}").contains("sentinel"));
        assert_eq!(file(&home).as_deref(), Some(preserved.as_str()));
    }
    assert_eq!(
        std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0
    );
    for name in [
        "raw.db",
        "knowledge.db",
        "providers.db",
        "state/worker.lock",
        "retained",
    ] {
        assert!(!home.path().join(name).exists());
    }
}

#[test]
fn rule_tables_and_an_empty_legacy_backup_survive_unrelated_saves() {
    let original = "providers = []\n[redaction]\nallowlist = []\n\
            [[redaction.extra_rules]]\nid = 'existing' # named rule\nregex = '^existing-fixture$' # pattern note\n\
            [backup]\ndir = '' # memory home\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["backup"]["dir"], "");
    assert_eq!(crate::backup::dir(home.path()).unwrap(), home.path());
    let body = posted(&shown, |v| {
        v["redaction"] = shown["redaction"].clone();
        v["summary"]["language"] = json!("English");
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["backup"]["dir"], "");
    assert!(file(&home).unwrap().contains("dir = '' # memory home"));
    let body = posted(&saved, |v| {
        v["redaction"] = saved["redaction"].clone();
        v["redaction"]["extra_rules"][0]["regex"] = json!("^new-fixture$");
        v["redaction"]["extra_rules"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "added", "regex": "^added-fixture$"}));
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(
        saved["redaction"]["extra_rules"].as_array().unwrap().len(),
        2
    );
    let text = file(&home).unwrap();
    assert_eq!(text.matches("[[redaction.extra_rules]]").count(), 2);
    assert!(text.contains("id = 'existing' # named rule"));
    assert!(text.contains("# pattern note"));
    assert!(text.contains("dir = '' # memory home"));
    let body = posted(&saved, |v| v["backup"] = json!({"dir": null}));
    save_to(&home, &body).unwrap();
    assert_eq!(
        crate::backup::dir(home.path()).unwrap(),
        home.path().join("backups")
    );
    assert!(file(&home).unwrap().contains("# memory home"));
    assert!(!home.path().join("backups").exists());
}

#[test]
fn renaming_and_reordering_rules_keeps_saved_table_comments() {
    let cases = [
        "providers = []\n[redaction]\n\
             # first table\n[[redaction.extra_rules]]\nid = 'one' # first name\n\
             regex  = '(abc)' # first pattern\nkeywords = ['abc'] # first keywords\n\
             entropy = 0.1 # first entropy\nsecret_group = 1 # first group\n\
             # second table\n[[redaction.extra_rules]]\nid = 'two' # second name\nregex = '^x$'\n\
             [backup]\ndir = 'kept' # other setting\n",
        "providers = []\n[redaction]\nextra_rules = [\n\
             # first table\n{ id = 'one', regex  = '(abc)', keywords = ['abc'], entropy = 0.1, secret_group = 1 }, # first name\n\
             # second table\n{ id = 'two', regex = '^x$' }, # second name\n]\n\
             [backup]\ndir = 'kept' # other setting\n",
    ];
    for original in cases {
        let home = home_with(Some(original));
        let shown = show(home.path());
        let body = posted(&shown, |v| {
            v["redaction"] = shown["redaction"].clone();
            v["redaction"]["extra_rules"][0]["id"] = json!("renamed");
            v["redaction"]["extra_rules"]
                .as_array_mut()
                .unwrap()
                .reverse();
        });
        let saved = save_to(&home, &body).unwrap();
        assert_eq!(saved["redaction"]["extra_rules"][0]["id"], "two");
        assert_eq!(saved["redaction"]["extra_rules"][1]["id"], "renamed");
        let text = file(&home).unwrap();
        for note in [
            "# first table",
            "# first name",
            "# second table",
            "# second name",
            "# other setting",
        ] {
            assert_eq!(
                text.matches(note).count(),
                1,
                "comment lost or duplicated: {note}"
            );
        }
        assert!(
            text.contains("regex  = '(abc)'"),
            "unchanged regex formatting was lost"
        );
        assert_eq!(
            text.matches("[[redaction.extra_rules]]").count(),
            original.matches("[[redaction.extra_rules]]").count()
        );
        let same = posted(&saved, |v| v["redaction"] = saved["redaction"].clone());
        save_to(&home, &same).unwrap();
        assert_eq!(
            file(&home).unwrap(),
            text,
            "unchanged save rewrote the renamed rule"
        );
    }
}

#[test]
fn clearing_optional_rule_fields_keeps_their_comments() {
    let original = "providers = []\n[redaction]\n\
            [[redaction.extra_rules]]\nid = 'one' # keep rule\nregex = '(abc)'\n\
            # keyword guidance\nkeywords = ['abc'] # keyword note\n\
            entropy = 0.1 # entropy note\nsecret_group = 1 # group note\n\
            [backup]\ndir = 'retained' # other setting\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    let body = posted(&shown, |v| {
        v["redaction"] = shown["redaction"].clone();
        v["redaction"]["extra_rules"][0]["keywords"] = json!([]);
        v["redaction"]["extra_rules"][0]["entropy"] = Value::Null;
        v["redaction"]["extra_rules"][0]["secret_group"] = Value::Null;
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["redaction"]["extra_rules"][0]["keywords"], json!([]));
    assert_eq!(saved["redaction"]["extra_rules"][0]["entropy"], Value::Null);
    assert_eq!(
        saved["redaction"]["extra_rules"][0]["secret_group"],
        Value::Null
    );
    let text = file(&home).unwrap();
    for note in [
        "# keep rule",
        "# keyword guidance",
        "# keyword note",
        "# entropy note",
        "# group note",
        "# other setting",
    ] {
        assert_eq!(
            text.matches(note).count(),
            1,
            "comment lost or repeated: {note}"
        );
    }
    let same = posted(&saved, |v| v["redaction"] = saved["redaction"].clone());
    save_to(&home, &same).unwrap();
    assert_eq!(
        file(&home).unwrap(),
        text,
        "a repeated save rewrote retained comments"
    );
}

#[test]
fn w6p_view_port_save_is_shared_and_older_payloads_preserve_it() {
    let original = "providers = []\n# retained configuration\n[view]\nport = 17374 # chosen\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["view"]["port"], 17374);
    assert_eq!(file(&home).as_deref(), Some(original));

    let body = posted(&shown, |v| v["view"] = json!({"port": 43123}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["view"]["port"], 43123);
    assert_eq!(config::view(home.path()).unwrap().port.get(), 43123);
    assert!(file(&home).unwrap().contains("# chosen"));
    assert!(file(&home).unwrap().contains("# retained configuration"));

    let body = posted(&saved, |v| v["summary"]["language"] = json!("English"));
    let kept = save_to(&home, &body).unwrap();
    assert_eq!(kept["view"]["port"], 43123);
    assert_eq!(config::view(home.path()).unwrap().port.get(), 43123);
}

/// #680: a `calls_a_day` written by hand out of range stays as it is through a save that does
/// not change it, a save that changes it to a value out of range is refused, and the page's
/// estimate reads no ledger into being.
#[test]
fn a_hand_written_bound_out_of_range_stays_and_the_estimate_makes_no_ledger() {
    let home = home_with(Some("providers = []\n[my_work]\ncalls_a_day = 0\n"));
    let mut raw = crate::raw::open(home.path()).unwrap();
    let prompt = crate::raw::Event {
        session: "s1".into(),
        repo: Some("r".into()),
        ts: crate::db::now_ms(),
        ..crate::raw::test_event(&json!({"prompt": "Build the parser."}).to_string())
    };
    raw.append(&prompt).unwrap();
    drop(raw);
    let shown = show(home.path());
    assert!(
        !home.path().join("providers.db").exists(),
        "the estimate made a ledger"
    );
    assert_eq!(shown["my_work"]["calls_a_day"], 0);
    assert!(shown["my_work"]["estimate"].is_object(), "{shown}");
    let body = posted(&shown, |v| {
        v["summary"]["language"] = json!("English");
        v["my_work"] = json!({"curate": false, "calls_a_day": 0});
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["my_work"]["calls_a_day"], 0);
    assert_eq!(config::load(home.path()).unwrap().my_work.calls_a_day, 0);
    let body = posted(&saved, |v| {
        v["my_work"] = json!({"curate": false, "calls_a_day": 10_001});
    });
    let r = save_to(&home, &body).unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "range", "my_work.calls_a_day")
    );
}

/// docs/my-work-model.md A6: the page shows `[my_work]` with its estimate, none before a
/// store exists and no USD with no paid entry; it saves the switch and the daily bound, refuses
/// a bound out of range, and an older page that sends none leaves them.
#[test]
fn my_work_shows_its_estimate_and_saves_its_switch_and_bound() {
    let home = home_with(Some("providers = []\n[summary]\ncurate = true\n"));
    let shown = show(home.path());
    assert_eq!(
        (
            &shown["my_work"]["curate"],
            &shown["my_work"]["calls_a_day"]
        ),
        (&json!(false), &json!(100))
    );
    assert_eq!(shown["my_work"]["estimate"], Value::Null, "no store yet");
    assert_eq!(shown["ranges"]["my_work_calls"], json!([1, 10_000]));
    let mut raw = crate::raw::open(home.path()).unwrap();
    let prompt = crate::raw::Event {
        session: "s1".into(),
        repo: Some("r".into()),
        ts: crate::db::now_ms(),
        ..crate::raw::test_event(&json!({"prompt": "Build the parser."}).to_string())
    };
    raw.append(&prompt).unwrap();
    drop(raw);
    let shown = show(home.path());
    let estimate = &shown["my_work"]["estimate"];
    assert_eq!(
        (&estimate["usd"], &estimate["priced"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(estimate["calls"][1], 100);
    assert!(estimate["calls"][0].as_u64().unwrap() >= 1, "{estimate}");
    let body = posted(&shown, |v| {
        v["my_work"] = json!({"curate": true, "calls_a_day": 40});
    });
    let saved = save_to(&home, &body).unwrap();
    let mine = |v: &Value| {
        (
            v["my_work"]["curate"].clone(),
            v["my_work"]["calls_a_day"].clone(),
        )
    };
    assert_eq!(mine(&saved), (json!(true), json!(40)));
    let cfg = config::load(home.path()).unwrap().my_work;
    assert_eq!((cfg.curate, cfg.calls_a_day), (true, 40));
    for bad in [0, 10_001] {
        let body = posted(&saved, |v| {
            v["my_work"] = json!({"curate": true, "calls_a_day": bad});
        });
        let r = save_to(&home, &body).unwrap_err();
        assert_eq!(
            (r.status, r.code, r.field.as_str()),
            (422, "range", "my_work.calls_a_day")
        );
    }
    let body = posted(&saved, |v| v["summary"]["language"] = json!("English"));
    let kept = save_to(&home, &body).unwrap();
    assert_eq!(mine(&kept), (json!(true), json!(40)));
    // Recording still reads the file.
    config::load_capture(home.path()).unwrap();
}

/// Decision 64: My Work's calls a day, the judge's and a curator's each take 4294967295 for no
/// cap, which `ranges.no_cap` states; one short of it is out of range, and My Work's estimate
/// then has no most, neither of calls nor of USD.
#[test]
fn each_calls_a_day_cap_may_be_no_cap() {
    let text = "[[providers]]\nkind = \"openai\"\nname = \"paid\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
                    limits = { usd_per_mtok_in = 1.0 }\n\n\
                    [judge]\nkey_file = \"/k/TYPESAFE_KEY.md\"\n";
    let home = home_with(Some(text));
    let mut raw = crate::raw::open(home.path()).unwrap();
    let prompt = crate::raw::Event {
        session: "s1".into(),
        repo: Some("r".into()),
        ts: crate::db::now_ms(),
        ..crate::raw::test_event(&json!({"prompt": "Build the parser."}).to_string())
    };
    raw.append(&prompt).unwrap();
    drop(raw);
    let shown = show(home.path());
    let no_cap = json!(u32::MAX);
    assert_eq!(shown["ranges"]["no_cap"], no_cap);
    assert_eq!(shown["my_work"]["estimate"]["calls"][1], 100);
    assert!(shown["my_work"]["estimate"]["usd"][1].is_f64(), "{shown}");
    let open = save_to(
        &home,
        &posted(&shown, |v| {
            v["my_work"] = json!({"curate": false, "calls_a_day": u32::MAX});
            v["judge"] = json!({"enabled": true, "daily_budget": u32::MAX});
            v["chain"][0]["daily_budget"] = json!(u32::MAX);
        }),
    )
    .unwrap();
    let cfg = config::load(home.path()).unwrap();
    assert_eq!(cfg.my_work.calls_a_day, u32::MAX);
    assert_eq!(cfg.judge.unwrap().daily_budget, u32::MAX);
    assert_eq!(cfg.providers[0].daily_budget(), u32::MAX);
    assert_eq!(open["chain"][0]["effective_daily_budget"], Value::Null);
    assert_eq!(open["judge"]["daily_budget"], no_cap);
    let estimate = &open["my_work"]["estimate"];
    assert_eq!(estimate["calls"][1], Value::Null, "{estimate}");
    assert!(estimate["calls"][0].as_u64().unwrap() >= 1, "{estimate}");
    assert!(
        estimate["usd"][0].is_f64() && estimate["usd"][1].is_null(),
        "{estimate}"
    );
    let before = file(&home).unwrap();
    for (field, edit) in [
        (
            "my_work.calls_a_day",
            json!({"curate": false, "calls_a_day": u32::MAX - 1}),
        ),
        (
            "judge.daily_budget",
            json!({"enabled": true, "daily_budget": u32::MAX - 1}),
        ),
        (
            "judge.daily_budget",
            json!({"enabled": true, "daily_budget": 0}),
        ),
    ] {
        let key = if field.starts_with("judge") {
            "judge"
        } else {
            "my_work"
        };
        let r = save_to(&home, &posted(&open, |v| v[key] = edit)).unwrap_err();
        assert_eq!((r.status, r.code, r.field.as_str()), (422, "range", field));
        assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    }
    let r = save_to(
        &home,
        &posted(&open, |v| {
            v["chain"][0]["daily_budget"] = json!(u32::MAX - 1)
        }),
    )
    .unwrap_err();
    assert_eq!((r.status, r.code), (422, "range"));
    // A cap again, and an older page that sends no judge budget leaves it.
    let capped = save_to(
        &home,
        &posted(&open, |v| {
            v["judge"] = json!({"enabled": true, "daily_budget": 50})
        }),
    )
    .unwrap();
    assert_eq!(
        config::load(home.path())
            .unwrap()
            .judge
            .unwrap()
            .daily_budget,
        50
    );
    save_to(
        &home,
        &posted(&capped, |v| v["judge"] = json!({"enabled": false})),
    )
    .unwrap();
    let judge = config::load(home.path()).unwrap().judge.unwrap();
    assert_eq!((judge.enabled, judge.daily_budget), (false, 50));
    // With no `[judge]`, a budget is refused as turning it on is.
    let home = home_with(Some("providers = []\n"));
    let shown = show(home.path());
    let r = save_to(
        &home,
        &posted(&shown, |v| {
            v["judge"] = json!({"enabled": false, "daily_budget": 50})
        }),
    )
    .unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "missing_key", "judge.daily_budget")
    );
    assert_eq!(file(&home).as_deref(), Some("providers = []\n"));
    // One written by hand out of range stays through a save that sends it unchanged (#680).
    let text = "providers = []\n[judge]\nkey_file = \"/k/TYPESAFE_KEY.md\"\ndaily_budget = 0\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            v["judge"] = json!({"enabled": true, "daily_budget": 0})
        }),
    )
    .unwrap();
    assert_eq!(file(&home).as_deref(), Some(text));
}

/// docs/retention.md: the page shows `[retention] days`, saves a day count in range or null for
/// forever, refuses one out of range, and an older page that sends none leaves it.
#[test]
fn retention_days_save_and_an_older_page_keeps_them() {
    let original = "providers = []\n[retention]\n# a month\ndays = 30\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    assert_eq!(shown["retention"]["days"], 30);
    assert_eq!(shown["ranges"]["retention_days"], json!([1, 36_500]));
    let kept = save_to(
        &home,
        &posted(&shown, |v| v["summary"]["language"] = json!("English")),
    )
    .unwrap();
    assert_eq!(kept["retention"]["days"], 30);
    let days = |home: &tempfile::TempDir| config::retention(home.path()).unwrap().days;
    assert_eq!(days(&home), Some(30));
    for (bad, code, field) in [
        (json!({"days": 0}), "range", "retention.days"),
        (json!({"days": 36_501}), "range", "retention.days"),
        (json!({"days": "30"}), "type", ""),
        (json!({}), "type", "retention.days"),
        (Value::Null, "type", ""),
    ] {
        let body = posted(&kept, |v| v["retention"] = bad.clone());
        let refusal = save_to(&home, &body).unwrap_err();
        assert_eq!(
            (refusal.status, refusal.code, refusal.field.as_str()),
            (422, code, field),
            "{bad}"
        );
        assert_eq!(days(&home), Some(30), "{bad}");
    }
    let body = posted(&kept, |v| v["retention"] = json!({"days": 90}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(
        (saved["retention"]["days"].clone(), days(&home)),
        (json!(90), Some(90))
    );
    let body = posted(&saved, |v| v["retention"] = json!({"days": null}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(
        (saved["retention"]["days"].clone(), days(&home)),
        (Value::Null, None)
    );
    assert!(file(&home).unwrap().contains("# a month"));
}

#[test]
fn w6p_invalid_or_stale_view_ports_keep_the_saved_configuration() {
    let original = "providers = []\n[view]\nport = 17374\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    for view in [
        json!({}),
        json!({"port": 0}),
        json!({"port": 65536}),
        json!({"port": "private-port-canary"}),
        json!({"port": 43123, "token": "private-port-canary"}),
    ] {
        let refusal = save_to(&home, &posted(&shown, |v| v["view"] = view)).unwrap_err();
        assert_eq!((refusal.status, refusal.code), (422, "type"));
        assert!(!format!("{refusal:?}").contains("private-port-canary"));
        assert_eq!(file(&home).as_deref(), Some(original));
    }
    let body = posted(&shown, |v| v["view"] = json!({"port": 43123}));
    let edited = format!("{original}# later owner edit\n");
    std::fs::write(home.path().join("config.toml"), &edited).unwrap();
    let refusal = save_to(&home, &body).unwrap_err();
    assert_eq!((refusal.status, refusal.code), (409, "stale"));
    assert_eq!(file(&home).as_deref(), Some(edited.as_str()));
}

#[test]
fn resident_settings_are_read_only_until_the_visible_choice_is_saved() {
    let home = home_with(None);
    let shown = show(home.path());
    assert_eq!(shown["worker"]["resident"], false);
    assert_eq!(shown["first_run"], true);
    assert_eq!(shown["resident_supported"], cfg!(target_os = "linux"));
    assert!(file(&home).is_none());

    let body = posted(&shown, |v| v["worker"] = json!({"resident": true}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["worker"]["resident"], true);
    assert_eq!(saved["first_run"], false);
    let parsed: toml::Value = toml::from_str(&file(&home).unwrap()).unwrap();
    assert_eq!(parsed["worker"]["resident"].as_bool(), Some(true));

    let body = posted(&saved, |v| v["worker"] = json!({"resident": false}));
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["worker"]["resident"], false);
    let before = file(&home);
    // An older page omitting the new field keeps the owner's saved mode.
    let saved = save_to(&home, &posted(&saved, |_| {})).unwrap();
    assert_eq!(saved["worker"]["resident"], false);
    assert_eq!(file(&home), before);

    let off = home_with(None);
    let body = posted(&show(off.path()), |v| {
        v["worker"] = json!({"resident": false})
    });
    let saved = save_to(&off, &body).unwrap();
    assert_eq!(saved["worker"]["resident"], false);
    assert_eq!(
        saved["first_run"], false,
        "an explicit first off choice is saved too"
    );
    assert!(file(&off).unwrap().contains("resident = false"));
}

#[test]
fn resident_saves_reject_wrong_types_stale_versions_and_invalid_runtime_settings() {
    let original = "providers = []\n[worker]\nresident = false # chosen\n[view]\nport = 17374\n";
    let home = home_with(Some(original));
    let shown = show(home.path());
    for worker in [
        json!({}),
        json!({"resident": null}),
        json!({"resident": "yes"}),
        json!({"resident": true, "command": "invented"}),
    ] {
        let body = posted(&shown, |v| v["worker"] = worker);
        let error = save_to(&home, &body).unwrap_err();
        assert_eq!((error.status, error.code), (422, "type"));
        assert_eq!(file(&home).as_deref(), Some(original));
    }
    let body = posted(&shown, |v| v["worker"] = json!({"resident": true}));
    let edited = format!("{original}# Edited by hand\n");
    std::fs::write(home.path().join("config.toml"), &edited).unwrap();
    let error = save_to(&home, &body).unwrap_err();
    assert_eq!((error.status, error.code), (409, "stale"));
    assert_eq!(file(&home).as_deref(), Some(edited.as_str()));
    for invalid in ["[worker]\nresident = 'yes'\n", "[view]\nport = 0\n"] {
        std::fs::write(home.path().join("config.toml"), invalid).unwrap();
        assert_eq!(show(home.path())["error"], "file_invalid");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_settings_save_waits_for_the_other_config_writer_then_refuses_its_stale_body() {
    let home = home_with(Some("providers = []\n[worker]\nresident = false\n"));
    let original = file(&home).unwrap();
    let body = posted(&show(home.path()), |v| {
        v["worker"] = json!({"resident": true})
    });
    std::fs::create_dir(home.path().join("state")).unwrap();
    let fence = std::fs::File::create(home.path().join("state/config.lock")).unwrap();
    fence.lock().unwrap();
    let path = home.path().to_owned();
    let (send, receive) = std::sync::mpsc::channel();
    let saving = std::thread::Builder::new()
        .name("r5-config-save".into())
        .spawn(move || {
            send.send(save(&path, &Mutex::new(()), &body)).unwrap();
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        assert!(
            receive.try_recv().is_err(),
            "save completed while another writer held config.lock"
        );
        let waiting = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .any(|task| {
                std::fs::read_to_string(task.path().join("comm"))
                    .is_ok_and(|name| name.trim() == "r5-config-save")
                    && std::fs::read_to_string(task.path().join("wchan"))
                        .is_ok_and(|wait| wait.contains("locks_") || wait.contains("flock_"))
            });
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "save never waited for config.lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(file(&home).as_deref(), Some(original.as_str()));
    let edited = format!("{original}# The first writer changed the version\n");
    std::fs::write(home.path().join("config.toml"), &edited).unwrap();
    drop(fence);
    let error = receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    saving.join().unwrap();
    assert_eq!((error.status, error.code), (409, "stale"));
    assert_eq!(file(&home).as_deref(), Some(edited.as_str()));
}

#[test]
fn the_summary_shows_saved_values_and_the_parser_ranges() {
    let home = home_with(None);
    let shown = show(home.path());
    assert_eq!(
        shown["summary"],
        json!({"curate": false, "language": "Japanese", "window_tokens": 5000,
                "idle_minutes": 10})
    );
    assert_eq!(
        shown["ranges"]["window_tokens"],
        json!([0, 4_294_967_295_u32])
    );
    assert_eq!(
        shown["ranges"]["idle_minutes"],
        json!([0, 4_294_967_295_u32])
    );
    assert!(!home.path().join("config.toml").exists());
    assert!(!home.path().join("providers.db").exists());

    let home = home_with(Some(
        "[summary]\ncurate = true\nlanguage = \"\"\nwindow_tokens = 0\nidle_minutes = 4294967295\n",
    ));
    // A blank language is the computer's, as one not set (decision 81; cubic on #864).
    assert_eq!(
        show(home.path())["summary"],
        json!({"curate": true, "language": "Japanese", "window_tokens": 0,
                "idle_minutes": 4_294_967_295_u32})
    );
}

#[test]
fn a_summary_save_is_lossless_and_stale_checked() {
    let text = "# invented settings\n[summary]\ncurate = false # owner switch\n\
            language = \"Japanese\" # language note\nwindow_tokens = 5000 # size note\n\
            idle_minutes = 10 # wait note\nshrink = true\nfuture = \"kept\"\n\
            [backup]\ndir = \"invented-backups\" # unrelated\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let unchanged = save_to(&home, &posted(&shown, |_| {})).unwrap();
    assert_eq!(file(&home).as_deref(), Some(text));
    assert_eq!(unchanged["version"], shown["version"]);
    let body = posted(&shown, |v| {
        v["summary"] = json!({"curate": true, "language": "English\n日本語 <example>",
                "window_tokens": 0, "idle_minutes": 4_294_967_295_u32});
    });
    let saved = save_to(&home, &body).unwrap();
    assert_eq!(saved["summary"]["language"], "English\n日本語 <example>");
    let cfg = config::load(home.path()).unwrap();
    assert!(cfg.summary.curate && cfg.summary.shrink == crate::curate::Shrink::All);
    assert_eq!(cfg.summary.window_tokens, 0);
    assert_eq!(cfg.summary.idle_minutes, u32::MAX);
    let after = file(&home).unwrap();
    for kept in [
        "# invented settings",
        "# owner switch",
        "# language note",
        "# size note",
        "# wait note",
        "shrink = true",
        "future = \"kept\"",
        "dir = \"invented-backups\" # unrelated",
    ] {
        assert!(after.contains(kept), "{kept}: {after}");
    }
    assert!(!home.path().join("providers.db").exists());
    let stale = save_to(&home, &body).unwrap_err();
    assert_eq!((stale.status, stale.code), (409, "stale"));
    assert_eq!(file(&home).as_deref(), Some(after.as_str()));
}

#[test]
fn the_paid_cap_takes_values_of_0_or_more_or_null_for_no_cap() {
    let home = home_with(Some(
        "paid_usd_per_month = 5.0 # monthly cap\n[summary]\nfuture = 7\n",
    ));
    let mut shown = show(home.path());
    assert_eq!(shown["paid_usd_per_month"], 5.0);
    assert_eq!(shown["ranges"]["paid_usd_per_month"], json!([0, null]));
    for cap in [0.0, 0.125, 5.0, 1.0e300] {
        shown = save_to(
            &home,
            &posted(&shown, |v| v["paid_usd_per_month"] = json!(cap)),
        )
        .unwrap();
        assert_eq!(shown["paid_usd_per_month"], cap);
        assert_eq!(config::load(home.path()).unwrap().paid_usd_per_month, cap);
        let text = file(&home).unwrap();
        assert!(
            text.contains("# monthly cap") && text.contains("future = 7"),
            "{text}"
        );
    }
    // Decision 64: a null is no cap, written `inf` and shown null; a save that leaves it keeps
    // it, and a number brings a cap back.
    shown = save_to(
        &home,
        &posted(&shown, |v| v["paid_usd_per_month"] = Value::Null),
    )
    .unwrap();
    assert_eq!(shown["paid_usd_per_month"], Value::Null);
    assert_eq!(
        config::load(home.path()).unwrap().paid_usd_per_month,
        config::no_monthly_cap()
    );
    let text = file(&home).unwrap();
    assert!(
        text.contains("paid_usd_per_month = inf") && text.contains("# monthly cap"),
        "{text}"
    );
    shown = save_to(
        &home,
        &posted(&shown, |v| v["summary"]["language"] = json!("English")),
    )
    .unwrap();
    assert_eq!(shown["paid_usd_per_month"], Value::Null, "left as it was");
    shown = save_to(
        &home,
        &posted(&shown, |v| v["paid_usd_per_month"] = json!(5.0)),
    )
    .unwrap();
    assert_eq!(config::load(home.path()).unwrap().paid_usd_per_month, 5.0);
    let before = file(&home).unwrap();
    // Left out, it is refused, not read as no cap.
    let left_out = posted(&shown, |v| {
        v.as_object_mut().unwrap().remove("paid_usd_per_month");
    });
    let r = save_to(&home, &left_out).unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "type", "paid_usd_per_month")
    );
    assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    let invalid = [
        ("paid_usd_per_month", json!(-0.1)),
        ("paid_usd_per_month", json!("5")),
    ];
    for (field, value) in invalid {
        let refusal = save_to(&home, &posted(&shown, |v| v[field] = value)).unwrap_err();
        assert_eq!(refusal.status, 422);
        assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    }
    for (field, value) in [
        ("curate", json!("true")),
        ("language", json!(42)),
        ("window_tokens", json!(-1)),
        ("window_tokens", json!(4_294_967_296_u64)),
        ("window_tokens", json!(1.5)),
        ("idle_minutes", json!(-1)),
        ("idle_minutes", json!(4_294_967_296_u64)),
        ("idle_minutes", json!(1.5)),
        ("shrink", json!(true)),
    ] {
        let refusal = save_to(&home, &posted(&shown, |v| v["summary"][field] = value)).unwrap_err();
        assert_eq!((refusal.status, refusal.code), (422, "type"), "{field}");
        assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    }
    let overflow = String::from_utf8(posted(&shown, |v| v["paid_usd_per_month"] = json!(0)))
        .unwrap()
        .replace("\"paid_usd_per_month\":0", "\"paid_usd_per_month\":1e999");
    assert!(save_to(&home, overflow.as_bytes()).is_err());
    assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    assert!(!home.path().join("providers.db").exists());
}

#[test]
fn the_spend_and_the_owner_stops_are_read_without_a_write() {
    let empty = home_with(None);
    let shown = show(empty.path());
    assert_eq!(shown["usd_this_month"], 0.0);
    assert_eq!(
        (&shown["stopped"], &shown["resting"]),
        (&json!([]), &json!([]))
    );
    assert!(!empty.path().join("providers.db").exists());

    let names = "[[providers]]\nkind = \"cli\"\nname = \"owner-stopped\"\ncli = \"claude\"\n\
                     [[providers]]\nkind = \"cli\"\nname = \"time-stopped\"\ncli = \"claude\"\n";
    let home = home_with(Some(names));
    let db = Connection::open(home.path().join("providers.db")).unwrap();
    // An older ledger needs no schema upgrade to read its spend and stops.
    db.execute_batch(
        "CREATE TABLE provider_calls(provider TEXT, ts INTEGER, role TEXT, usd REAL);
            CREATE TABLE provider_state(provider TEXT, down_until INTEGER);
            INSERT INTO provider_state VALUES ('owner-stopped', 9223372036854775807),
                ('time-stopped', 9223372036854775806), ('expired', 1);",
    )
    .unwrap();
    for (provider, ts, role, usd) in [
        ("removed-entry", crate::db::now_ms(), "curator", Some(0.25)),
        ("other-entry", crate::db::now_ms(), "digest", Some(0.5)),
        ("embedding", crate::db::now_ms(), "embed", Some(99.0)),
        ("embedding", crate::db::now_ms(), "query", Some(99.0)),
        ("old-entry", 1, "curator", Some(88.0)),
        ("free-entry", crate::db::now_ms(), "curator", None),
    ] {
        db.execute(
            "INSERT INTO provider_calls VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![provider, ts, role, usd],
        )
        .unwrap();
    }
    let before = std::fs::read(home.path().join("providers.db")).unwrap();
    let shown = show(home.path());
    assert_eq!(shown["usd_this_month"], 0.75);
    assert_eq!(shown["stopped"], json!(["owner-stopped"]));
    // The older ledger has no rest set without credits (69b-3c-1b).
    assert_eq!(
        shown["resting"],
        json!([{"name": "time-stopped", "until": 9_223_372_036_854_775_806_i64}])
    );
    assert_eq!(
        std::fs::read(home.path().join("providers.db")).unwrap(),
        before
    );
    assert!(!home.path().join("providers.db-wal").exists());
    assert_eq!(file(&home).as_deref(), Some(names));
    // A name the ledger keeps that config.toml no longer has is not listed (69b-3c-1b).
    std::fs::write(home.path().join("config.toml"), "providers = []\n").unwrap();
    let shown = show(home.path());
    assert_eq!(
        (&shown["stopped"], &shown["resting"]),
        (&json!([]), &json!([]))
    );

    let damaged = home_with(Some("providers = []\n"));
    std::fs::write(
        damaged.path().join("providers.db"),
        "invented damaged ledger",
    )
    .unwrap();
    let shown = show(damaged.path());
    assert!(shown.get("error").is_none());
    assert_eq!(shown["usd_this_month"], Value::Null);
    assert_eq!(
        (&shown["stopped"], &shown["resting"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(
        std::fs::read_to_string(damaged.path().join("providers.db")).unwrap(),
        "invented damaged ledger"
    );
}

#[test]
fn geminis_place_follows_the_config_chain() {
    let home = home_with(Some("[summary]\nfuture = \"kept\" # unchanged\n"));
    let mut shown = show(home.path());
    assert!(
        shown.get("gemini").is_some(),
        "Gemini's saved choice must be shown"
    );
    assert_eq!(shown["gemini"], Value::Null);
    for place in [
        json!("before-subscriptions"),
        json!("after-subscriptions"),
        Value::Null,
    ] {
        shown = save_to(&home, &posted(&shown, |v| v["gemini"] = place.clone())).unwrap();
        assert_eq!(shown["gemini"], place);
        let cfg = config::load(home.path()).unwrap();
        let gemini = cfg.providers.iter().position(|p| p.name() == "gemini");
        match place.as_str() {
            Some("before-subscriptions") => {
                let first_cli = cfg
                    .providers
                    .iter()
                    .position(|p| matches!(p, Provider::Cli { .. }))
                    .unwrap();
                assert_eq!(gemini, Some(first_cli - 1));
            }
            Some("after-subscriptions") => assert_eq!(gemini, Some(cfg.providers.len() - 1)),
            _ => assert!(gemini.is_none()),
        }
        assert_eq!(
            shown["chain"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "gemini"),
            place != Value::Null
        );
        assert!(
            file(&home)
                .unwrap()
                .contains("future = \"kept\" # unchanged")
        );
        assert!(!home.path().join("providers.db").exists());
    }
    let before = file(&home).unwrap();
    for invalid in [json!("none"), json!("elsewhere"), json!(true), json!(1)] {
        let refusal = save_to(&home, &posted(&shown, |v| v["gemini"] = invalid)).unwrap_err();
        assert!(matches!(
            (refusal.status, refusal.code),
            (422, "type") | (400, "bad_request")
        ));
        assert_eq!(file(&home).as_deref(), Some(before.as_str()));
    }

    // The parser keeps an explicit Gemini entry and lets [chain] order take precedence.
    let explicit = home_with(Some(
        "gemini = \"before-subscriptions\" # place\n\
            [chain]\norder = [\"subscription\", \"gemini\"]\n\
            [[providers]]\nkind = \"cli\"\nname = \"subscription\"\ncli = \"invented-cli\"\n\
            [[providers]]\nkind = \"openai\"\nname = \"gemini\"\n\
            base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"invented-model\"\n",
    ));
    let shown = show(explicit.path());
    let saved = save_to(
        &explicit,
        &posted(&shown, |v| v["gemini"] = json!("after-subscriptions")),
    )
    .unwrap();
    assert_eq!(saved["chain"][0]["name"], "subscription");
    assert_eq!(saved["chain"][1]["name"], "gemini");
    assert!(
        file(&explicit)
            .unwrap()
            .contains("gemini = \"after-subscriptions\" # place")
    );
    let saved = save_to(&explicit, &posted(&saved, |v| v["gemini"] = Value::Null)).unwrap();
    assert_eq!(saved["gemini"], Value::Null);
    assert_eq!(saved["chain"][1]["name"], "gemini");
}

#[test]
fn taking_gemini_out_keeps_its_comments() {
    let home = home_with(Some(
        "# invented heading\ngemini = \"after-subscriptions\" # place note\n\
            paid_usd_per_month = 5.0 # cap note\n[summary]\nfuture = \"kept\" # future note\n",
    ));
    let shown = show(home.path());
    let saved = save_to(&home, &posted(&shown, |v| v["gemini"] = Value::Null)).unwrap();
    assert_eq!(saved["gemini"], Value::Null);
    let text = file(&home).unwrap();
    for comment in [
        "# invented heading",
        "# place note",
        "# cap note",
        "# future note",
    ] {
        assert!(text.contains(comment), "missing {comment}: {text}");
    }
    assert_eq!(config::load(home.path()).unwrap().gemini, None);
}

#[test]
fn a_top_level_setting_changed_keeps_the_comments_above_it() {
    let home = home_with(Some(
        "# cap heading\n\"paid_usd_per_month\" = 5.0 # cap suffix\n\
            # Gemini heading\n\"gemini\" = \"before-subscriptions\" # Gemini suffix\n\
            [summary]\nfuture = \"kept\" # unrelated\n",
    ));
    let shown = show(home.path());
    let saved = save_to(
        &home,
        &posted(&shown, |v| {
            v["paid_usd_per_month"] = json!(0.125);
            v["gemini"] = json!("after-subscriptions");
        }),
    )
    .unwrap();
    assert_eq!(saved["paid_usd_per_month"], 0.125);
    assert_eq!(saved["gemini"], "after-subscriptions");
    let text = file(&home).unwrap();
    for kept in [
        "# cap heading",
        "# Gemini heading",
        "\"paid_usd_per_month\" = 0.125 # cap suffix",
        "\"gemini\" = \"after-subscriptions\" # Gemini suffix",
        "future = \"kept\" # unrelated",
    ] {
        assert!(text.contains(kept), "missing {kept}: {text}");
    }
}

fn at<'a>(shown: &'a Value, name: &str) -> &'a Value {
    (shown["chain"].as_array().unwrap().iter())
        .find(|e| e["name"] == name)
        .unwrap()
}

/// A config of API entries, each named with its key file.
fn api_entries(entries: &[(&str, &Path)]) -> String {
    (entries.iter())
        .map(|(name, file)| {
            format!(
                "[[providers]]\nkind = \"openai\"\nname = \"{name}\"\n\
                     base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\nkey_file = {:?}\n\n",
                file.display().to_string()
            )
        })
        .collect()
}

fn key_body(entry: &str, key: &str, version: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"entry": entry, "key": key, "version": version})).unwrap()
}

/// #94 part 3: a key typed on the page reaches its entry's key file, against the config the
/// page showed, and no answer holds it.
#[cfg(target_os = "linux")]
#[test]
fn a_key_saved_from_the_page_reaches_its_file_and_no_answer() {
    let keys = tempfile::tempdir().unwrap();
    let file = keys.path().join("LOCAL_KEY.md");
    std::fs::write(&file, "Local\nold-key-123\n").unwrap();
    let solo = keys.path().join("SOLO_KEY.md");
    std::fs::write(&solo, "Solo\nsolo-key-123\n").unwrap();
    let text = api_entries(&[
        ("local", &file),
        ("twin", &file),
        ("twin", &file),
        ("solo", &solo),
    ]);
    let home = home_with(Some(&text));
    let version = show(home.path())["version"].clone();
    let canary = format!("{}-{}", "canary", "7d1e0b52");
    // Parity row 11: the entries that read the file rest no longer; others do.
    let db = crate::providers_db::open(home.path()).unwrap();
    let rest = crate::providers_db::State {
        down_until: 5_000_000_000_000,
        ..Default::default()
    };
    for p in ["local", "twin", "solo", "other"] {
        crate::providers_db::set_state(&db, p, rest).unwrap();
    }
    let saved = save_key(
        home.path(),
        &Mutex::new(()),
        &key_body("local", &canary, &version),
    );
    assert_eq!(
        saved,
        Ok(json!({"entry": "local", "key": "ok", "durable": true}))
    );
    let rests =
        ["local", "twin", "solo", "other"].map(|p| crate::providers_db::state(&db, p).unwrap());
    assert_eq!(rests, [Default::default(), Default::default(), rest, rest]);
    assert_eq!(crate::config::read_key(&file).unwrap(), canary);
    let shown = show(home.path());
    assert!(!shown.to_string().contains(&canary), "{shown}");
    // config.toml is not written, so the page's version still holds.
    assert_eq!(
        (self::file(&home), &shown["version"]),
        (Some(text.clone()), &version)
    );
    // Entries of one name that name one file take the key.
    let twin = save_key(
        home.path(),
        &Mutex::new(()),
        &key_body("twin", "twin-key-1", &version),
    );
    assert_eq!(twin.unwrap()["key"], "ok");
    assert_eq!(crate::config::read_key(&file).unwrap(), "twin-key-1");
    // A config changed since the page read it: refused, nothing written.
    std::fs::write(home.path().join("config.toml"), format!("{text}# edited\n")).unwrap();
    let stale = save_key(
        home.path(),
        &Mutex::new(()),
        &key_body("local", "late-key-12", &version),
    );
    let r = stale.unwrap_err();
    assert_eq!((r.status, r.code), (409, "stale"));
    assert_eq!(crate::config::read_key(&file).unwrap(), "twin-key-1");
}

/// #94 part 3: a key save names one key file of one API entry, and a request of another
/// shape writes nothing.
#[test]
fn a_key_save_names_one_key_file_of_one_api_entry() {
    let keys = tempfile::tempdir().unwrap();
    let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
    let text = api_entries(&[("twin", &a), ("twin", &b)])
        + "[[providers]]\nkind = \"openai\"\nname = \"open\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\n\
               [[providers]]\nkind = \"cli\"\nname = \"sub\"\ncli = \"claude\"\n";
    let home = home_with(Some(&text));
    let version = show(home.path())["version"].clone();
    let refusal = |body: Vec<u8>| {
        let r = save_key(home.path(), &Mutex::new(()), &body).unwrap_err();
        (r.status, r.code)
    };
    let key = "key-abcdefgh12";
    assert_eq!(refusal(key_body("twin", key, &version)), (422, "ambiguous"));
    assert_eq!(
        refusal(key_body("open", key, &version)),
        (422, "no_key_file")
    );
    assert_eq!(
        refusal(key_body("sub", key, &version)),
        (422, "no_key_file")
    );
    assert_eq!(
        refusal(key_body("nobody", key, &version)),
        (404, "no_entry")
    );
    assert_eq!(
        refusal(key_body("twin", "short", &version)),
        (422, "bad_key")
    );
    assert_eq!(
        refusal(key_body("twin", "key-abc\nbase_url", &version)),
        (422, "bad_key")
    );
    assert_eq!(
        refusal(key_body(&"x".repeat(65), key, &version)),
        (422, "bad_entry")
    );
    let extra =
        json!({"entry": "twin", "key": key, "version": version, "key_file": "/tmp/X_KEY.md"});
    assert_eq!(
        refusal(serde_json::to_vec(&extra).unwrap()),
        (400, "bad_request")
    );
    assert_eq!(self::file(&home), Some(text));
    assert!(!a.exists() && !b.exists());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn a_key_save_waits_for_owner_only_files_off_linux() {
    let keys = tempfile::tempdir().unwrap();
    let file = keys.path().join("LOCAL_KEY.md");
    let home = home_with(Some(&api_entries(&[("local", &file)])));
    let version = show(home.path())["version"].clone();
    let r = save_key(
        home.path(),
        &Mutex::new(()),
        &key_body("local", "key-abcdefgh12", &version),
    );
    let r = r.unwrap_err();
    assert_eq!((r.status, r.code), (422, "unsupported"));
    assert!(!file.exists());
}

/// #94 acceptance test 2: no key reaches the page, and none is taken from it.
#[test]
fn no_key_is_shown_or_taken() {
    let home = tempfile::tempdir().unwrap();
    let sentinel = format!("{}-{}", "oboete-sentinel", "4b1d");
    let key = home.path().join("LOCAL_KEY.md");
    std::fs::write(&key, format!("# the key\n{sentinel}\n")).unwrap();
    let text = format!(
        "[[providers]]\nkind = \"openai\"\nname = \"local\"\nbase_url = \"http://127.0.0.1:9/v1\"\n\
             model = \"m\"\nkey_file = {:?}\nheaders = {{ \"x-h\" = \"{sentinel}\" }}\n\
             extra = {{ note = \"{sentinel}\" }}\n",
        key.display().to_string()
    );
    std::fs::write(home.path().join("config.toml"), &text).unwrap();
    let shown = show(home.path());
    assert!(shown.get("error").is_none(), "{shown}");
    assert!(!shown.to_string().contains(&sentinel), "{shown}");
    assert_eq!(at(&shown, "local")["key"], "ok");
    for extra in [
        ("key", json!(sentinel)),
        ("base_url", json!("http://evil.example/v1")),
        ("key_file", json!("/tmp/k")),
        ("headers", json!({"x": "y"})),
    ] {
        let body = posted(&shown, |v| {
            v["chain"][0][extra.0] = extra.1.clone();
        });
        let refusal = save_to(&home, &body).unwrap_err();
        assert_eq!((refusal.status, refusal.code), (422, "type"), "{}", extra.0);
        assert_eq!(file(&home).as_deref(), Some(text.as_str()));
    }
    let saved = save_to(
        &home,
        &posted(&shown, |v| v["chain"][0]["timeout_s"] = json!(30)),
    )
    .unwrap();
    assert!(!saved.to_string().contains(&sentinel), "{saved}");
}

/// A save writes only the keys it changes, keeps the rest of the file (comments, other
/// tables) as it was, and leaves a value equal to the entry's own out.
#[test]
fn a_save_writes_only_what_changes() {
    let text = "# my settings\n[summary]\ncurate = false # not yet\n\n[backup]\ndir = \"b\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let unchanged = save_to(&home, &posted(&shown, |_| {})).unwrap();
    assert_eq!(file(&home).as_deref(), Some(text));
    assert_eq!(unchanged["version"], shown["version"]);
    let groq = at(&shown, "groq");
    let own_timeout = groq["effective_timeout_s"].clone();
    let names: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
        .map(|e| e["name"].clone())
        .collect();
    let saved = save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]["session_start_chars"] = json!(2000);
            v["capture"]["tool_output"] = json!("head-tail");
            let chain = v["chain"].as_array_mut().unwrap();
            chain.swap(0, 1);
            for e in chain.iter_mut() {
                match e["name"].as_str().unwrap() {
                    "codex" => e["on"] = json!(false),
                    "groq" => {
                        e["timeout_s"] = own_timeout.clone();
                        e["daily_budget"] = json!(50);
                    }
                    "claude" => e["model"] = json!("sonnet"),
                    _ => {}
                }
            }
        }),
    )
    .unwrap();
    let after = file(&home).unwrap();
    assert!(after.starts_with(text), "{after}");
    for line in [
        "session_start_chars = 2000",
        "tool_output = \"head-tail\"",
        "off = [\"codex\"]",
        "daily_budget = { groq = 50 }",
        "model = { claude = \"sonnet\" }",
    ] {
        assert!(after.contains(line), "{line}: {after}");
    }
    // The groq timeout equals its own: not written.
    assert!(!after.contains("timeout_s"), "{after}");
    let order = format!(
        "order = [{:?}, {:?}",
        names[1].as_str().unwrap(),
        names[0].as_str().unwrap()
    );
    assert!(after.contains(&order), "{order}: {after}");
    assert_eq!(saved["inject"]["session_start_chars"], 2000);
    assert_eq!(at(&saved, "codex")["on"], false);
    assert_eq!(at(&saved, "claude")["effective_model"], "sonnet");
    // Back to the entry's own values: the keys go.
    let back = save_to(
        &home,
        &posted(&saved, |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                e["on"] = json!(true);
                e["daily_budget"] = Value::Null;
                e["model"] = Value::Null;
            }
            let chain = v["chain"].as_array_mut().unwrap();
            chain.swap(0, 1);
        }),
    )
    .unwrap();
    let after = file(&home).unwrap();
    for key in ["order", "off", "daily_budget", "model"] {
        assert!(!after.contains(&format!("{key} =")), "{key}: {after}");
    }
    assert_eq!(at(&back, "codex")["on"], true);
}

/// Each refusal leaves the file's bytes as they were.
/// J1: the judge's switch saves where `[judge]` is; with no table (no key yet) turning it on
/// is refused and nothing is written, and an older page that leaves it out changes nothing.
#[test]
fn the_judges_switch_saves_only_where_its_key_is() {
    let text = "providers = []\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let r = save_to(
        &home,
        &posted(&shown, |v| v["judge"] = json!({"enabled": true})),
    )
    .unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "missing_key", "judge.enabled")
    );
    assert_eq!(file(&home).as_deref(), Some(text));
    save_to(
        &home,
        &posted(&shown, |v| v["judge"] = json!({"enabled": false})),
    )
    .unwrap();
    assert_eq!(file(&home).as_deref(), Some(text));
    let text = "providers = []\n[judge]\nkey_file = \"/k/TYPESAFE_KEY.md\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    assert_eq!(
        (&shown["judge"]["set"], &shown["judge"]["enabled"]),
        (&json!(true), &json!(true))
    );
    let saved = save_to(
        &home,
        &posted(&shown, |v| v["judge"] = json!({"enabled": false})),
    )
    .unwrap();
    assert_eq!(saved["judge"]["enabled"], false);
    assert!(!config::load(home.path()).unwrap().judge.unwrap().enabled);
    let shown = show(home.path());
    save_to(&home, &posted(&shown, |_| {})).unwrap();
    assert!(
        !config::load(home.path()).unwrap().judge.unwrap().enabled,
        "left out, left alone"
    );
}

/// J7: the destination saves from the page. One chosen while the judge is on needs what it
/// sends with (a key, and Cloudflare's account id); d1 on this PC needs none, so it alone writes
/// `[judge]` where there is none. A name not on the list is refused.
#[test]
fn a_destination_chosen_while_on_needs_its_key() {
    fn choose(home: &tempfile::TempDir, enabled: bool, d: &str) -> Result<Value, Refusal> {
        let shown = show(home.path());
        let to = json!({"enabled": enabled, "destination": d});
        save_to(home, &posted(&shown, |v| v["judge"] = to))
    }
    let destination = |home: &tempfile::TempDir| {
        let j = config::load(home.path()).unwrap().judge.unwrap();
        (j.destination.id(), j.enabled)
    };
    let keys = tempfile::tempdir().unwrap();
    let typesafe = keys.path().join("TYPESAFE_KEY.md");
    std::fs::write(&typesafe, "# a test key\nTESTKEY_for_the_stub\n").unwrap();
    let text = format!(
        "providers = []\n[judge]\nkey_file = '{}'\n",
        typesafe.display()
    );
    let home = home_with(Some(&text));
    assert_eq!(show(home.path())["judge"]["destination"], "jev");
    let r = choose(&home, true, "clef-flash").unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "missing_key", "judge.destination")
    );
    assert_eq!(file(&home).as_deref(), Some(text.as_str()));
    choose(&home, false, "clef-flash").unwrap();
    assert_eq!(destination(&home), ("clef-flash", false));
    choose(&home, true, "d1-local").unwrap();
    assert_eq!(destination(&home), ("d1-local", true));
    choose(&home, true, "jev").unwrap();
    assert_eq!(destination(&home), ("jev", true));
    let r = choose(&home, true, "clef-omni").unwrap_err();
    assert_eq!((r.status, r.code), (422, "type"));
    // One saved while off, with no key, is not turned on by the switch alone either; one on
    // stays on as it was.
    choose(&home, false, "clef-flash").unwrap();
    let text = file(&home).unwrap();
    let shown = show(home.path());
    let on = |v: &mut Value| v["judge"] = json!({"enabled": true});
    let r = save_to(&home, &posted(&shown, on)).unwrap_err();
    assert_eq!(
        (r.status, r.code, r.field.as_str()),
        (422, "missing_key", "judge.enabled")
    );
    assert_eq!(file(&home).as_deref(), Some(text.as_str()));
    // With no `[judge]`, d1 on this PC alone writes it, on.
    let home = home_with(Some("providers = []\n"));
    let r = choose(&home, true, "jev").unwrap_err();
    assert_eq!((r.status, r.code), (422, "missing_key"));
    choose(&home, false, "d1-local").unwrap();
    assert_eq!(file(&home).as_deref(), Some("providers = []\n"));
    choose(&home, true, "d1-local").unwrap();
    assert_eq!(destination(&home), ("d1-local", true));
}

/// A hand-written `judge:` entry left on does not count as the chain in use (`load_chain`
/// leaves it out): turning off the last other entry is refused.
#[test]
fn a_judge_name_left_on_does_not_keep_the_chain_in_use() {
    let text = "[[providers]]\nkind = \"openai\"\nname = \"judge:jev\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\n\
                    [[providers]]\nkind = \"openai\"\nname = \"open\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let r = save_to(
        &home,
        &posted(&shown, |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == "open" {
                    e["on"] = json!(false);
                }
            }
        }),
    )
    .unwrap_err();
    assert_eq!((r.status, r.code), (422, "chain_empty"));
    assert_eq!(file(&home).as_deref(), Some(text));
}

#[test]
fn a_refused_save_leaves_the_file() {
    let text = "[summary]\ncurate = false\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let refusal = |change: &dyn Fn(&mut Value)| {
        let r = save_to(&home, &posted(&shown, change)).unwrap_err();
        assert_eq!(file(&home).as_deref(), Some(text));
        (r.status, r.code, r.field)
    };
    let named = |name: &'static str, key: &'static str, value: Value| {
        move |v: &mut Value| {
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == name {
                    e[key] = value.clone();
                }
            }
        }
    };
    assert_eq!(
        refusal(&|v| v["version"] = json!("0000")),
        (409, "stale", String::new())
    );
    assert_eq!(
        refusal(&|v| v["inject"]["session_start_chars"] = json!(999)),
        (422, "range", "inject.session_start_chars".into())
    );
    assert_eq!(
        refusal(&|v| v["inject"]["session_start_index"] = json!(51)),
        (422, "range", "inject.session_start_index".into())
    );
    assert_eq!(
        refusal(&named("groq", "daily_budget", json!(0))),
        (422, "range", "chain.groq.daily_budget".into())
    );
    assert_eq!(
        refusal(&named("groq", "timeout_s", json!(4))),
        (422, "range", "chain.groq.timeout_s".into())
    );
    assert_eq!(
        refusal(&named("groq", "model", json!("m\nx"))),
        (422, "model", "chain.groq.model".into())
    );
    assert_eq!(
        refusal(&named("openrouter", "model", json!("openai/gpt-6"))),
        (422, "paid_model", "chain.openrouter.model".into())
    );
    assert_eq!(
        refusal(&|v| {
            v["chain"].as_array_mut().unwrap().pop();
        }),
        (422, "names", "chain".into())
    );
    assert_eq!(
        refusal(&|v| {
            let first = v["chain"][0].clone();
            v["chain"].as_array_mut().unwrap().push(first);
        }),
        (422, "names", "chain".into())
    );
    assert_eq!(
        refusal(&|v| {
            for e in v["chain"].as_array_mut().unwrap() {
                e["on"] = json!(false);
            }
        }),
        (422, "chain_empty", "chain".into())
    );
    assert_eq!(
        refusal(&|v| v["capture"]["tool_output"] = json!("some")),
        (422, "type", String::new())
    );
    let r = save_to(&home, b"{not json").unwrap_err();
    assert_eq!((r.status, r.code), (400, "bad_request"));
    assert_eq!(file(&home).as_deref(), Some(text));
}

/// A model the entry cannot price is refused; an OpenRouter `:free` entry takes another
/// `:free` model, and an entry with prices keeps its model.
#[test]
fn a_model_the_entry_cannot_price_is_refused() {
    let text = "gemini = \"after-subscriptions\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    assert_eq!(at(&shown, "gemini")["model_rule"], "fixed");
    assert_eq!(at(&shown, "openrouter")["model_rule"], "free");
    assert_eq!(at(&shown, "groq")["model_rule"], "any");
    let model = |name: &'static str, m: &'static str| {
        posted(&shown, move |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == name {
                    e["model"] = json!(m);
                }
            }
        })
    };
    let r = save_to(&home, &model("gemini", "gemini-9-pro")).unwrap_err();
    assert_eq!((r.status, r.code), (422, "paid_model"));
    let own = at(&shown, "gemini")["effective_model"]
        .as_str()
        .unwrap()
        .to_owned();
    let same = posted(&shown, |v| {
        for e in v["chain"].as_array_mut().unwrap() {
            if e["name"] == "gemini" {
                e["model"] = json!(own);
            }
        }
    });
    save_to(&home, &same).unwrap();
    assert_eq!(file(&home).as_deref(), Some(text));
    let saved = save_to(&home, &model("openrouter", "qwen/qwen3.8-27b:free")).unwrap();
    assert_eq!(
        at(&saved, "openrouter")["effective_model"],
        "qwen/qwen3.8-27b:free"
    );
}

/// Task 8: the prompt's injection and its corrections are shown with their ranges, saved
/// alone, and refused out of range by name.
#[test]
fn inject_settings_check_ranges_and_save_alone() {
    let home = home_with(None);
    let shown = show(home.path());
    assert_eq!(
        shown["inject"],
        json!({"session_start": true, "session_start_note": true, "session_start_list": true, "session_start_chars": 9000,
                "session_start_index": 10, "per_prompt": false,
                "per_prompt_chars": 2500, "per_prompt_wait_ms": 2000, "memory_lookup": true,
                "memory_instructions": true, "correction": true, "correction_chars": 800})
    );
    assert_eq!(shown["ranges"]["session_start_index"], json!([0, 50]));
    assert_eq!(shown["ranges"]["per_prompt_chars"], json!([500, 6000]));
    assert_eq!(shown["ranges"]["per_prompt_wait_ms"], json!([100, 3000]));
    assert_eq!(shown["ranges"]["correction_chars"], json!([300, 3000]));
    save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]["per_prompt"] = json!(true);
            v["inject"]["per_prompt_wait_ms"] = json!(800);
            v["inject"]["correction_chars"] = json!(1200);
        }),
    )
    .unwrap();
    assert_eq!(
        file(&home).as_deref(),
        Some("[inject]\nper_prompt = true\nper_prompt_wait_ms = 800\ncorrection_chars = 1200\n")
    );
    let i = config::inject(home.path()).unwrap();
    assert!(i.per_prompt && i.correction_chars == 1_200 && i.per_prompt_wait_ms == 800);
    // A page from before G23 sends no wait: the saved one stays.
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]
                .as_object_mut()
                .unwrap()
                .remove("per_prompt_wait_ms");
            v["inject"]["per_prompt"] = json!(false);
        }),
    )
    .unwrap();
    assert_eq!(config::inject(home.path()).unwrap().per_prompt_wait_ms, 800);
    // Decision 75: the index's lines save alone; a page from before it keeps them.
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| v["inject"]["session_start_index"] = json!(4)),
    )
    .unwrap();
    assert!(file(&home).unwrap().contains("session_start_index = 4\n"));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]
                .as_object_mut()
                .unwrap()
                .remove("session_start_index");
            v["inject"]["per_prompt"] = json!(true);
        }),
    )
    .unwrap();
    assert_eq!(config::inject(home.path()).unwrap().session_start_index, 4);
    // The memory lookup's switch saves alone; a page that omits it keeps it.
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| v["inject"]["memory_lookup"] = json!(false)),
    )
    .unwrap();
    assert!(!config::inject(home.path()).unwrap().memory_lookup);
    let shown = show(home.path());
    let omitted = posted(&shown, |v| {
        v["inject"].as_object_mut().unwrap().remove("memory_lookup");
    });
    save_to(&home, &omitted).unwrap();
    assert!(!config::inject(home.path()).unwrap().memory_lookup);
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]["memory_instructions"] = json!(false)
        }),
    )
    .unwrap();
    assert!(!config::inject(home.path()).unwrap().memory_instructions);
    for (key, bad) in [
        ("per_prompt_chars", 499),
        ("per_prompt_wait_ms", 99),
        ("correction_chars", 3_001),
    ] {
        let shown = show(home.path());
        let r = save_to(&home, &posted(&shown, |v| v["inject"][key] = json!(bad))).unwrap_err();
        assert_eq!(
            (r.status, r.code, r.field),
            (422, "range", format!("inject.{key}"))
        );
    }
}

/// No file: the page shows the defaults and a save makes a file with only what it changes.
#[test]
fn a_save_without_a_file_makes_one() {
    let home = home_with(None);
    let shown = show(home.path());
    assert_eq!(shown["version"], "none");
    assert_eq!(shown["inject"]["session_start_chars"], 9_000);
    save_to(
        &home,
        &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
    )
    .unwrap();
    assert_eq!(
        file(&home).as_deref(),
        Some("[inject]\nsession_start = false\n")
    );
    assert!(!config::inject(home.path()).unwrap().session_start);
}

/// A chain with no entry in use already, by hand or with no entries at all, does not stop
/// the rest of the settings from saving; turning off the last one in use does (cubic on #94).
#[test]
fn a_chain_with_none_in_use_still_saves_the_rest() {
    let names: Vec<String> = (show(home_with(None).path())["chain"].as_array().unwrap())
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    let all_off = format!("[chain]\noff = {names:?}\n");
    for text in ["providers = []\n", all_off.as_str()] {
        let home = home_with(Some(text));
        let shown = show(home.path());
        let saved = save_to(
            &home,
            &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
        )
        .unwrap_or_else(|r| panic!("{text}: {r:?}"));
        assert_eq!(saved["inject"]["session_start"], false, "{text}");
        assert!(file(&home).unwrap().starts_with(text), "{text}");
    }
}

/// What is not the page's stays as the file has it: a name no entry has, and a value out of
/// the page's range that the user wrote by hand (cubic on #94).
#[test]
fn a_save_keeps_what_is_not_the_pages() {
    let text = "[chain]\noff = [\"gone\"]\ndaily_budget = { old = 20, groq = 200000 }\n\
                    timeout_s = { groq = 1200 }\nmodel = { old = \"m x\" }\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    assert_eq!(at(&shown, "groq")["timeout_s"], 1200);
    let saved = save_to(
        &home,
        &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
    )
    .unwrap();
    assert_eq!(saved["inject"]["session_start"], false);
    assert!(file(&home).unwrap().starts_with(text), "{:?}", file(&home));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                match e["name"].as_str().unwrap() {
                    "groq-20b" => e["daily_budget"] = json!(50),
                    "nim" => e["on"] = json!(false),
                    _ => {}
                }
            }
        }),
    )
    .unwrap();
    let chain = &config::load(home.path()).unwrap().chain;
    assert_eq!(chain.off, ["nim", "gone"]);
    assert_eq!(chain.daily_budget["old"], 20);
    assert_eq!(chain.daily_budget["groq"], 200_000);
    assert_eq!(chain.daily_budget["groq-20b"], 50);
    assert_eq!(chain.timeout_s["groq"], 1200);
    assert_eq!(chain.model["old"], "m x");
}

/// `[chain]` sets every entry of one name alike, so the page has one row per name, and a
/// value is the entries' own only when it is each one's (Codex on #270).
#[test]
fn entries_of_one_name_are_one_row() {
    let entry = "[[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n";
    let text = format!("{entry}timeout_s = 60\n\n{entry}timeout_s = 90\n");
    let home = home_with(Some(&text));
    let shown = show(home.path());
    assert_eq!(shown["chain"].as_array().unwrap().len(), 1);
    assert_eq!(shown["chain"][0]["entries"], 2);
    let r = save_to(
        &home,
        &posted(&shown, |v| {
            let row = v["chain"][0].clone();
            v["chain"].as_array_mut().unwrap().push(row);
        }),
    )
    .unwrap_err();
    assert_eq!((r.status, r.code), (422, "names"));
    save_to(
        &home,
        &posted(&shown, |v| v["chain"][0]["timeout_s"] = json!(60)),
    )
    .unwrap();
    let timeouts: Vec<u64> = (config::load(home.path()).unwrap().providers.iter())
        .map(|p| match p {
            Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. } => *timeout_s,
        })
        .collect();
    assert_eq!(timeouts, [60, 60]);
}

#[test]
fn provider_rows_keep_physical_selectors_and_hide_secret_fields() {
    let text = "gemini = \"after-subscriptions\"\n\
            [chain]\norder = [\"b\", \"a\"]\noff = [\"a\"]\n\
            timeout_s = { a = 120 }\nmodel = { a = \"overlaid\" }\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"first\"\n\
            timeout_s = 60\nheaders = { authorization = \"header-canary\" }\n\
            extra = { secret = \"extra-canary\" }\n\
            [[providers]]\nkind = \"cli\"\nname = \"b\"\ncli = \"claude\"\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://user:endpoint-canary@example.invalid/v1?token=query-canary\"\n\
            model = \"second\"\ntimeout_s = 90\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let rows = shown["providers"]
        .as_array()
        .expect("individual provider rows");
    assert_eq!(rows.len(), 4);
    for (i, model, order) in [(0, "first", 1), (2, "second", 2)] {
        assert_eq!(rows[i]["selector"], json!({"source": "file", "index": i}));
        assert_eq!(rows[i]["saved"]["model"], model);
        assert_eq!(rows[i]["effective"]["model"], "overlaid");
        assert_eq!(rows[i]["effective"]["timeout_s"], 120);
        assert_eq!(rows[i]["effective"]["on"], false);
        assert_eq!(rows[i]["effective"]["order"], order);
    }
    assert_eq!(rows[0]["saved"]["base_url"], "https://example.invalid/v1");
    assert_eq!(rows[2]["saved"]["base_url"], Value::Null);
    assert_eq!(rows[3]["selector"], json!({"source": "gemini", "index": 0}));
    assert_eq!(shown["chain"].as_array().unwrap().len(), 3);
    for canary in [
        "header-canary",
        "extra-canary",
        "endpoint-canary",
        "query-canary",
    ] {
        assert!(!shown.to_string().contains(canary));
    }
    assert_eq!(file(&home).as_deref(), Some(text));
    assert!(!home.path().join("state").exists());
    assert!(!home.path().join("providers.db").exists());

    let empty = home_with(None);
    let shown = show(empty.path());
    let rows = shown["providers"].as_array().unwrap();
    assert!(!rows.is_empty());
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["selector"], json!({"source": "builtin", "index": i}));
    }
    assert!(std::fs::read_dir(empty.path()).unwrap().next().is_none());
}

#[test]
fn individual_provider_edits_keep_duplicates_unknowns_and_stale_config() {
    let text = "# config note\n[chain]\nmodel = { a = \"overlay\" } # group note\n\
            [[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n\
            model = \"first\" # first note\ntimeout_s = 60\n\
            [[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n\
            model = \"second\" # second note\ntimeout_s = 90\nfuture = \"kept\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let body = serde_json::to_vec(&json!({"version": shown["version"],
            "action": {"op": "edit", "selector": shown["providers"][1]["selector"],
                "entry": {"kind": "cli", "name": "a", "cli": "claude", "model": "new",
                    "enabled": false, "timeout_s": 120, "limits": {
                        "max_request_tokens": null, "daily_tokens": null,
                        "usd_per_mtok_in": 0.0, "usd_per_mtok_out": 0.0,
                        "max_output_tokens": 4000}}}}))
    .unwrap();
    let saved = save_provider(home.path(), &Mutex::new(()), &body).expect("physical edit");
    let cfg = config::load(home.path()).unwrap();
    assert_eq!(cfg.providers.len(), 2);
    assert_eq!(saved["providers"][0]["saved"]["model"], "first");
    assert_eq!(saved["providers"][1]["saved"]["model"], "new");
    assert_eq!(saved["providers"][1]["effective"]["model"], "overlay");
    assert_eq!(saved["providers"][1]["effective"]["on"], false);
    assert_eq!(config::load_chain(home.path()).unwrap().providers.len(), 1);
    let after = file(&home).unwrap();
    for kept in [
        "# config note",
        "# group note",
        "# first note",
        "# second note",
        "future = \"kept\"",
    ] {
        assert!(after.contains(kept), "{kept}");
    }
    let refusal = save_provider(home.path(), &Mutex::new(()), &body).unwrap_err();
    assert_eq!((refusal.status, refusal.code), (409, "stale"));
    assert_eq!(file(&home).as_deref(), Some(after.as_str()));
    assert!(!home.path().join("providers.db").exists());
}

/// A priced entry and a CLI entry of one name: one row with the stricter rule and `[chain]`'s
/// model, which applies to the CLI entry. A save that leaves the model keeps it, and one that
/// empties it removes it (Codex and CodeRabbit on #270).
#[test]
fn a_shared_name_shows_one_model_and_keeps_it() {
    let text = "[chain]\nmodel = { a = \"x\" }\n\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
                    limits = { usd_per_mtok_in = 1.0 }\n\n\
                    [[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let row = &shown["chain"][0];
    assert_eq!(
        (&row["model"], &row["model_rule"]),
        (&json!("x"), &json!("fixed"))
    );
    save_to(
        &home,
        &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
    )
    .unwrap();
    assert_eq!(config::load(home.path()).unwrap().chain.model["a"], "x");
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| v["chain"][0]["model"] = Value::Null),
    )
    .unwrap();
    assert!(config::load(home.path()).unwrap().chain.model.is_empty());
}

/// #274 item 1: a `[chain]` timeout past 2^53 reaches the page rounded, as a browser parses a
/// JSON number into a double. An unrelated save sends it back rounded, and the file keeps its
/// own value.
#[test]
fn a_timeout_past_2_53_survives_an_unrelated_save() {
    // The second: the entry's own value rounds as the file's `[chain]` one does (cubic on
    // #287), so taking the posted number for the entry's own would drop the file's.
    let own = "[[providers]]\nkind = \"openai\"\nname = \"groq\"\n\
                   base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                   timeout_s = 9007199254740992\n";
    for rest in ["", own] {
        let text = format!("[chain]\ntimeout_s = {{ groq = 9007199254740993 }}\n\n{rest}");
        let home = home_with(Some(&text));
        let shown = show(home.path());
        let body = posted(&shown, |v| {
            v["inject"]["session_start"] = json!(false);
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == "groq" {
                    // What `JSON.parse` and then `JSON.stringify` make of it.
                    e["timeout_s"] = json!(9_007_199_254_740_992_u64);
                }
            }
        });
        save_to(&home, &body).unwrap();
        let after = file(&home).unwrap();
        assert!(after.contains("groq = 9007199254740993"), "{after}");
        assert!(!crate::config::inject(home.path()).unwrap().session_start);
    }
}

/// #274 item 2: a `[chain]` model no entry of its name can take is shown, as not applied, and
/// a save that leaves it keeps it; one that empties it removes it.
#[test]
fn a_model_no_entry_takes_is_shown_and_kept() {
    let text = "[chain]\nmodel = { a = \"x\" }\n\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
                    limits = { usd_per_mtok_in = 1.0 }\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    let row = &shown["chain"][0];
    assert_eq!(
        (
            &row["model"],
            &row["model_applied"],
            &row["effective_model"]
        ),
        (&json!("x"), &json!(false), &json!("m"))
    );
    save_to(
        &home,
        &posted(&shown, |v| v["inject"]["session_start"] = json!(false)),
    )
    .unwrap();
    assert!(file(&home).unwrap().contains("a = \"x\""));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| v["chain"][0]["model"] = Value::Null),
    )
    .unwrap();
    assert!(!file(&home).unwrap().contains("a = \"x\""));
}

/// A `[chain]` model the row's entry cannot take is marked, though another entry of its name
/// takes it (cubic on #287): the row shows the first entry, and says the entries differ.
#[test]
fn a_model_the_rows_entry_does_not_take_is_marked() {
    let entry = |model: &str, priced: &str| {
        format!(
            "[[providers]]\nkind = \"openai\"\nname = \"a\"\nbase_url = \"https://example.invalid/v1\"\n\
                 model = \"{model}\"\n{priced}\n"
        )
    };
    let text = format!(
        "[chain]\nmodel = {{ a = \"x\" }}\n\n{}{}",
        entry("m", "limits = { usd_per_mtok_in = 1.0 }"),
        entry("n", "")
    );
    let home = home_with(Some(&text));
    let row = &show(home.path())["chain"][0];
    assert_eq!(
        (&row["model"], &row["model_applied"], &row["differs"]),
        (&json!("x"), &json!(false), &json!(["model"]))
    );
}

/// Entries of one name whose calls a day come from their keys, one read and one not, use
/// different budgets, and the row says so (cubic on #287).
#[test]
fn entries_whose_keys_give_different_budgets_say_so() {
    let keys = tempfile::tempdir().unwrap();
    let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
    std::fs::write(&a, "# a\nkey-a\n").unwrap();
    std::fs::write(&b, "# b\nkey-b\n").unwrap();
    let entry = |f: &std::path::Path| {
        format!(
            "[[providers]]\nkind = \"openai\"\nname = \"o\"\n\
                 base_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m:free\"\nkey_file = {:?}\n",
            f.display().to_string()
        )
    };
    let home = home_with(Some(&(entry(&a) + &entry(&b))));
    let db = crate::providers_db::open(home.path()).unwrap();
    let first = (config::load(home.path()).unwrap().providers.into_iter())
        .next()
        .unwrap();
    let key = crate::provider::key_of(&first);
    crate::providers_db::set_key_limit(&db, "o", Some(1000), crate::db::now_ms(), &key).unwrap();
    let row = &show(home.path())["chain"][0];
    assert_eq!(
        (&row["effective_daily_budget"], &row["differs"]),
        (&json!(200), &json!(["key_file", "daily_budget"]))
    );
}

/// 69b-3c-1b: the rests from one read of the ledger, in the settings answer and alone.
/// `resting` is a future timed rest, neither an owner's hold nor a past one; a name is listed only
/// while its state holds every enabled entry of it, as the curator decides.
#[test]
fn a_name_rests_only_while_its_state_holds_every_enabled_entry() {
    let codex = |credits: bool, enabled: bool| {
        format!(
            "[[providers]]\nkind = \"cli\"\nname = \"codex\"\ncli = \"codex\"\n\
                 credits = {credits}\nenabled = {enabled}\n"
        )
    };
    let cli = |name: &str| {
        format!("[[providers]]\nkind = \"cli\"\nname = \"{name}\"\ncli = \"claude\"\n")
    };
    let lists = |text: &str, rows: &[(&str, i64, bool)]| {
        let home = home_with(Some(text));
        let db = crate::providers_db::open(home.path()).unwrap();
        for &(p, down_until, uncredited) in rows {
            let state = crate::providers_db::State {
                down_until,
                uncredited,
                ..Default::default()
            };
            crate::providers_db::set_state(&db, p, state).unwrap();
        }
        let shown = show(home.path());
        let alone = rests(home.path());
        assert_eq!(
            (&shown["stopped"], &shown["resting"]),
            (&alone["stopped"], &alone["resting"])
        );
        // No rebuild or rewrite holds its lock here (#855).
        assert!(alone["paused"].is_null());
        json!({"stopped": alone["stopped"], "resting": alone["resting"]})
    };
    let later = crate::db::now_ms() + 3_600_000;
    let hold = crate::providers_db::OWNER_HOLD;
    let text = cli("a") + &cli("b") + &cli("c");
    assert_eq!(
        lists(
            &text,
            &[("a", later, false), ("b", hold, false), ("c", 1, false)]
        ),
        json!({"stopped": ["b"], "resting": [{"name": "a", "until": later}]})
    );
    // A codex entry with credits under a rest set without them, alone and beside an entry of
    // its name that the rest holds: the name can still run, and is not listed.
    for text in [codex(true, true), codex(true, true) + &codex(false, true)] {
        for until in [later, hold] {
            assert_eq!(
                lists(&text, &[("codex", until, true)]),
                json!({"stopped": [], "resting": []}),
                "{text} {until}"
            );
        }
    }
    // A disabled entry with credits is never tried, so it hides no rest of its name; enabled,
    // it does.
    assert_eq!(
        lists(
            &(codex(true, false) + &codex(false, true)),
            &[("codex", later, true)]
        ),
        json!({"stopped": [], "resting": [{"name": "codex", "until": later}]})
    );
    assert_eq!(
        lists(
            &(codex(true, true) + &codex(false, true)),
            &[("codex", later, true)]
        ),
        json!({"stopped": [], "resting": []})
    );
    // A name whose entries are all turned off is not in use: neither its rest nor its stop is
    // listed (#817; cubic and CodeRabbit on #816).
    for until in [later, hold] {
        assert_eq!(
            lists(&codex(false, false), &[("codex", until, false)]),
            json!({"stopped": [], "resting": []}),
            "{until}"
        );
    }
}

/// #800 item 3 (69b-3c): a name is capped while any of its entries stops at a calls a day, in
/// either order, though the row's effective calls a day is the first entry's.
#[test]
fn a_name_is_capped_while_any_entry_has_a_calls_a_day() {
    let entry = |budget: &str| {
        format!(
            "[[providers]]\nkind = \"openai\"\nname = \"sub\"\nsubscription = true\n\
                 base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n{budget}"
        )
    };
    let row = |text: String| show(home_with(Some(&text)).path())["chain"][0].clone();
    let open = entry("");
    let capped = entry("daily_budget = 50\n");
    for text in [open.clone() + &capped, capped.clone() + &open] {
        let row = row(text);
        assert_eq!(
            (&row["subscription"], &row["capped"]),
            (&json!(true), &json!(true))
        );
    }
    assert_eq!(row(open.clone() + &open)["capped"], json!(false));
}

/// #274 item 3: the row shows the first entry's values, and says where entries of its name
/// differ.
#[test]
fn entries_of_one_name_say_where_they_differ() {
    let keys = tempfile::tempdir().unwrap();
    let (a, b) = (keys.path().join("A_KEY.md"), keys.path().join("B_KEY.md"));
    let text = api_entries(&[("twin", &a), ("same", &a), ("same", &a)])
        + &format!(
            "[[providers]]\nkind = \"openai\"\nname = \"twin\"\n\
                 base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"n\"\nkey_file = {:?}\n",
            b.display().to_string()
        );
    let home = home_with(Some(&text));
    let shown = show(home.path());
    assert_eq!(at(&shown, "twin")["differs"], json!(["key_file", "model"]));
    assert_eq!(at(&shown, "same")["differs"], json!([]));
}

/// The rule shown is the entry's own, without `[chain]`, as a save checks it: an OpenRouter
/// entry of a paid model that `[chain]` gives a `:free` one still takes any model
/// (OpenCodeReview on #270).
#[test]
fn the_rule_shown_is_the_entrys_own() {
    let text = "[chain]\nmodel = { o = \"m:free\" }\n\n[[providers]]\nkind = \"openai\"\n\
                    name = \"o\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"paid/m\"\n";
    let home = home_with(Some(text));
    let row = &show(home.path())["chain"][0];
    assert_eq!(
        (&row["model"], &row["model_rule"]),
        (&json!("m:free"), &json!("any"))
    );
}

/// A map written as a table of its own changes entry by entry: an entry that does not change
/// keeps its comment and place, and one that does keeps its comment (cubic on #270).
#[test]
fn a_map_table_changes_entry_by_entry() {
    let text = "[chain.daily_budget]\ngroq = 100 # half\nnim = 50 # trying it\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            for e in v["chain"].as_array_mut().unwrap() {
                match e["name"].as_str().unwrap() {
                    "nim" => e["daily_budget"] = json!(60),
                    "groq-20b" => e["daily_budget"] = json!(20),
                    _ => {}
                }
            }
        }),
    )
    .unwrap();
    let saved = file(&home).unwrap();
    assert!(
        saved.starts_with("[chain.daily_budget]\ngroq = 100 # half\nnim = 60 # trying it\n"),
        "{saved}"
    );
    let budgets = config::load(home.path()).unwrap().chain.daily_budget;
    assert_eq!(
        (budgets["groq"], budgets["nim"], budgets["groq-20b"]),
        (100, 60, 20)
    );
}

/// A key whose value changes keeps the comment after it (cubic on #270).
#[test]
fn a_changed_key_keeps_its_comment() {
    let text = "[chain]\noff = [\"groq\"] # paused until Friday\n\n[inject]\n\
                    session_start_chars = 3000 # short\n";
    let home = home_with(Some(text));
    let shown = show(home.path());
    save_to(
        &home,
        &posted(&shown, |v| {
            v["inject"]["session_start_chars"] = json!(4000);
            for e in v["chain"].as_array_mut().unwrap() {
                if e["name"] == "nim" {
                    e["on"] = json!(false);
                }
            }
        }),
    )
    .unwrap();
    let saved = file(&home).unwrap();
    assert!(
        saved.contains("off = [\"groq\", \"nim\"] # paused until Friday"),
        "{saved}"
    );
    assert!(
        saved.contains("session_start_chars = 4000 # short"),
        "{saved}"
    );
}

/// A file that does not parse: no form, and no save.
#[test]
fn a_file_that_does_not_parse_is_neither_shown_nor_written() {
    for text in [
        "[chain]\noff = 3\n",
        "[inject]\nsession_start = \"no\"\n",
        "[summary\n",
        // Read by backups and by capture's redaction alone (Codex on #270).
        "[backup]\ndir = 3\n",
        "[redaction]\nextra_rules = [{ id = \"a\", regex = '(' }]\n",
    ] {
        let home = home_with(Some(text));
        let shown = show(home.path());
        assert_eq!(shown["error"], "file_invalid", "{text}");
        let body = json!({"version": shown["version"], "inject": {"session_start": true,
                "session_start_chars": 6000, "per_prompt": false, "per_prompt_chars": 1500,
                "correction": true, "correction_chars": 800}, "capture": {"store_prompts": true,
                "tool_output": "full"}, "chain": [], "summary": {"curate": false,
                "language": "Japanese", "window_tokens": 5000, "idle_minutes": 10},
                "paid_usd_per_month": 5.0});
        let r = save_to(&home, &serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!((r.status, r.code), (422, "file_invalid"), "{text}");
        assert_eq!(file(&home).as_deref(), Some(text));
    }
}
