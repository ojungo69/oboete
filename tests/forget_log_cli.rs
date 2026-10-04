//! A corrupt request-log line is skipped within the documented memory bound, through the CLI.
#![cfg(target_os = "linux")]

use std::io::{Seek, Write};
use std::process::{Command, Stdio};

#[test]
fn an_oversized_log_line_keeps_the_following_request_with_bounded_memory() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(h.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let projects = h.join("claude/projects/synthetic");
    std::fs::create_dir_all(&projects).unwrap();
    let canary = "bounded-log-native-record-805";
    let event = serde_json::json!({
        "type":"user", "sessionId":"bounded-log-session", "cwd":"/synthetic",
        "timestamp":"2026-09-01T00:00:00.100Z",
        "message":{"role":"user", "content":canary}
    });
    std::fs::write(projects.join("source.jsonl"), format!("{event}\n")).unwrap();
    let run = |args: &[&str], limited: bool| {
        let mut command = if limited {
            // util-linux limits only this synthetic child, before it opens the log.
            let mut command = Command::new("prlimit");
            command.args(["--as=134217728", "--", env!("CARGO_BIN_EXE_oboete")]);
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_oboete"))
        };
        command
            .arg("--home")
            .arg(h)
            .args(args)
            .current_dir(h)
            .env("HOME", h)
            .env("USERPROFILE", h)
            .env("CODEX_HOME", h.join("codex"))
            .env("CLAUDE_CONFIG_DIR", h.join("claude"))
            .env("OBOETE_NO_SPAWN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.output().unwrap()
    };
    let imported = run(
        &["import", "transcripts", "--agent", "claude", "--yes"],
        false,
    );
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let indexed = run(&["worker", "--idle-ms", "0"], false);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    let search = run(&["search", "--all", "--raw", "only", "--", canary], false);
    assert!(
        search.status.success(),
        "{}",
        String::from_utf8_lossy(&search.stderr)
    );
    let hits = String::from_utf8(search.stdout).unwrap();
    let id = hits
        .split_whitespace()
        .next()
        .expect("the native record is searchable");
    let snapshot = h.join("before-forget.db");
    std::fs::copy(h.join("raw.db"), &snapshot).unwrap();
    let started = run(&["forget", "--record", id, "--yes"], false);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let path = h.join("forget.log");
    let request = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&request).contains(canary));
    let mut log = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    log.set_len(192 << 20).unwrap();
    log.seek(std::io::SeekFrom::End(0)).unwrap();
    log.write_all(b"\n").unwrap();
    log.write_all(&request).unwrap();
    log.sync_all().unwrap();
    drop(log);
    // F2 with one surviving log: the following valid line is the only remaining request.
    std::fs::remove_file(h.join("backups/forget.log")).unwrap();
    let replacement = h.join("replacement.db");
    std::fs::copy(snapshot, &replacement).unwrap();
    std::fs::rename(replacement, h.join("raw.db")).unwrap();
    let status = run(&["forget", "--status"], true);
    assert!(
        status.status.success(),
        "an oversized line exhausted the capped CLI: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let shown = String::from_utf8(status.stdout).unwrap();
    let jobs: Vec<serde_json::Value> = shown
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        jobs.len(),
        1,
        "the valid request after the damaged line was lost"
    );
    assert_eq!(jobs[0]["records"], 1);
    let warnings = String::from_utf8(status.stderr).unwrap();
    assert!(
        warnings.contains("line 1") && warnings.contains("cap"),
        "{warnings}"
    );
}
