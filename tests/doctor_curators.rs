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
    for db in ["knowledge.db", "providers.db"] {
        std::fs::write(
            home.path().join(db),
            "not a database, one of two damaged stores",
        )
        .unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for db in ["knowledge.db", "providers.db"] {
        assert!(text.contains(&format!("cannot read {db}")), "{db}: {text}");
    }
    assert!(text.contains("providers (chain order):"), "{text}");
    assert!(!out.status.success());
}

/// Decision 99 (2026-11-12): a store of oboete v1 is one line that says this build no longer
/// migrates it, with a build that still does. Doctor does not open it: it is left as it was, and
/// one that is not a database is not "cannot read".
#[test]
fn doctor_names_a_v1_store_in_one_line_and_leaves_it_as_it_was() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("oboete.db");
    std::fs::write(&db, "not a database: doctor never opens it").unwrap();
    let bytes = std::fs::read(&db).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("oboete.db")).collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(
        lines[0].starts_with("  oboete.db: a store of oboete v1, which this build no longer migrates: migrate it with the trial build v0.1.0-trial.2, "),
        "{text}"
    );
    assert!(
        lines[0].ends_with("(`oboete migrate`, then `oboete migrate --finish`)"),
        "{text}"
    );
    assert!(!text.contains("cannot read"), "{text}");
    assert!(!text.contains("v1 events not migrated"), "{text}");
    assert_eq!(std::fs::read(&db).unwrap(), bytes);
}

/// The home's evaluation copies (`<home>/eval`), which forget does not reach, are one line with
/// their size: the bytes of the files under it, at any depth.
#[test]
fn doctor_names_the_evaluation_copies_with_their_size() {
    let home = tempfile::tempdir().unwrap();
    let eval = home.path().join("eval");
    std::fs::create_dir_all(eval.join("run-1")).unwrap();
    std::fs::write(eval.join("a.jsonl"), "x".repeat(300)).unwrap();
    std::fs::write(eval.join("run-1").join("b.db"), "y".repeat(200)).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains("evaluation copies"))
        .unwrap_or_else(|| panic!("no evaluation copies line in {text}"));
    assert_eq!(
        line,
        format!(
            "  evaluation copies, which forget does not reach: {} (500 bytes)",
            eval.display()
        )
    );
}

/// #542: doctor shows the home's own provider calls from providers.db, whatever their role, with
/// no v1 store beside them.
#[test]
fn doctor_shows_the_homes_own_provider_calls() {
    let home = tempfile::tempdir().unwrap();
    let oboete = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .args(["--home", &home.path().to_string_lossy()])
            .args(args)
            .env("OBOETE_NO_SPAWN", "1")
            .output()
            .unwrap()
    };
    // `resume` opens providers.db, which makes its tables.
    oboete(&["resume", "groq"]);
    rusqlite::Connection::open(home.path().join("providers.db"))
        .unwrap()
        .execute(
            "INSERT INTO provider_calls(ts, provider, role, outcome, ms) VALUES(1, 'invented', 'summary', 'ok', 7)",
            [],
        )
        .unwrap();
    let text = String::from_utf8_lossy(&oboete(&["doctor"]).stdout).into_owned();
    assert!(!home.path().join("oboete.db").exists());
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let header = lines
        .iter()
        .position(|l| *l == "  last provider calls (providers.db):")
        .unwrap_or_else(|| panic!("no providers.db calls in {text}"));
    assert_eq!(lines[header + 1], "    invented summary ok 7ms");
    assert!(!text.contains("v1's last provider calls"), "{text}");
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
        text.contains("  warning: no provider is enabled:"),
        "{text}"
    );
    for cause_or_remedy in [
        "entries are individually disabled or their name groups are off",
        "enable an entry and its name group to curate",
        "set [summary] curate = false to stop curation",
    ] {
        assert!(text.contains(cause_or_remedy), "{text}");
    }
    assert!(
        text.contains("  warning: [chain] timeout_s: no chain entry is named \"nobody\""),
        "{text}"
    );
    let native_off = entries.replace("kind = \"cli\"", "kind = \"cli\"\nenabled = false");
    let native_off = doctor(&format!("[summary]\ncurate = true\n{native_off}"));
    assert!(
        native_off.contains("  warning: no provider is enabled:"),
        "{native_off}"
    );
    assert!(
        native_off.contains("entries are individually disabled"),
        "{native_off}"
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

/// Codex on #440: a spool of kept hook calls doctor cannot list is said, and fails the report,
/// rather than counted as empty.
#[test]
fn doctor_says_when_the_kept_hook_calls_cannot_be_listed() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("unwritten"), "not a directory").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "doctor"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("the hook calls kept after failed writes cannot be listed"),
        "{text}"
    );
    assert!(!out.status.success());
}
