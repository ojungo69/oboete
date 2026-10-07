//! Task 8: a hook that cannot write because raw.db is damaged starts the worker, which restores
//! raw.db from the backups. Nothing else starts one: hooks start it only after a written row.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn oboete(home: &Path, args: &[&str], stdin: &str, spawn: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oboete"));
    cmd.arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !spawn {
        cmd.env("OBOETE_NO_SPAWN", "1");
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Until the restore's note is written (polling `search` would hold raw.lock shared over and over,
/// against the exclusive lock the restore waits for), then `search` finds the first prompt again.
fn restored(h: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.join("state").join("restored").exists() {
        assert!(Instant::now() < deadline, "raw.db was not restored");
        std::thread::sleep(Duration::from_millis(100));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = oboete(h, &["search", "zebra"], "", false);
        if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("zebra crossing") {
            return;
        }
        if Instant::now() >= deadline {
            // Why, on the Windows runner where this fails now and then: whether a worker run
            // here fails, and whether the search finds the record after it (the detached
            // worker's stderr goes nowhere).
            let detached = std::fs::read_to_string(h.join("state").join("worker-outcome"));
            let worker = oboete(h, &["worker", "--idle-ms", "0"], "", false);
            let again = oboete(h, &["search", "zebra"], "", false);
            panic!(
                "search does not find the restored record: {}, stdout {:?}, stderr {:?}; the \
                 detached worker's failure: {detached:?}; a worker run now: {}, stderr {:?}; \
                 search after it: stdout {:?}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
                worker.status,
                String::from_utf8_lossy(&worker.stderr),
                String::from_utf8_lossy(&again.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// One prompt recorded and backed up, then raw.db overwritten with bytes that are no database.
fn damaged_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let payload = serde_json::json!({"session_id": "s", "prompt": "zebra crossing notes"});
    let hook = ["hook", "claude", "UserPromptSubmit"];
    assert!(
        oboete(h, &hook, &payload.to_string(), false)
            .status
            .success()
    );
    // Its idle exit backs raw.db up.
    assert!(
        oboete(h, &["worker", "--idle-ms", "0"], "", false)
            .status
            .success()
    );
    home
}

fn damage(h: &Path) {
    for name in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(h.join(name));
    }
    std::fs::write(h.join("raw.db"), vec![b'x'; 4096]).unwrap();
}

fn second_prompt(h: &Path) {
    let payload = serde_json::json!({"session_id": "s", "prompt": "second"}).to_string();
    // The write fails, and the agent is not blocked.
    let hook = ["hook", "claude", "UserPromptSubmit"];
    assert!(oboete(h, &hook, &payload, true).status.success());
}

#[test]
fn a_worker_running_when_raw_db_is_damaged_restores_it_on_a_hook_s_request() {
    let home = damaged_home();
    let h = home.path();
    // A rule added after capture: the running worker's rescan writes a tombstone to raw.db.
    std::fs::write(
        h.join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"notes\", regex = 'notes' }]\n",
    )
    .unwrap();
    let mut worker = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(h)
        .args(["worker", "--idle-ms", "30000"])
        .env("OBOETE_NO_SPAWN", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Until it holds the lock.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(h.join("state").join("worker.lock"))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while lock.try_lock().is_ok() {
        lock.unlock().unwrap();
        assert!(Instant::now() < deadline, "the worker never took the lock");
        std::thread::sleep(Duration::from_millis(50));
    }
    let tombstones = || -> i64 {
        rusqlite::Connection::open(h.join("raw.db"))
            .and_then(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM records WHERE type = 'tombstone'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap_or(0)
    };
    while tombstones() == 0 {
        assert!(Instant::now() < deadline, "the rescan wrote no tombstone");
        std::thread::sleep(Duration::from_millis(50));
    }
    damage(h);
    second_prompt(h); // no worker starts: the running one holds the lock
    restored(h); // well before the running worker's idle exit
    worker.kill().unwrap();
    worker.wait().unwrap();
}

#[test]
fn a_hook_that_finds_raw_db_damaged_starts_the_restore() {
    let home = damaged_home();
    damage(home.path());
    second_prompt(home.path());
    restored(home.path());
}

#[test]
fn w5b_restore_with_a_temporary_directory_inside_home_uses_the_complete_native_cli() {
    let home = tempfile::tempdir().unwrap();
    let owner = tempfile::tempdir().unwrap();
    let h = home.path();
    let temporary = h.join("tmp");
    std::fs::create_dir(&temporary).unwrap();
    std::fs::write(
        h.join("config.toml"),
        "providers = []\n[summary]\ncurate = false\n[embedding]\nprovider = 'none'\n[worker]\nresident = false\n",
    )
    .unwrap();
    let run = |args: &[&str], input: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oboete"));
        command
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| {
                !["KEY", "TOKEN", "SECRET", "PASSWORD"]
                    .iter()
                    .any(|part| key.to_string_lossy().to_ascii_uppercase().contains(part))
            }))
            .env("HOME", owner.path())
            .env("USERPROFILE", owner.path())
            .env("CODEX_HOME", owner.path().join("codex"))
            .env("CLAUDE_CONFIG_DIR", owner.path().join("claude"))
            .env("XDG_CONFIG_HOME", owner.path().join("config"))
            .env("OBOETE_NO_SPAWN", "1")
            .env("TMPDIR", &temporary)
            .env("TEMP", &temporary)
            .env("TMP", &temporary)
            .current_dir(owner.path())
            .arg("--home")
            .arg(h)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    for i in 0..3 {
        let payload =
            serde_json::json!({"session_id":format!("s{i}"),"prompt":"zebra crossing notes"});
        run(
            &["hook", "claude", "UserPromptSubmit"],
            &payload.to_string(),
        );
    }
    run(&["worker", "--idle-ms", "0"], "");
    assert!(h.join("backups").is_dir());
    let restored = run(&["restore"], "");
    assert!(String::from_utf8_lossy(&restored.stdout).contains("3 record(s)"));
    assert!(h.join("state/restored").is_file());
    let raw = rusqlite::Connection::open(h.join("raw.db")).unwrap();
    let records: i64 = raw
        .query_row("SELECT COUNT(*) FROM records WHERE type='event'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(records, 3);
    drop(raw);
    assert!(h.join("knowledge.db").is_file());
    let knowledge = rusqlite::Connection::open_with_flags(
        h.join("knowledge.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let indexed: i64 = knowledge
        .query_row("SELECT COUNT(*) FROM raw_fts_docsize", [], |r| r.get(0))
        .unwrap();
    assert_eq!(indexed, 3);
    drop(knowledge);
    let searched = run(&["search", "zebra"], "");
    assert!(String::from_utf8_lossy(&searched.stdout).contains("zebra crossing"));
    assert!(!h.join("providers.db").exists());
}
