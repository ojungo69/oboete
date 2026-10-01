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
    assert_eq!(codex["events"], 9);
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
fn claude_mem_keeps_its_everyday_store_refusal() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join(".oboete");
    for args in [
        vec!["import", "claude-mem", "missing.db"],
        vec!["import", "claude-mem", "missing.db", "--eval-store"],
    ] {
        let out = oboete(&home, root.path(), &args, "");
        assert!(!out.status.success());
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains("everyday store"), "{said}");
    }
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
