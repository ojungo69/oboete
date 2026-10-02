//! Milestone 5's first vertical: durable refusal to return or import a forgotten raw record.
//! Every store, transcript and child configuration lives in a synthetic temporary home.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const CANARY: &str = "forget-canary-amethyst-92741";

fn command(home: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_oboete"));
    c.arg("--home")
        .arg(home)
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn run(home: &Path, args: &[&str], input: &str) -> Output {
    let mut child = command(home, args).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn hook_record(home: &Path, body: &str) -> String {
    let payload = serde_json::json!({"session_id":"forget-session", "cwd":home, "prompt":body});
    ok(run(
        home,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    ));
    ok(run(home, &["worker", "--idle-ms", "0"], ""));
    let hits = ok(run(
        home,
        &["search", "--all", "--raw", "only", "--", body],
        "",
    ));
    hits.split_whitespace()
        .next()
        .expect("the raw hit has an id")
        .to_owned()
}

#[test]
fn a_record_without_native_provenance_is_refused_before_registration() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = hook_record(home, CANARY);
    let result = run(home, &["forget", "--record", &id, "--yes"], "");
    assert!(
        !result.status.success(),
        "a record without a stable source identity was accepted"
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("native source identity"));
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    assert!(!home.join("privacy.db").exists());
}

fn record(home: &Path, body: &str) -> String {
    let source = home.join("native-source.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    db.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT OR IGNORE INTO meta VALUES('device_id','nativefixture');
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,agent TEXT,repo TEXT,cwd TEXT,started_at INTEGER,last_event_at INTEGER);
        CREATE TABLE IF NOT EXISTS session_repos(session_id TEXT,repo TEXT);
        CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY,session_id TEXT,event TEXT,ts INTEGER,payload TEXT);
        CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,kind TEXT,title TEXT,body TEXT);
        CREATE TABLE IF NOT EXISTS summaries(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,body TEXT);
        CREATE TABLE IF NOT EXISTS prompts(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,body TEXT);
        INSERT OR IGNORE INTO sessions VALUES('native-session','claude','github.com/test/privacy','/synthetic',100,110);").unwrap();
    let next: i64 = db
        .query_row("SELECT COALESCE(MAX(id),0)+1 FROM events", [], |r| r.get(0))
        .unwrap();
    db.execute(
        "INSERT INTO events VALUES(?1,'native-session','UserPromptSubmit',?2,?3)",
        rusqlite::params![
            next,
            100 + next,
            serde_json::json!({"prompt":body}).to_string()
        ],
    )
    .unwrap();
    drop(db);
    ok(run(
        home,
        &["migrate", "--from", source.to_str().unwrap()],
        "",
    ));
    ok(run(home, &["worker", "--idle-ms", "0"], ""));
    let hits = ok(run(
        home,
        &["search", "--all", "--raw", "only", "--", body],
        "",
    ));
    hits.split_whitespace().next().unwrap().to_owned()
}

#[test]
fn forgetting_a_record_hides_it_and_reports_the_unfinished_purge() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(home, CANARY);
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    let preview = ok(run(home, &["forget", "--record", &id], "no\n"));
    assert!(preview.contains("raw records: 1"), "{preview}");
    assert!(
        !home.join("privacy.db").exists(),
        "a rejected preview registered a request"
    );
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    let started = ok(run(home, &["forget", "--record", &id, "--yes"], ""));
    assert!(started.contains("pending_physical_purge"), "{started}");
    assert!(!String::from_utf8_lossy(&run(home, &["get", &id], "").stdout).contains(CANARY));
    let status = ok(run(home, &["forget", "--status"], ""));
    assert!(status.contains("pending_physical_purge"), "{status}");
    assert!(!status.contains("\"local\":\"done\""), "{status}");
    for name in ["privacy.db", "privacy.head"] {
        let bytes = std::fs::read(home.join(name)).unwrap();
        assert!(
            !bytes.windows(CANARY.len()).any(|s| s == CANARY.as_bytes()),
            "{name} kept content"
        );
    }
}

#[test]
fn losing_the_control_files_never_looks_like_an_empty_history() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(home, CANARY);
    ok(run(home, &["forget", "--record", &id, "--yes"], ""));
    std::fs::remove_file(home.join("privacy.db")).unwrap();
    std::fs::remove_file(home.join("privacy.head")).unwrap();
    let status = run(home, &["forget", "--status"], "");
    assert!(
        !status.status.success(),
        "missing deletion authority was reported as an empty history"
    );
    let get = run(home, &["get", &id], "");
    assert!(!get.status.success());
    assert!(!String::from_utf8_lossy(&get.stdout).contains(CANARY));
}

fn copy_backup(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for file in std::fs::read_dir(from).unwrap() {
        let file = file.unwrap();
        std::fs::copy(file.path(), to.join(file.file_name())).unwrap();
    }
}

#[test]
fn an_old_backup_and_changed_capture_rules_do_not_reimport_a_forgotten_v1_event() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let keep = record(&home, "unrelated-persisted-opal-731");
    let old_backup = root.path().join("old-backup");
    copy_backup(&home.join("backups"), &old_backup);
    let source = root.path().join("v1.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT INTO meta VALUES('device_id','olddevice');
        CREATE TABLE sessions(id TEXT PRIMARY KEY,agent TEXT,repo TEXT,cwd TEXT,started_at INTEGER,last_event_at INTEGER);
        CREATE TABLE session_repos(session_id TEXT,repo TEXT);
        CREATE TABLE events(id INTEGER PRIMARY KEY,session_id TEXT,event TEXT,ts INTEGER,payload TEXT);
        CREATE TABLE observations(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,kind TEXT,title TEXT,body TEXT);
        CREATE TABLE summaries(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,body TEXT);
        CREATE TABLE prompts(id INTEGER PRIMARY KEY,uid TEXT,session_id TEXT,repo TEXT,ts INTEGER,body TEXT);
        INSERT INTO sessions VALUES('v1-session','claude','github.com/test/privacy','/synthetic',100,110);").unwrap();
    db.execute(
        "INSERT INTO events VALUES(1,'v1-session','UserPromptSubmit',110,?1)",
        [serde_json::json!({"prompt":CANARY}).to_string()],
    )
    .unwrap();
    drop(db);
    let args = ["migrate", "--from", source.to_str().unwrap()];
    ok(run(&home, &args, ""));
    ok(run(&home, &["worker", "--idle-ms", "0"], ""));
    // An ordinary restore before deletion must preserve the source identity too.
    ok(run(&home, &["restore"], ""));
    let found = ok(run(
        &home,
        &["search", "--all", "--raw", "only", "--", CANARY],
        "",
    ));
    let id = found.split_whitespace().next().unwrap();
    ok(run(&home, &["forget", "--record", id, "--yes"], ""));
    std::fs::remove_dir_all(home.join("backups")).unwrap();
    copy_backup(&old_backup, &home.join("backups"));
    ok(run(&home, &["restore"], ""));
    assert!(ok(run(&home, &["get", &keep], "")).contains("unrelated-persisted-opal-731"));
    assert!(!String::from_utf8_lossy(&run(&home, &["get", id], "").stdout).contains(CANARY));
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n[redaction]\nextra_rules = [{id='amethyst',regex='amethyst'}]\n").unwrap();
    let imported = ok(run(&home, &args, ""));
    assert!(
        imported.contains("\"events\":1,\"records\":0"),
        "a forgotten source identity returned under changed capture rules: {imported}"
    );
    assert!(ok(run(&home, &["forget", "--status"], "")).contains("pending_physical_purge"));
}

#[test]
fn a_copied_transcript_keeps_its_forgotten_identity_under_new_redaction_rules() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    record(&home, "seed-for-backup-621");
    let old_backup = root.path().join("old-backup");
    copy_backup(&home.join("backups"), &old_backup);
    let sessions = home.join("codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let content = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/transcripts/codex-basic.jsonl"),
    )
    .unwrap()
    .replace("Add a 50ms timeout to fetchJson", CANARY)
    .replace("/work/svc", &home.to_string_lossy())
    .replace("/work/parent", &home.to_string_lossy());
    std::fs::write(sessions.join("rollout-source.jsonl"), &content).unwrap();
    let args = ["import", "transcripts", "--agent", "codex", "--yes"];
    let imported = ok(run(&home, &args, ""));
    assert!(imported.contains("\"events\":8"), "{imported}");
    ok(run(&home, &["worker", "--idle-ms", "0"], ""));
    let found = ok(run(
        &home,
        &["search", "--all", "--raw", "only", "--", CANARY],
        "",
    ));
    let id = found.split_whitespace().next().unwrap();
    ok(run(&home, &["forget", "--record", id, "--yes"], ""));
    std::fs::remove_dir_all(home.join("backups")).unwrap();
    copy_backup(&old_backup, &home.join("backups"));
    ok(run(&home, &["restore"], ""));
    std::fs::rename(
        sessions.join("rollout-source.jsonl"),
        sessions.join("rollout-copied.jsonl"),
    )
    .unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n[redaction]\nextra_rules = [{id='amethyst',regex='amethyst'}]\n").unwrap();
    let imported = ok(run(&home, &args, ""));
    assert!(
        imported.contains("\"events\":7"),
        "a forgotten transcript event returned under another path/ruleset: {imported}"
    );
}

#[test]
fn a_corrupt_raw_store_recovers_its_forget_from_the_separate_journal() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(home, CANARY);
    ok(run(home, &["forget", "--record", &id, "--yes"], ""));
    std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("raw.db"))
        .unwrap()
        .write_all(b"broken SQLite header")
        .unwrap();
    ok(run(home, &["worker", "--idle-ms", "0"], ""));
    assert!(!String::from_utf8_lossy(&run(home, &["get", &id], "").stdout).contains(CANARY));
    let jobs = ok(run(home, &["forget", "--status"], ""));
    assert_eq!(jobs.lines().count(), 1);
    assert!(jobs.contains("pending_physical_purge"));
}

#[test]
fn older_or_unknown_controls_are_errors_not_empty_deny_lists() {
    for broken in ["older", "unknown", "damaged"] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
        let first = record(&home, CANARY);
        ok(run(&home, &["forget", "--record", &first, "--yes"], ""));
        let old = root.path().join("older-control.db");
        std::fs::copy(home.join("privacy.db"), &old).unwrap();
        let second = record(&home, "second-private-sapphire-9284");
        ok(run(&home, &["forget", "--record", &second, "--yes"], ""));
        match broken {
            "older" => {
                std::fs::copy(&old, home.join("privacy.db")).unwrap();
            }
            "unknown" => {
                rusqlite::Connection::open(home.join("privacy.db"))
                    .unwrap()
                    .execute_batch("PRAGMA user_version=99")
                    .unwrap();
            }
            _ => {
                std::fs::write(home.join("privacy.db"), b"damaged privacy control").unwrap();
            }
        }
        assert!(
            !run(&home, &["forget", "--status"], "").status.success(),
            "{broken}"
        );
        let output = run(&home, &["get", &second], "");
        assert!(!output.status.success(), "{broken}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("second-private-sapphire-9284"));
        let before = std::fs::read(home.join("raw.db")).unwrap();
        assert!(!run(&home, &["restore"], "").status.success(), "{broken}");
        assert_eq!(
            std::fs::read(home.join("raw.db")).unwrap(),
            before,
            "{broken} changed the existing raw store"
        );
    }
}
