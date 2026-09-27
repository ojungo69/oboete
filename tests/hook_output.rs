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
    let ended = start.elapsed();
    assert!(out.status.success());
    // The worker holds its lock while it waits for records (60 s idle): held now, it was running
    // when the output ended.
    let held = || {
        std::fs::File::open(home.path().join("state").join("worker.lock"))
            .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
    };
    let until = Instant::now() + Duration::from_secs(5);
    while !held() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held(), "no worker was running");
    assert!(ended < Duration::from_secs(20), "{ended:?}");
}
