//! `oboete migrate` (milestone 4 Task 9, spec 7.4): a pass from the command line, a rerun that
//! imports nothing, and `--finish` that deletes nothing without a yes.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn oboete(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.to_string_lossy()])
        .args(args)
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn migrate_imports_once_and_finish_asks_first() {
    let home = tempfile::tempdir().unwrap();
    let v1 = rusqlite::Connection::open(home.path().join("oboete.db")).unwrap();
    v1.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL,
           cwd TEXT, started_at INTEGER NOT NULL, last_event_at INTEGER NOT NULL);
         CREATE TABLE session_repos(session_id TEXT NOT NULL, repo TEXT NOT NULL,
           PRIMARY KEY(session_id, repo)) WITHOUT ROWID;
         CREATE TABLE events(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
           event TEXT NOT NULL, ts INTEGER NOT NULL, payload TEXT NOT NULL);
         CREATE TABLE observations(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT,
           ts INTEGER, kind TEXT, title TEXT, body TEXT, uid TEXT);
         CREATE TABLE summaries(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT, ts INTEGER,
           body TEXT, uid TEXT);
         CREATE TABLE prompts(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT, ts INTEGER,
           body TEXT, uid TEXT);
         INSERT INTO meta VALUES('device_id', 'd1e5');
         INSERT INTO sessions VALUES('s1', 'claude', 'github.com/o/r', '/w', 100, 100);
         INSERT INTO events(session_id, event, ts, payload)
           VALUES('s1', 'UserPromptSubmit', 110, '{\"prompt\": \"use tabs\"}');",
    )
    .unwrap();
    drop(v1);
    let first = oboete(home.path(), &["migrate"], "");
    let said = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(said.contains("[summary] curate is not set"), "{said}");
    assert!(said.contains("\"events\":1,\"records\":1"), "{said}");
    let again = oboete(home.path(), &["migrate"], "");
    let said = String::from_utf8_lossy(&again.stdout);
    assert!(said.contains("\"events\":0,\"records\":0"), "{said}");
    let finish = oboete(home.path(), &["migrate", "--finish"], "no\n");
    let said = String::from_utf8_lossy(&finish.stdout);
    assert!(finish.status.success(), "{said}");
    assert!(said.ends_with("Nothing was deleted.\n"), "{said}");
    assert!(home.path().join("oboete.db").exists());
    // `--finish` reads the home's own store only: the files it deletes are the home's.
    let elsewhere = home.path().join("elsewhere.db");
    std::fs::copy(home.path().join("oboete.db"), &elsewhere).unwrap();
    let args = [
        "migrate",
        "--finish",
        "--from",
        &elsewhere.to_string_lossy(),
    ];
    let refused = oboete(home.path(), &args, "yes\n");
    assert!(!refused.status.success() && home.path().join("oboete.db").exists());
    let doctor = oboete(home.path(), &["doctor"], "");
    let said = String::from_utf8_lossy(&doctor.stdout);
    assert!(said.contains("v1 events not migrated yet: 0"), "{said}");
}
