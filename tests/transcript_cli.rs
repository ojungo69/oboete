//! Transcript import's command-line preview leaves the destination untouched (Task 9, spec 7.4).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn oboete(home: &Path, root: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("CODEX_HOME", root.join("codex"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn fixture(root: &Path) {
    let sessions = root.join("codex/sessions/2026/09/02");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/transcripts/codex-basic.jsonl"),
        sessions.join("rollout-cli-test.jsonl"),
    )
    .unwrap();
}

fn report(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let last = out
        .stdout
        .rsplit(|b| *b == b'\n')
        .find(|line| !line.is_empty())
        .expect("the final output line is the JSON report");
    serde_json::from_slice(last).unwrap()
}

fn assert_fixture_counts(report: &serde_json::Value) {
    let codex = &report["agents"]["codex"];
    assert_eq!(codex["files"], 1);
    assert_eq!(codex["sessions"], 1);
    assert_eq!(codex["events"], 8);
    assert!(codex["bytes"].as_u64().unwrap() > 0);
}

fn tree(path: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn read(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                entries.insert(relative, None);
                read(root, &path, entries);
            } else {
                entries.insert(relative, Some(std::fs::read(path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    read(path, path, &mut entries);
    entries
}

#[test]
fn preview_reports_counts_without_creating_a_home() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let home = root.path().join("new-home");
    let source = tree(&root.path().join("codex"));
    let out = oboete(
        &home,
        root.path(),
        &["import", "transcripts", "--agent", "codex"],
        "",
    );
    assert_fixture_counts(&report(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--yes"));
    assert!(!home.exists(), "preview created the destination home");
    assert_eq!(tree(&root.path().join("codex")), source);
}

#[test]
fn preview_leaves_an_existing_store_and_every_file_unchanged() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let home = root.path().join("home");
    let payload = serde_json::json!({
        "session_id": "seed-live",
        "cwd": root.path(),
        "prompt": "keep this live record",
    });
    let seed = oboete(
        &home,
        root.path(),
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    assert!(seed.status.success());
    assert!(home.join("raw.db").exists());
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    std::fs::write(home.join("state/keep.txt"), "keep this file\n").unwrap();
    let before = tree(&home);
    let out = oboete(
        &home,
        root.path(),
        &["import", "transcripts", "--agent", "codex"],
        "",
    );
    assert_fixture_counts(&report(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--yes"));
    assert_eq!(tree(&home), before, "preview changed the destination home");
}

#[test]
fn yes_imports_the_fixture_once_and_leaves_its_source_unchanged() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let home = root.path().join("home");
    let source = tree(&root.path().join("codex"));
    let args = ["import", "transcripts", "--agent", "codex", "--yes"];
    let first = oboete(&home, root.path(), &args, "");
    assert_fixture_counts(&report(&first));
    assert!(home.join("raw.db").exists());
    let again = oboete(&home, root.path(), &args, "");
    assert_eq!(report(&again)["agents"]["codex"]["events"], 0);
    assert_eq!(tree(&root.path().join("codex")), source);
}

#[test]
fn refused_files_do_not_block_later_files_and_exit_nonzero() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let day = root.path().join("codex/sessions/2026/09/02");
    let refused = day.join("rollout-cli-test.jsonl");
    let original = std::fs::read_to_string(&refused).unwrap();
    let home = root.path().join("home");
    let args = ["import", "transcripts", "--agent", "codex", "--yes"];
    assert_fixture_counts(&report(&oboete(&home, root.path(), &args, "")));
    std::fs::write(
        &refused,
        original.replace(
            "Add a 50ms timeout to fetchJson",
            "Rewrite the earlier prompt",
        ),
    )
    .unwrap();
    std::fs::write(
        day.join("rollout-z-good.jsonl"),
        original.replace(
            "22222222-2222-4222-8222-222222222222",
            "44444444-4444-4444-8444-444444444444",
        ),
    )
    .unwrap();
    for events in [8, 0] {
        let out = oboete(&home, root.path(), &args, "");
        assert!(!out.status.success());
        let raw = rusqlite::Connection::open_with_flags(
            home.join("raw.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let records: i64 = raw
            .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(records, 16);
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            said.contains(refused.file_name().unwrap().to_str().unwrap()),
            "{said}"
        );
        let counts: serde_json::Value = serde_json::from_str(said.lines().last().unwrap()).unwrap();
        assert_eq!(counts["agents"]["codex"]["files"], 2);
        assert_eq!(counts["agents"]["codex"]["refused"], 1);
        assert_eq!(counts["agents"]["codex"]["events"], events);
    }
}

#[test]
fn two_files_with_one_session_are_imported_once() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let day = root.path().join("codex/sessions/2026/09/02");
    let original = std::fs::read_to_string(day.join("rollout-cli-test.jsonl")).unwrap();
    std::fs::write(
        day.join("rollout-copy.jsonl"),
        original.replace(
            "Add a 50ms timeout to fetchJson",
            "Continue the forked session",
        ),
    )
    .unwrap();
    let source = tree(&root.path().join("codex"));
    let home = root.path().join("home");
    let args = ["import", "transcripts", "--agent", "codex", "--yes"];
    for (events, seen) in [(8, 8), (0, 16)] {
        let out = oboete(&home, root.path(), &args, "");
        let counts = report(&out);
        let codex = &counts["agents"]["codex"];
        assert_eq!(codex["files"], 2);
        assert_eq!(codex["sessions"], 1);
        assert_eq!(codex["events"], events);
        assert_eq!(codex["seen"], seen);
        assert_eq!(codex["refused"], 0);
    }
    assert_eq!(tree(&root.path().join("codex")), source);
}

/// docs/claude-mem-import.md I1: the everyday store takes claude-mem's history; a store named an
/// evaluation store is refused there, before anything is opened.
#[test]
fn claude_mem_imports_into_the_everyday_store_and_not_as_an_evaluation_store() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join(".oboete");
    let db = root.path().join("claude-mem.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE TABLE sdk_sessions(id INTEGER PRIMARY KEY, content_session_id TEXT NOT NULL,
               memory_session_id TEXT, project TEXT);
             CREATE TABLE observations(id INTEGER PRIMARY KEY, memory_session_id TEXT,
               project TEXT, created_at_epoch INTEGER, type TEXT, title TEXT, narrative TEXT,
               facts TEXT);
             CREATE TABLE session_summaries(id INTEGER PRIMARY KEY, memory_session_id TEXT,
               project TEXT, created_at_epoch INTEGER, request TEXT, investigated TEXT,
               learned TEXT, completed TEXT, next_steps TEXT);
             CREATE TABLE user_prompts(id INTEGER PRIMARY KEY, content_session_id TEXT,
               created_at_epoch INTEGER, prompt_text TEXT);
             INSERT INTO observations VALUES(1, NULL, 'p', 1000, 'change', 'Invented',
               'An invented body.', '[]');",
        )
        .unwrap();
    let db = db.to_str().unwrap();
    let out = oboete(
        &home,
        root.path(),
        &["import", "claude-mem", db, "--eval-store"],
        "",
    );
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("everyday one"), "{said}");
    assert!(!home.join("raw.db").exists());
    let out = oboete(&home, root.path(), &["import", "claude-mem", db], "");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    let stats: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(stats["observations"], 1);
}

#[test]
fn claude_mem_still_requires_a_database_path() {
    let root = tempfile::tempdir().unwrap();
    let out = oboete(
        &root.path().join("home"),
        root.path(),
        &["import", "claude-mem", "--eval-store"],
        "",
    );
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(said.contains("database") || said.contains("<db>"), "{said}");
}

#[test]
fn source_specific_arguments_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    for (args, expected) in [
        (
            vec!["import", "transcripts", "unexpected.db"],
            "takes no database path",
        ),
        (
            vec!["import", "claude-mem", "missing.db", "--yes"],
            "--yes is only for import transcripts",
        ),
        (
            vec!["import", "claude-mem", "missing.db", "--agent", "codex"],
            "--agent is only for import transcripts",
        ),
        (
            vec!["import", "transcripts", "--agent", "grok"],
            "invalid value",
        ),
    ] {
        let out = oboete(&home, root.path(), &args, "");
        assert!(!out.status.success());
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains(expected), "{said}");
    }
}
