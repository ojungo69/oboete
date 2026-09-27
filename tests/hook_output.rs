//! An agent reads a hook's output to its end. A hook that starts the worker must not leave that
//! output open for the worker's lifetime: Windows passes a process's inheritable handles on to
//! its children, the hook's own stdout among them (60 s measured on Windows 11).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_hooks_output_ends_when_the_hook_does_not_when_its_worker_does() {
    let home = tempfile::tempdir().unwrap();
    let mut hook = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home.path())
        .args(["hook", "claude", "UserPromptSubmit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"s","prompt":"a prompt that starts the worker"}"#)
        .unwrap();
    let start = Instant::now();
    let out = hook.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(
        home.path().join("state").join("worker.lock").exists(),
        "no worker was started"
    );
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}
