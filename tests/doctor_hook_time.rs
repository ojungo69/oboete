//! Doctor names the hooks that were slow (docs/hook-time.md), with `[inject]` right or wrong: the
//! record is every hook's, not the injection's.

#[test]
fn doctor_names_the_slow_hooks_whatever_inject_holds() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    // One hook, seen still running at 2 s in the prompt point, with no end after it.
    std::fs::write(
        state.join("hook-time.jsonl"),
        format!(
            "\n{{\"ts\":{now},\"pid\":7,\"agent\":\"claude\",\"event\":\"UserPromptSubmit\",\
             \"at\":\"slow\",\"ms\":2001,\"before_ms\":3,\"phase\":\"prompt\",\
             \"began\":{{\"parse\":0,\"prompt\":40}}}}\n"
        ),
    )
    .unwrap();
    let doctor = || {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .args(["--home", &home.path().to_string_lossy(), "doctor"])
            .env("OBOETE_NO_SPAWN", "1")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let line = "slow hooks: 1 on record in the last 7 days still ran at 2 s, 1 of them never \
                ended (cancelled or killed); most often in: prompt";
    let text = doctor();
    assert!(text.contains(line), "{text}");
    // A wait out of its range: the injection's settings do not load.
    std::fs::write(
        home.path().join("config.toml"),
        "[inject]\nper_prompt_wait_ms = 5\n",
    )
    .unwrap();
    let text = doctor();
    assert!(text.contains("injection: settings are wrong"), "{text}");
    assert!(text.contains(line), "{text}");
}
