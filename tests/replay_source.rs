//! D9: nothing `oboete replay` records says the owner is at work, including the `oboete hook`
//! processes it spawns to time them.

#[test]
fn a_replay_and_its_spawned_hooks_record_no_hook_record() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = serde_json::json!({"agent": "claude", "event": "UserPromptSubmit",
        "ts": "2026-09-01T00:00:00.000Z",
        "payload": {"session_id": "s", "cwd": "__OBOETE_REPLAY_ROOT__", "prompt": "one"}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "replay"])
        .arg(&fixture)
        .args(["--spawn-sample", "2"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
    let count = |sql: &str| -> i64 { raw.query_row(sql, [], |r| r.get(0)).unwrap() };
    // The fixture's prompt and the two spawned samples.
    assert_eq!(
        count("SELECT COUNT(*) FROM records WHERE source = 'replay'"),
        3
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM records WHERE source = 'hook'"),
        0
    );
}

/// Milestone 4 Task 0 (OpenCodeReview on #301): `--read-sample` runs both arms through the hooks
/// it spawns, as `run` wires them: the cold arm shows nothing for the checkout, and the warm arm,
/// after the drain, shows its manifest from the spawned SessionStart and from the in-process read.
/// With no write samples, the samples' checkout has nothing to show, so a SessionStart sent there
/// prints nothing.
#[test]
fn the_read_arms_run_through_the_spawned_hooks() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = serde_json::json!({"agent": "claude", "event": "UserPromptSubmit",
        "ts": "2026-09-01T00:00:00.000Z",
        "payload": {"session_id": "s", "cwd": "__OBOETE_REPLAY_ROOT__",
                    "prompt": "Fix the flaky test first."}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "replay"])
        .arg(&fixture)
        .args(["--read-sample", "2", "--spawn-sample", "0"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let (cold, warm) = (&report["read"]["cold"], &report["read"]["warm"]);
    assert_eq!(
        (&cold["read_chars"], &cold["session_start_printed_bytes"]),
        (&0.into(), &0.into())
    );
    assert!(warm["read_chars"].as_u64().unwrap() > 0);
    assert!(warm["session_start_printed_bytes"].as_u64().unwrap() > 0);
    assert_eq!(warm["session_start_ms"]["n"], 2);
}
