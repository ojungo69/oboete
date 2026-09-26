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

#[test]
fn a_hook_that_finds_raw_db_damaged_starts_the_restore() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let prompt = |text: &str| serde_json::json!({"session_id": "s", "prompt": text}).to_string();
    let hook = ["hook", "claude", "UserPromptSubmit"];
    assert!(
        oboete(h, &hook, &prompt("zebra crossing notes"), false)
            .status
            .success()
    );
    // Its idle exit backs raw.db up.
    assert!(
        oboete(h, &["worker", "--idle-ms", "0"], "", false)
            .status
            .success()
    );
    for name in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(h.join(name));
    }
    std::fs::write(h.join("raw.db"), vec![b'x'; 4096]).unwrap();
    // The write fails, the agent is not blocked, and a worker starts.
    assert!(oboete(h, &hook, &prompt("second"), true).status.success());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let out = oboete(h, &["search", "zebra"], "", false);
        if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("zebra crossing") {
            break;
        }
        assert!(Instant::now() < deadline, "raw.db was not restored");
        std::thread::sleep(Duration::from_millis(200));
    }
}
