use super::*;

#[test]
fn read_hook_line_keeps_the_measured_microseconds() {
    let samples = [19_125];
    assert_eq!(read_stats_ms(&samples)["p95"], json!(19.125));
    assert_eq!(stats_ms(&samples)["p95"], json!(19.1));
}

#[test]
fn a_ported_replay_keeps_fixture_times_and_reports_no_v1_summary() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = |ts: &str, prompt: &str| {
        json!({"agent": "claude", "event": "UserPromptSubmit", "ts": ts,
                   "payload": {"session_id": "s", "prompt": prompt}})
        .to_string()
    };
    let lines = [
        line("2026-09-01T00:00:00.000Z", "one"),
        line("2026-09-02T00:00:00.000Z", "two"),
    ];
    std::fs::write(&fixture, lines.join("\n")).unwrap();
    run(home.path(), &fixture, None, 0, &[1], "claude", 0, 0).unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    let ts: Vec<i64> = raw
        .after(raw.device(), 0, 10)
        .unwrap()
        .into_iter()
        .map(|r| match r.item {
            crate::raw::Item::Event(e) => e.ts,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ts, vec![1_788_220_800_000, 1_788_307_200_000]);
    // v1's store was never opened.
    assert!(!home.path().join("oboete.db").exists());
}

#[test]
fn the_24_hour_fixture_keeps_its_day_in_raw() {
    // Issue #65: replay records each event at the fixture's time, not at now.
    let home = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/fixtures/long-24h.jsonl");
    run(home.path(), &fixture, None, 0, &[1], "claude", 0, 0).unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    let ts: Vec<i64> = raw
        .after(raw.device(), 0, 10_000)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.item {
            crate::raw::Item::Event(e) => Some(e.ts),
            _ => None,
        })
        .collect();
    let span = ts.iter().max().unwrap() - ts.iter().min().unwrap();
    assert!(span >= 23 * 3_600_000 + 1_800_000, "{span}");
    // D9: a replay is not the owner at work.
    assert_eq!(raw.last_hook_ts().unwrap(), None);
    // The export ran: segments exist for what was recorded.
    assert!(
        home.path()
            .join("backups")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}

#[test]
fn a_repo_root_with_a_backslash_replays() {
    // Windows paths put backslashes into the fixture's JSON strings: they are escaped there.
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo\\xroot");
    std::fs::create_dir_all(&root).unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "cwd": ROOT_PLACEHOLDER, "prompt": "one"}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    run(home.path(), &fixture, Some(root), 0, &[1], "claude", 0, 0).unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    assert_eq!(raw.after(raw.device(), 0, 10).unwrap().len(), 1);
}

/// Milestone 4 Task 0, the read arms: before the consumers run, SessionStart shows no memory
/// for the checkout, only the work state section; once they are drained it shows the manifest,
/// the replayed prompt with it.
#[test]
fn a_cold_read_shows_nothing_and_a_warm_one_shows_the_manifest() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("replay-repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "cwd": ROOT_PLACEHOLDER,
                                      "prompt": "Fix the flaky test first."}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    run(
        home.path(),
        &fixture,
        Some(root.clone()),
        0,
        &[1],
        "claude",
        0,
        0,
    )
    .unwrap();
    let (cold, chars) = read_in_process(home.path(), &root, 3);
    assert_eq!(
        (cold.len(), chars),
        (3, crate::work_state::nothing_open().chars().count())
    );
    drop(crate::worker::drained(home.path(), false).unwrap());
    let (warm, chars) = read_in_process(home.path(), &root, 3);
    assert_eq!(warm.len(), 3);
    let text = hook::inject_text(home.path(), &root, Some("read-sample"));
    assert_eq!(chars, text.chars().count());
    assert!(text.contains("Fix the flaky test first."), "{text}");
}

/// cubic on #301: a warm arm whose drain did not run would report the cold path as warm.
#[test]
fn the_warm_arm_refuses_a_home_whose_worker_lock_is_held() {
    let home = tempfile::tempdir().unwrap();
    crate::raw::open(home.path()).unwrap();
    let _held = crate::worker::lock(home.path()).unwrap().unwrap();
    let err = drain_for_read(home.path()).unwrap_err();
    assert!(
        format!("{err:#}").contains("holds the worker lock"),
        "{err:#}"
    );
}

/// Codex on #301: a worker running when the replay starts would consume the replayed records
/// before the cold arm reads, and one that stops before the drain would go unnoticed.
#[test]
fn the_read_sample_refuses_a_worker_running_before_the_replay() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "prompt": "one"}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let _held = crate::worker::lock(home.path()).unwrap().unwrap();
    let err = run(home.path(), &fixture, None, 0, &[1], "claude", 1, 0).unwrap_err();
    assert!(
        format!("{err:#}").contains("holds the worker lock"),
        "{err:#}"
    );
    assert!(!home.path().join("raw.db").exists());
}

/// Codex on #440: the events failed live calls kept are left to a live hook, which clears the
/// marker and starts the worker when it writes them back.
#[test]
fn a_replay_refuses_a_home_with_kept_events() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    std::fs::write(&fixture, "").unwrap();
    let kept = crate::capture::Captured {
        event: crate::raw::test_event("a kept prompt"),
        ledger: Vec::new(),
    };
    crate::unwritten::keep(home.path(), &[kept], "").unwrap();
    let err = run(home.path(), &fixture, None, 0, &[1], "claude", 0, 0).unwrap_err();
    assert!(
        format!("{err:#}").contains("failed hook calls kept"),
        "{err:#}"
    );
    assert_eq!(crate::unwritten::counts(home.path()).unwrap(), (1, 0));
    // A spool that cannot be listed may hold some (Codex on #440).
    let odd = tempfile::tempdir().unwrap();
    std::fs::write(odd.path().join("unwritten"), "").unwrap();
    let err = run(odd.path(), &fixture, None, 0, &[1], "claude", 0, 0).unwrap_err();
    assert!(format!("{err:#}").contains("cannot be listed"), "{err:#}");
}

/// cubic on #301: Grok and agy inject at other points, with other payloads.
#[test]
fn the_read_sample_is_for_claude_code_only() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    std::fs::write(&fixture, "").unwrap();
    for agent in ["grok", "agy", "codex"] {
        assert!(run(home.path(), &fixture, None, 0, &[1], agent, 1, 0).is_err());
    }
    assert!(!home.path().join("raw.db").exists());
}

/// #301: a read arm forgets its hooks' records, never one another process recorded meanwhile.
#[test]
fn a_read_arm_forgets_only_its_own_records() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let before = raw.max_seq_of(raw.device()).unwrap();
    let event = |session: &str, source: &str| crate::raw::Event {
        session: session.into(),
        source: source.into(),
        body: json!({"prompt": "hello"}).to_string(),
        ..crate::raw::test_event("")
    };
    let sampled = raw
        .append(&event("read-sample-own-cold-prompt-0", "replay"))
        .unwrap();
    let real = raw.append(&event("s1", "hook")).unwrap();
    let replayed = raw.append(&event("s2", "replay")).unwrap();
    forget_samples(&mut raw, before, "read-sample-own").unwrap();
    let kept: Vec<(i64, bool)> = raw
        .after(raw.device(), before, 10)
        .unwrap()
        .into_iter()
        .filter(|r| !matches!(r.item, crate::raw::Item::Tombstone(_)))
        .map(|r| (r.seq, matches!(r.item, crate::raw::Item::Event(_))))
        .collect();
    assert_eq!(kept, [(sampled, false), (real, true), (replayed, true)]);
}

#[test]
fn a_sample_output_has_the_size_asked_for() {
    for bytes in [1024, 64 * 1024, 256 * 1024] {
        let t = tool_output(bytes);
        assert!(t.len() <= bytes && t.len() > bytes - 4, "{}", t.len());
        assert!(t.contains("token") && t.contains("テスト"));
    }
}

#[test]
fn fixture_times_are_read_to_the_millisecond() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let ms = |t: &str| fixture_ms(&conn, &json!(t));
    assert_eq!(ms("1970-01-02T00:00:00.000Z"), Some(86_400_000));
    assert_eq!(ms("2026-09-01T00:00:00.123Z"), Some(1_788_220_800_123));
    assert_eq!(ms("not a time"), None);
    assert_eq!(fixture_ms(&conn, &Value::Null), None);
}
