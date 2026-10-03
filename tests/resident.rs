//! docs/resident.md: `oboete worker` in a home whose config.toml says `[worker] resident = true`
//! stays when it is idle (R2), says so in its outcome while it waits (R10), and steps aside for
//! `oboete restore`, backing up first (R12). Linux only, as the resident worker is for now.
#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn oboete(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        // The test starts the worker itself.
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

/// A worker that a failed test would leave running is killed.
struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < Duration::from_secs(20), "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn restore_runs_beside_a_resident_worker_which_backs_up_and_exits_for_it() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(h.join("config.toml"), "[worker]\nresident = true\n").unwrap();
    let payload = serde_json::json!({"session_id": "s", "prompt": "zebra crossing notes"});
    let hook = ["hook", "claude", "UserPromptSubmit"];
    assert!(oboete(h, &hook, &payload.to_string()).status.success());
    // As a hook starts it: with no idle time of its own.
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(h)
            .arg("worker")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // It waits with a clean outcome, which only a resident worker writes before it exits.
    let outcome = h.join("state").join("worker-outcome");
    until("a clean outcome from a worker that still runs", || {
        std::fs::read_to_string(&outcome).is_ok_and(|o| o.ends_with('\n'))
    });
    assert!(worker.0.try_wait().unwrap().is_none(), "it exited");
    // Its idle time (60 s) has not passed: the segments a restore reads are the ones it writes as
    // it steps aside.
    assert!(!h.join("backups").exists());
    let restore = oboete(h, &["restore"], "");
    assert!(
        restore.status.success(),
        "{}",
        String::from_utf8_lossy(&restore.stderr)
    );
    until("the worker exits", || {
        worker.0.try_wait().unwrap().is_some()
    });
    assert!(worker.0.wait().unwrap().success());
    let found = oboete(h, &["search", "zebra"], "");
    assert!(
        String::from_utf8_lossy(&found.stdout).contains("zebra crossing"),
        "{found:?}"
    );
}

/// R6: the token file is its owner's to read whatever the umask. Under one that takes the owner's
/// own read away (Codex on #376), a file asked for at 0600 came out write-only, and the viewer
/// refused every request.
#[test]
fn the_token_file_is_0600_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let _viewer = Worker(
        Command::new("sh")
            .args([
                "-c",
                "umask 0400 && exec \"$0\" --home \"$1\" view --resident",
            ])
            .arg(env!("CARGO_BIN_EXE_oboete"))
            .arg(h)
            .env("OBOETE_NO_SPAWN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let file = h.join("state").join("view-token");
    until("the token file", || file.exists());
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
