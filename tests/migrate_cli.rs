//! `oboete migrate` (milestone 4 Task 9, spec 7.4): a pass from the command line, a rerun that
//! imports nothing, and `--finish` that deletes nothing without a yes.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn oboete(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.to_string_lossy()])
        .args(args)
        .env("OBOETE_NO_SPAWN", "1")
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
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

#[cfg(unix)]
#[test]
fn migration_commands_refuse_source_aliases_without_writing_v1() {
    for command in ["migrate", "finish", "transcripts"] {
        for alias in ["identical", "symlink", "hardlink"] {
            if command != "migrate" && alias == "identical" {
                continue;
            }
            let home = tempfile::tempdir().unwrap();
            let from = home.path().join(if alias == "identical" {
                "raw.db"
            } else {
                "oboete.db"
            });
            let v1 = rusqlite::Connection::open(&from).unwrap();
            v1.execute_batch(
                "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES('device_id', 'v1-device');",
            )
            .unwrap();
            drop(v1);
            match alias {
                "symlink" => std::os::unix::fs::symlink(&from, home.path().join("raw.db")).unwrap(),
                "hardlink" => std::fs::hard_link(&from, home.path().join("raw.db")).unwrap(),
                _ => {}
            }
            let before = std::fs::read(&from).unwrap();
            let args = match command {
                "migrate" => vec!["migrate", "--from", from.to_str().unwrap()],
                "finish" => vec!["migrate", "--finish"],
                _ => vec!["import", "transcripts", "--yes"],
            };
            let out = oboete(home.path(), &args, "yes\n");
            assert!(
                std::fs::read(&from).unwrap() == before,
                "{command} wrote its {alias} v1 source"
            );
            assert!(!out.status.success(), "{command} accepted a {alias} alias");
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(said.contains("aliases destination raw.db"), "{said}");
        }
    }
}

#[test]
fn migration_commands_refuse_a_stopped_restore_alias() {
    for args in [
        vec!["migrate"],
        vec!["migrate", "--finish"],
        vec!["import", "transcripts", "--yes"],
    ] {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("oboete.db");
        let v1 = rusqlite::Connection::open(&from).unwrap();
        v1.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES('device_id', 'v1-device');",
        )
        .unwrap();
        drop(v1);
        std::fs::hard_link(&from, home.path().join("raw.db.restored")).unwrap();
        let before = std::fs::read(&from).unwrap();
        let out = oboete(home.path(), &args, "yes\n");
        assert!(!out.status.success());
        assert!(std::fs::read(&from).unwrap() == before);
        assert!(!home.path().join("raw.db").exists());
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains("aliases destination raw.db"), "{said}");
    }
}

#[cfg(unix)]
#[test]
fn invalid_v1_settings_stay_private_in_a_fresh_home() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let v1 = root.path().join("v1");
    std::fs::create_dir(&v1).unwrap();
    std::fs::set_permissions(&v1, std::fs::Permissions::from_mode(0o700)).unwrap();
    let secret = "private-config-regression-value";
    let config = format!("[redaction]\nextra_rules = [{{ id = 'private', regex = '{secret}' }}\n");
    std::fs::write(v1.join("config.toml"), &config).unwrap();
    std::fs::set_permissions(
        v1.join("config.toml"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let home = root.path().join("new-home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_oboete"));
    command
        .arg("--home")
        .arg(&home)
        .args(["migrate", "--from"])
        .arg(v1.join("oboete.db"));
    let out = command.output().unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("parse ") && !said.contains(secret), "{said}");
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        config
    );
    assert_eq!(std::fs::metadata(&home).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        std::fs::metadata(home.join("config.toml")).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(v1.join("config.toml")).unwrap(),
        config
    );
}
