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

/// The legacy oboete.db section counts each of its tables, and a readable store is healthy and
/// left as it was (doctor opens it read only, spec 7.4).
#[test]
fn doctor_counts_the_tables_of_a_legacy_store() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("oboete.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES('device_id', 'd1');
             CREATE TABLE sessions(id TEXT); CREATE TABLE events(id INTEGER);
             CREATE TABLE observations(id INTEGER); CREATE TABLE summaries(id INTEGER);
             CREATE TABLE provider_calls(id INTEGER PRIMARY KEY, ts INTEGER, provider TEXT,
               outcome TEXT, ms INTEGER, detail TEXT);",
        )
        .unwrap();
    let bytes = std::fs::read(&db).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("sessions 0 | raw events 0 | observations 0 | summaries 0"),
        "{text}"
    );
    assert!(!text.contains("cannot read oboete.db"), "{text}");
    assert!(text.contains("v1 events not migrated yet: 0"), "{text}");
    assert_eq!(std::fs::read(&db).unwrap(), bytes);
}

/// #94: doctor prints `[inject]` as it applies, an entry `[chain]` turns off, and `[chain]`'s
/// warnings, each under the provider list.
#[test]
fn doctor_prints_inject_and_chain_as_they_apply() {
    let home = tempfile::tempdir().unwrap();
    let doctor = |config: &str| {
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .args(["--home", &home.path().to_string_lossy(), "doctor"])
            .env("OBOETE_NO_SPAWN", "1")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let entries = "[[providers]]\nkind = \"cli\"\nname = \"a\"\ncli = \"claude\"\n\n\
                   [[providers]]\nkind = \"cli\"\nname = \"b\"\ncli = \"codex\"\n";
    let text = doctor(&format!(
        "[summary]\ncurate = true\n{entries}[inject]\nsession_start_chars = 2000\n\
         [chain]\noff = [\"a\", \"b\"]\ntimeout_s = {{ nobody = 3 }}\n"
    ));
    assert!(
        text.contains("injection: the manifest, up to 2000 characters"),
        "{text}"
    );
    // Task 8's settings, at their defaults.
    assert!(text.contains("per-prompt injection: off\n"), "{text}");
    assert!(
        text.contains("corrections: on, up to 800 characters at the next prompt"),
        "{text}"
    );
    let providers = text.split_once("providers (chain order):").unwrap().1;
    let mut lines = providers.lines();
    lines
        .find(|l| l.trim_start().starts_with("a "))
        .unwrap_or_else(|| panic!("no a line in {text}"));
    let detail = lines.next().unwrap_or_default();
    assert!(detail.trim_start().starts_with("off, "), "{text}");
    assert!(
        text.contains("  warning: every chain entry is off, so nothing is curated"),
        "{text}"
    );
    assert!(
        text.contains("  warning: [chain] timeout_s: no chain entry is named \"nobody\""),
        "{text}"
    );
    let off = doctor(&format!(
        "{entries}[inject]\nsession_start = false\nper_prompt = true\nper_prompt_chars = 900\ncorrection = false\n"
    ));
    assert!(
        off.contains("injection: off, no manifest is shown to an agent"),
        "{off}"
    );
    assert!(
        off.contains("per-prompt injection: on, up to 900 characters a prompt"),
        "{off}"
    );
    assert!(off.contains("corrections: off\n"), "{off}");
    assert!(!off.contains("warning:"), "{off}");
    let wrong = doctor(&format!("{entries}[inject]\nsession_start = \"false\"\n"));
    // With the file it is about, as the capture line gives it (OpenCodeReview on #267).
    assert!(
        wrong.contains("injection: settings are wrong, so nothing is injected: parse "),
        "{wrong}"
    );
}
