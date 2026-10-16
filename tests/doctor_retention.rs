//! Doctor reads `[retention]` with or without a store (docs/retention.md; Codex on #552): a wrong
//! table expires nothing, so it fails doctor before the first capture too.

#[test]
fn doctor_fails_on_a_wrong_retention_table_before_any_store() {
    let home = tempfile::tempdir().unwrap();
    let doctor = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .args(["--home", &home.path().to_string_lossy(), "doctor"])
            .env("OBOETE_NO_SPAWN", "1")
            .output()
            .unwrap()
    };
    std::fs::write(home.path().join("config.toml"), "[retention]\ndays = 0\n").unwrap();
    let out = doctor();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!home.path().join("raw.db").exists(), "{text}");
    assert!(text.contains("nothing expires until it loads"), "{text}");
    assert!(!out.status.success(), "{text}");
    std::fs::write(home.path().join("config.toml"), "[retention]\ndays = 30\n").unwrap();
    let out = doctor();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("retention: 30 days ("), "{text}");
}
