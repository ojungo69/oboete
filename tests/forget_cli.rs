//! Milestone 5 slice 1 (docs/milestone-5-plan.md, D1): a forgotten imported record is hidden,
//! never imported again, and survives the loss of raw.db or of a request log; nothing of it stops a
//! hook. Every store, transcript and child configuration lives in a synthetic temporary home.

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
fn a_record_without_an_import_identity_is_refused_before_registration() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = hook_record(home, CANARY);
    let result = run(home, &["forget", "--record", &id, "--yes"], "");
    assert!(!result.status.success(), "a hook record was accepted");
    assert!(String::from_utf8_lossy(&result.stderr).contains("no import identity"));
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    assert!(ok(run(home, &["forget", "--status"], "")).is_empty());
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

/// D1 rules 1 to 3: the request is registered in raw.db, the record hidden, and both log copies
/// hold it without its text; forget says what it cannot reach, and only a terminal or `--yes`
/// confirms it.
#[test]
fn forgetting_a_record_hides_it_logs_it_without_text_and_says_what_it_cannot_reach() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(home, CANARY);
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    let piped = run(home, &["forget", "--record", &id], "yes\n");
    assert!(!piped.status.success(), "a pipe confirmed a forget");
    let said = String::from_utf8_lossy(&piped.stdout);
    assert!(said.contains("Raw records: 1"), "{said}");
    assert!(said.contains("Physical purge is not built yet"), "{said}");
    assert!(said.contains("the old oboete v1 store"), "{said}");
    assert!(String::from_utf8_lossy(&piped.stderr).contains("needs a terminal"));
    assert!(ok(run(home, &["forget", "--status"], "")).is_empty());
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
    let started = ok(run(home, &["forget", "--record", &id, "--yes"], ""));
    assert!(started.contains("physical purge pending"), "{started}");
    assert!(started.contains("Request logged in"), "{started}");
    assert!(!String::from_utf8_lossy(&run(home, &["get", &id], "").stdout).contains(CANARY));
    assert!(ok(run(home, &["forget", "--status"], "")).contains("physical purge pending"));
    for log in [home.join("forget.log"), home.join("backups/forget.log")] {
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1, "{}", log.display());
        assert!(!text.contains(CANARY), "{} kept the text", log.display());
    }
}

/// D1: a request log copy lost, older, of another home or damaged never stops a hook from
/// recording, the forget stays, and the next worker writes the copy whole again.
#[test]
fn a_lost_older_foreign_or_damaged_log_never_stops_a_hook() {
    for copy in ["forget.log", "backups/forget.log"] {
        for damage in ["lost", "older", "foreign", "damaged"] {
            let home = tempfile::tempdir().unwrap();
            let home = home.path();
            std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
            let first = record(home, CANARY);
            let log = home.join(copy);
            let before = std::fs::read(&log).ok();
            ok(run(home, &["forget", "--record", &first, "--yes"], ""));
            match damage {
                "lost" => std::fs::remove_file(&log).unwrap(),
                "older" => std::fs::write(&log, before.unwrap_or_default()).unwrap(),
                "foreign" => {
                    let text = std::fs::read_to_string(&log).unwrap();
                    let mut line: serde_json::Value =
                        serde_json::from_str(text.lines().next().unwrap()).unwrap();
                    line["request"]["home"] = "elsewhere".into();
                    std::fs::write(&log, format!("{line}\n")).unwrap();
                }
                _ => std::fs::write(&log, b"\x00not a request log\xff").unwrap(),
            }
            let id = hook_record(home, "recorded-after-the-damage-4417");
            assert!(ok(run(home, &["get", &id], "")).contains("recorded-after-the-damage-4417"));
            let get = run(home, &["get", &first], "");
            assert!(
                !String::from_utf8_lossy(&get.stdout).contains(CANARY),
                "{copy} {damage}"
            );
            assert_eq!(
                ok(run(home, &["forget", "--status"], "")).lines().count(),
                1,
                "{copy} {damage}"
            );
            let text = String::from_utf8_lossy(&std::fs::read(&log).unwrap()).into_owned();
            assert!(
                text.lines()
                    .any(|l| l.contains("\"home\"") && !l.contains("elsewhere")),
                "{copy} {damage}: the worker did not write the request back"
            );
        }
    }
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
    assert!(ok(run(&home, &["forget", "--status"], "")).contains("physical purge pending"));
}

/// A v1 record's transcript boundary survives in the bodyless request when a restore loses the
/// record itself. Earlier transcript history stays importable; the forgotten prompt does not.
#[test]
fn an_old_restore_keeps_the_forgotten_v1_sessions_transcript_cut() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    record(&home, "unrelated-session-before-the-forgotten-one-401");
    let old_backup = root.path().join("before-session");
    copy_backup(&home.join("backups"), &old_backup);
    let source = home.join("native-source.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    db.execute_batch(
        "INSERT INTO sessions VALUES('forgotten-cut-session','claude','github.com/test/privacy',
         '/synthetic',1788220800000,1788220800000);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO events VALUES(2,'forgotten-cut-session','UserPromptSubmit',1788220800000,?1)",
        [serde_json::json!({"prompt":CANARY}).to_string()],
    )
    .unwrap();
    drop(db);
    ok(run(
        &home,
        &["migrate", "--from", source.to_str().unwrap()],
        "",
    ));
    ok(run(&home, &["worker", "--idle-ms", "0"], ""));
    let found = ok(run(
        &home,
        &["search", "--all", "--raw", "only", "--", CANARY],
        "",
    ));
    let id = found.split_whitespace().next().unwrap();
    ok(run(&home, &["forget", "--record", id, "--yes"], ""));
    let projects = home.join("claude/projects/synthetic");
    std::fs::create_dir_all(&projects).unwrap();
    let line = |time: &str, prompt: &str| {
        serde_json::json!({
            "type":"user", "sessionId":"forgotten-cut-session", "cwd":"/synthetic",
            "timestamp":time, "message":{"role":"user", "content":prompt}
        })
        .to_string()
    };
    std::fs::write(
        projects.join("source.jsonl"),
        format!(
            "{}\n{}\n",
            line(
                "2026-08-31T23:59:59.000Z",
                "earlier-transcript-record-to-keep-402"
            ),
            line("2026-09-01T00:00:00.000Z", CANARY)
        ),
    )
    .unwrap();
    std::fs::remove_dir_all(home.join("backups")).unwrap();
    copy_backup(&old_backup, &home.join("backups"));
    ok(run(&home, &["restore"], ""));
    ok(run(
        &home,
        &["import", "transcripts", "--agent", "claude", "--yes"],
        "",
    ));
    ok(run(&home, &["worker", "--idle-ms", "0"], ""));
    let found = ok(run(
        &home,
        &["search", "--all", "--raw", "only", "--", CANARY],
        "",
    ));
    assert!(
        !found.contains(CANARY),
        "the transcript brought the forgotten prompt back: {found}"
    );
    let earlier = ok(run(
        &home,
        &[
            "search",
            "--all",
            "--raw",
            "only",
            "--",
            "earlier-transcript-record-to-keep-402",
        ],
        "",
    ));
    assert!(
        earlier.contains("earlier-transcript-record-to-keep-402"),
        "{earlier}"
    );
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

/// D1 rules 4 and 14: raw.db damaged after a forget is restored from segments older than it, and
/// the request logs forget it again.
#[test]
fn a_corrupt_raw_store_recovers_its_forget_from_the_request_logs() {
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
}

/// D1 F1: the backup directory could not be written at forget, raw.db is lost later; the home's
/// log alone forgets the record again in the restore.
#[test]
fn the_home_log_alone_restores_a_forget_the_backup_directory_missed() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(&home, CANARY);
    let away = root.path().join("backups-away");
    std::fs::rename(home.join("backups"), &away).unwrap();
    std::fs::write(home.join("backups"), b"not a directory").unwrap();
    let said = ok(run(&home, &["forget", "--record", &id, "--yes"], ""));
    assert!(said.contains("Not logged"), "{said}");
    std::fs::remove_file(home.join("backups")).unwrap();
    std::fs::rename(&away, home.join("backups")).unwrap();
    std::fs::remove_file(home.join("raw.db")).unwrap();
    ok(run(&home, &["restore"], ""));
    assert!(!String::from_utf8_lossy(&run(&home, &["get", &id], "").stdout).contains(CANARY));
    let found = ok(run(
        &home,
        &["search", "--all", "--raw", "only", "--", CANARY],
        "",
    ));
    // Search says "no hits" when it finds none (#380): the record is not among them.
    assert!(!found.contains(CANARY), "{found}");
}

/// D1 F2: raw.db goes back to a copy from before the forget, a hook takes the seqs again and the
/// source is imported again: the import reads the logs first, so the forgotten record stays out,
/// and the new record reads.
#[test]
fn raw_db_gone_back_keeps_the_new_record_and_not_the_forgotten_one() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(&home, CANARY);
    let old = root.path().join("raw-before.db");
    std::fs::copy(home.join("raw.db"), &old).unwrap();
    ok(run(&home, &["forget", "--record", &id, "--yes"], ""));
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.join(f));
    }
    std::fs::copy(&old, home.join("raw.db")).unwrap();
    let source = home.join("native-source.db");
    let imported = ok(run(
        &home,
        &["migrate", "--from", source.to_str().unwrap()],
        "",
    ));
    assert!(imported.contains("\"records\":0"), "{imported}");
    let fresh = hook_record(&home, "a-new-record-after-the-rollback-5521");
    assert!(ok(run(&home, &["get", &fresh], "")).contains("a-new-record-after-the-rollback-5521"));
    assert!(!String::from_utf8_lossy(&run(&home, &["get", &id], "").stdout).contains(CANARY));
}

/// F2 includes a new file taking raw.db's place: changing the device that appends new records
/// does not make the same home's forget requests foreign, including a request registered after
/// the first replacement and reconciled into a copy taken before it.
#[test]
fn replacing_raw_db_twice_keeps_both_forgets_and_blocks_reimport() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let first = record(&home, CANARY);
    let old = root.path().join("raw-before.db");
    std::fs::copy(home.join("raw.db"), &old).unwrap();
    let replace = || {
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        let staged = home.join("raw.db.copy");
        std::fs::copy(&old, &staged).unwrap();
        std::fs::rename(&staged, home.join("raw.db")).unwrap();
        ok(run(&home, &["worker", "--idle-ms", "0"], ""));
    };
    ok(run(&home, &["forget", "--record", &first, "--yes"], ""));
    replace();
    assert!(
        !String::from_utf8_lossy(&run(&home, &["get", &first], "").stdout).contains(CANARY),
        "the worker brought the first forgotten record back after a file replacement"
    );
    let second = record(&home, "forget-canary-topaz-after-replacement-381");
    ok(run(&home, &["forget", "--record", &second, "--yes"], ""));
    replace();
    let source = home.join("native-source.db");
    let imported = ok(run(
        &home,
        &["migrate", "--from", source.to_str().unwrap()],
        "",
    ));
    assert!(imported.contains("\"records\":0"), "{imported}");
    assert_eq!(
        ok(run(&home, &["forget", "--status"], "")).lines().count(),
        2
    );
    assert!(
        !ok(run(
            &home,
            &["search", "--all", "--raw", "only", "--", "forget-canary"],
            "",
        ))
        .contains("forget-canary")
    );
    let fresh = hook_record(&home, "unrelated-record-after-two-replacements-382");
    assert!(
        ok(run(&home, &["get", &fresh], ""))
            .contains("unrelated-record-after-two-replacements-382")
    );
}

/// F1 after F2: record backups carry the same home's identity after its appending device
/// changes, so losing raw.db and restoring an older backup still applies the surviving log.
#[test]
fn restoring_a_copied_stores_backup_keeps_its_forget_log() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    record(&home, "unrelated-record-before-the-copy-391");
    let staged = home.join("raw.db.copy");
    std::fs::copy(home.join("raw.db"), &staged).unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.join(f));
    }
    std::fs::rename(&staged, home.join("raw.db")).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[summary]\ncurate = false\n[backup]\ndir = 'new-backups'\n",
    )
    .unwrap();
    let id = record(&home, CANARY);
    let old_backup = root.path().join("before-forget");
    copy_backup(&home.join("new-backups"), &old_backup);
    ok(run(&home, &["forget", "--record", &id, "--yes"], ""));
    std::fs::remove_dir_all(home.join("new-backups")).unwrap();
    copy_backup(&old_backup, &home.join("new-backups"));
    for f in ["raw.db", "raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.join(f));
    }
    ok(run(&home, &["restore"], ""));
    assert!(
        !String::from_utf8_lossy(&run(&home, &["get", &id], "").stdout).contains(CANARY),
        "the surviving log was lost when a copied store was restored from its backup"
    );
    assert_eq!(
        ok(run(&home, &["forget", "--status"], "")).lines().count(),
        1
    );
    let source = home.join("native-source.db");
    let imported = ok(run(
        &home,
        &["migrate", "--from", source.to_str().unwrap()],
        "",
    ));
    assert!(imported.contains("\"records\":0"), "{imported}");
}

/// D1 limit (F3): raw.db and both request logs lost bring the text back from older segments,
/// as forget said before it was confirmed.
#[test]
fn raw_db_and_both_logs_lost_bring_the_text_back_as_forget_said() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let id = record(home, CANARY);
    let said = ok(run(home, &["forget", "--record", &id, "--yes"], ""));
    assert!(
        said.contains("If raw.db and both request logs are lost"),
        "{said}"
    );
    for f in ["raw.db", "forget.log", "backups/forget.log"] {
        std::fs::remove_file(home.join(f)).unwrap();
    }
    ok(run(home, &["restore"], ""));
    assert!(ok(run(home, &["get", &id], "")).contains(CANARY));
}

/// D1 rule 9: a torn last line in a log harms only itself; a later forget and a restore keep both.
#[test]
fn a_torn_log_line_and_a_later_forget_both_survive_a_restore() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::write(home.join("config.toml"), "[summary]\ncurate = false\n").unwrap();
    let first = record(home, CANARY);
    let second = record(home, "second-private-sapphire-9284");
    ok(run(home, &["forget", "--record", &first, "--yes"], ""));
    for log in [home.join("forget.log"), home.join("backups/forget.log")] {
        let text = std::fs::read_to_string(&log).unwrap();
        std::fs::write(&log, &text[..text.len() - 30]).unwrap();
    }
    ok(run(home, &["forget", "--record", &second, "--yes"], ""));
    std::fs::write(home.join("raw.db"), b"broken SQLite header").unwrap();
    ok(run(home, &["restore"], ""));
    // The torn line is skipped; the second request is applied; the first was written again
    // from raw.db by the second forget's reconcile, before raw.db was lost.
    assert!(
        !String::from_utf8_lossy(&run(home, &["get", &second], "").stdout)
            .contains("second-private-sapphire-9284")
    );
    assert!(!String::from_utf8_lossy(&run(home, &["get", &first], "").stdout).contains(CANARY));
}
