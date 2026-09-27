//! Doctor says which configured curator CLIs are never used: those with no proven no-tool mode
//! (spec 6.5), before any call has tried them.

#[test]
fn doctor_names_a_cli_curator_that_is_always_skipped() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[[providers]]\nkind = \"cli\"\nname = \"agy\"\ncli = \"agy\"\n\n\
         [[providers]]\nkind = \"cli\"\nname = \"codex\"\ncli = \"codex\"\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let providers = text.split_once("providers (chain order):").unwrap().1;
    let line = |name: &str| {
        providers
            .lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no {name} line in {text}"))
            .to_owned()
    };
    assert!(
        line("agy")
            .ends_with("skipped as a curator (isolation: this CLI has no proven no-tool mode)"),
        "{text}"
    );
    assert!(!line("codex").contains("skipped as a curator"), "{text}");
}

/// A store doctor cannot read is one line and a failed exit, not the end of the report: the
/// sections after it are still printed (doctor is needed most when a store is damaged).
#[test]
fn doctor_reports_past_a_store_it_cannot_read() {
    let home = tempfile::tempdir().unwrap();
    for db in ["knowledge.db", "oboete.db", "providers.db"] {
        std::fs::write(
            home.path().join(db),
            "not a database, one of three damaged stores",
        )
        .unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for db in ["knowledge.db", "oboete.db", "providers.db"] {
        assert!(text.contains(&format!("cannot read {db}")), "{db}: {text}");
    }
    assert!(text.contains("providers (chain order):"), "{text}");
    assert!(!out.status.success());
}
