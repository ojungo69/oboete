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
