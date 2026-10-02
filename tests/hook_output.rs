//! An agent reads a hook's output to its end. A hook that starts the worker must not leave that
//! output open for the worker's lifetime: Windows passes a process's inheritable handles on to
//! its children, the hook's own stdout among them (60 s measured on Windows 11).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn hook_output(
    home: &std::path::Path,
    agent: &str,
    event: &str,
    input: &str,
) -> std::process::Output {
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(["hook", agent, event])
        // No worker is wanted for a hook's output.
        .env("OBOETE_NO_SPAWN", "1")
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

#[test]
fn session_start_note_says_in_japanese_that_there_is_no_memory_yet() {
    let home = tempfile::tempdir().unwrap();
    let out = hook_output(
        home.path(),
        "claude",
        "SessionStart",
        r#"{"session_id":"s","source":"startup"}"#,
    );
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"\"},\"systemMessage\":\"oboete: 記録は有効です。このリポジトリには、まだ渡せる記憶がありません。画面を開くには oboete view --open\"}\n"
    );
}

#[test]
fn session_start_note_reports_injection_off_in_japanese() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[inject]\nsession_start = false\n",
    )
    .unwrap();
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
fn session_start_note_can_be_disabled_without_changing_the_old_empty_output() {
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
        assert_eq!(out.stdout, b"", "session_start = {enabled}");
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
fn session_start_note_keeps_replay_hooks_silent_without_memory() {
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
    assert_eq!(out.stdout, b"");
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
