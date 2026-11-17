//! An agent reads a hook's output to its end. A hook that starts the worker must not leave that
//! output open for the worker's lifetime: Windows passes a process's inheritable handles on to
//! its children, the hook's own stdout among them (60 s measured on Windows 11).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a session start shows while there is no memory and nothing open: the work state
/// section (docs/work-state.md L7) and the memory tools' use after it (claude-mem 13.35's block).
fn only_the_work_state_section(out: &[u8]) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(out).unwrap();
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let (work, memory_use) = context
        .split_once("\n# oboete memory use\n")
        .expect(context);
    assert!(
        work.starts_with("# Work state: your to-do lists and working state\n")
            && work.ends_with("\n\nNothing open yet.")
            && memory_use.contains("save_memory")
            && !context.contains("<oboete-memory>"),
        "{context}"
    );
    v
}

/// A config.toml with `rest` whose notes are in Japanese: unset, the note's language is the
/// computer's (decision 81), which differs between the machines the tests run on.
fn japanese(home: &std::path::Path, rest: &str) {
    let config = format!("{rest}[summary]\nlanguage = \"Japanese\"\n");
    std::fs::write(home.join("config.toml"), config).unwrap();
}

fn hook_output(
    home: &std::path::Path,
    agent: &str,
    event: &str,
    input: &str,
) -> std::process::Output {
    hook_output_with(home, agent, event, input, &[])
}

/// `hook_output`, with `env` set for the hook.
fn hook_output_with(
    home: &std::path::Path,
    agent: &str,
    event: &str,
    input: &str,
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(["hook", agent, event])
        // No worker is wanted for a hook's output.
        .env("OBOETE_NO_SPAWN", "1")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    hook.wait_with_output().unwrap()
}

/// #955: Pi's plugin gives a hook it waits for the time its wait ends (`OBOETE_ANSWER_BY_MS`, ms
/// since the epoch). A prompt whose answer is no longer wanted is recorded, and its hook injects
/// nothing.
#[test]
fn a_pi_prompt_past_its_answers_time_is_recorded_and_given_nothing() {
    let home = tempfile::tempdir().unwrap();
    let past = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        - 1;
    let out = hook_output_with(
        home.path(),
        "pi",
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"a prompt Pi stopped waiting for"}"#,
        &[("OBOETE_ANSWER_BY_MS", &past.to_string())],
    );
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nothing injected: the hook's time is over"),
        "{stderr}"
    );
    let raw = rusqlite::Connection::open_with_flags(
        home.path().join("raw.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let recorded: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM records WHERE body LIKE '%a prompt Pi stopped waiting for%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recorded, 1);
}

/// ms since the epoch.
fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// `oboete hook pi <event>` with `input`, told that Pi waits 2 s for its answer, and killed as that
/// wait ends, as Pi's plugin kills it.
fn pi_hook_killed_at_its_answers_time(home: &std::path::Path, event: &str, input: &str) {
    let by = epoch_ms() + 2_000;
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(["hook", "pi", event])
        .env("OBOETE_NO_SPAWN", "1")
        .env("OBOETE_ANSWER_BY_MS", by.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let kill_at = Instant::now() + Duration::from_millis(2_000);
    while hook.try_wait().unwrap().is_none() && Instant::now() < kill_at {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = hook.kill();
    hook.wait().unwrap();
}

/// What `home`'s kept files hold.
fn kept_text(home: &std::path::Path) -> String {
    std::fs::read_dir(home.join("unwritten"))
        .map(|dir| {
            dir.filter_map(Result::ok)
                .filter_map(|f| std::fs::read_to_string(f.path()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// #955: a prompt and a session's start that Pi waits for, on a store whose write lock is held,
/// killed as Pi's 2 s wait ends: each hook waits for the store only until 1 s before then, and
/// keeps its event before the kill.
#[test]
fn a_pi_hook_on_a_held_store_keeps_its_event_before_pi_kills_it() {
    let home = tempfile::tempdir().unwrap();
    let first = hook_output(
        home.path(),
        "pi",
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"the store is made"}"#,
    );
    assert!(first.status.success());
    let lock = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    pi_hook_killed_at_its_answers_time(
        home.path(),
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"a prompt on a held store"}"#,
    );
    pi_hook_killed_at_its_answers_time(
        home.path(),
        "SessionStart",
        r#"{"session_id":"t","source":"startup"}"#,
    );
    lock.execute_batch("ROLLBACK").unwrap();
    let kept = kept_text(home.path());
    assert!(kept.contains("a prompt on a held store"), "{kept}");
    assert!(kept.contains("\"t\""), "the start of session t: {kept}");
}

/// #955: a hook Pi waits for writes no kept file back (docs/unwritten.md U3): it keeps its own
/// events behind them, and the next call that waits as long as it likes writes them all back in
/// their order.
#[test]
fn a_pi_hook_pi_waits_for_keeps_its_event_behind_the_kept_files() {
    let home = tempfile::tempdir().unwrap();
    let made = hook_output(
        home.path(),
        "pi",
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"the store is made"}"#,
    );
    assert!(made.status.success());
    let lock = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let past = (epoch_ms() - 1).to_string();
    let kept = hook_output_with(
        home.path(),
        "pi",
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"first, kept on a held store"}"#,
        &[("OBOETE_ANSWER_BY_MS", &past)],
    );
    assert!(kept.status.success());
    lock.execute_batch("ROLLBACK").unwrap();
    let ahead = (epoch_ms() + 2_000).to_string();
    let second = hook_output_with(
        home.path(),
        "pi",
        "UserPromptSubmit",
        r#"{"session_id":"s","prompt":"second, on a free store"}"#,
        &[("OBOETE_ANSWER_BY_MS", &ahead)],
    );
    assert!(second.status.success());
    let both = kept_text(home.path());
    assert!(
        both.contains("first, kept on a held store") && both.contains("second, on a free store"),
        "{both}"
    );
    let third = hook_output(
        home.path(),
        "pi",
        "PostToolUse",
        r#"{"session_id":"s","tool_name":"read","tool_input":{},"tool_response":"third"}"#,
    );
    assert!(third.status.success());
    let left = kept_text(home.path());
    assert!(
        !left.contains("first, kept") && !left.contains("second, on"),
        "all written back: {left}"
    );
    let raw = rusqlite::Connection::open_with_flags(
        home.path().join("raw.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let order: Vec<String> = raw
        .prepare(
            "SELECT CAST(body AS TEXT) FROM records WHERE body LIKE '%first, kept%'
             OR body LIKE '%second, on%' OR body LIKE '%third%' ORDER BY seq",
        )
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|b| b.unwrap())
        .collect();
    let at = |what: &str| order.iter().position(|b| b.contains(what)).expect(what);
    assert!(
        at("first, kept") < at("second, on") && at("second, on") < at("third"),
        "{order:?}"
    );
}

#[test]
fn session_start_note_says_in_japanese_that_there_is_no_memory_yet() {
    let home = tempfile::tempdir().unwrap();
    japanese(home.path(), "");
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    assert_eq!(
        only_the_work_state_section(&out.stdout)["systemMessage"],
        "oboete: 記録は有効です。このリポジトリには、まだ渡せる記憶がありません。画面を開くには oboete view --open"
    );
}

#[test]
fn session_start_note_reports_injection_off_in_japanese() {
    let home = tempfile::tempdir().unwrap();
    japanese(home.path(), "[inject]\nsession_start = false\n");
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"\"},\"systemMessage\":\"oboete: 記録は有効です。セッション開始時の記憶の受け渡しはオフになっています。\"}\n"
    );
}

#[test]
fn session_start_note_can_be_disabled_leaving_only_what_the_agent_gets() {
    let home = tempfile::tempdir().unwrap();
    for enabled in [true, false] {
        std::fs::write(
            home.path().join("config.toml"),
            format!("[inject]\nsession_start = {enabled}\nsession_start_note = false\n"),
        )
        .unwrap();
        let out = hook_output(
            home.path(),
            "claude",
            "SessionStart",
            r#"{"session_id":"s","source":"startup"}"#,
        );
        assert!(out.status.success());
        if enabled {
            let v = only_the_work_state_section(&out.stdout);
            assert!(v.get("systemMessage").is_none(), "{v}");
        } else {
            assert_eq!(out.stdout, b"");
        }
    }
}

/// Nothing is injected then, and no line says what the hook cannot know.
#[test]
fn inject_settings_that_do_not_load_give_no_note() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[inject]\nsession_start_chars = 999\n[summary]\nlanguage = \"Japanese\"\n",
    )
    .unwrap();
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nothing injected"),
        "{out:?}"
    );
}

/// A store that cannot be read is not an empty one: the line says so instead of "no memory yet"
/// (Codex on #361). Recording still works, so no failure line stands in for it.
#[test]
fn session_start_note_says_when_memory_could_not_be_read() {
    let home = tempfile::tempdir().unwrap();
    japanese(home.path(), "");
    std::fs::create_dir(home.path().join("knowledge.db")).unwrap();
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("manifest not read"), "{stderr}");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"\"},\"systemMessage\":\"oboete: 記録は有効です。記憶を読み出せませんでした。oboete doctor で状態を確認できます。\"}\n"
    );
}

/// A resumed session already carries its memory: SessionStart is no injection point then, and
/// says nothing, as before.
#[test]
fn a_resumed_session_gets_no_note() {
    let home = tempfile::tempdir().unwrap();
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"resume"}"#,
    );
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
}

#[test]
fn session_start_note_disabled_keeps_a_recording_failure_fail_open() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[inject]\nsession_start_note = false\n",
    )
    .unwrap();
    std::fs::create_dir(home.path().join("raw.db")).unwrap();
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["hookSpecificOutput"]
    );
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .starts_with("oboete: recording has failed since ")
    );
}

#[test]
fn session_start_note_is_never_in_a_replay() {
    let home = tempfile::tempdir().unwrap();
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home.path())
        .args(["hook", "claude", "SessionStart"])
        .env("OBOETE_REPLAY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"s","source":"startup"}"#)
        .unwrap();
    let out = hook.wait_with_output().unwrap();
    assert!(out.status.success());
    let v = only_the_work_state_section(&out.stdout);
    assert!(v.get("systemMessage").is_none(), "{v}");
}

#[test]
fn a_hooks_output_ends_when_the_hook_does_not_when_its_worker_does() {
    let home = tempfile::tempdir().unwrap();
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home.path())
        .args(["hook", "claude", "UserPromptSubmit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"s","prompt":"a prompt that starts the worker"}"#)
        .unwrap();
    let start = Instant::now();
    let out = hook.wait_with_output().unwrap();
    let ended = start.elapsed();
    assert!(out.status.success());
    // The worker holds its lock while it waits for records (60 s idle): held now, it was running
    // when the output ended.
    let held = || {
        std::fs::File::open(home.path().join("state").join("worker.lock"))
            .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
    };
    let until = Instant::now() + Duration::from_secs(5);
    while !held() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held(), "no worker was running");
    assert!(ended < Duration::from_secs(20), "{ended:?}");
}
