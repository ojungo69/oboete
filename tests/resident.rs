//! docs/resident.md: `oboete worker` in a home whose config.toml says `[worker] resident = true`
//! stays when it is idle (R2), says so in its outcome while it waits (R10), and steps aside for
//! `oboete restore`, backing up first (R12); `oboete view` there brings up the resident viewer
//! and the worker (R7), or serves on its own when the viewer cannot start. Linux only, as the
//! resident worker is for now.
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
    // As a hook starts it: with no idle time of its own, and no viewer, which this test is not
    // about.
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(h)
            .arg("worker")
            .env("OBOETE_NO_SPAWN", "1")
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

/// The processes `oboete view` started detached in `home` (the worker, the viewer), stopped when
/// the test ends however it ends: found by their `--home` argument.
struct Detached<'a>(&'a Path);

impl Drop for Detached<'_> {
    fn drop(&mut self) {
        let want = format!("--home\0{}\0", self.0.display());
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains(&want) {
                let _ = Command::new("kill").arg(pid.to_string()).status();
            }
        }
    }
}

/// Whether a process holds the lock file `name` of `home`'s state.
fn held(home: &Path, name: &str) -> bool {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("state").join(name))
        .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// Resident test 2 (R7): `oboete view` in a resident home with nothing running starts the viewer
/// and the worker, both locks held, and prints the address with the token of the viewer's file,
/// which answers there.
#[test]
fn view_in_a_resident_home_brings_up_the_viewer_and_the_worker() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let _detached = Detached(h);
    // As the owner runs it: it may start processes. One that does not return (it serves on its own
    // address) is killed, and the test fails.
    let mut view = Worker(
        Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(h)
            .arg("view")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    until("oboete view returns", || {
        view.0.try_wait().unwrap().is_some()
    });
    assert!(view.0.wait().unwrap().success());
    let mut out = String::new();
    std::io::Read::read_to_string(&mut view.0.stdout.take().unwrap(), &mut out).unwrap();
    let token = std::fs::read_to_string(h.join("state").join("view-token")).unwrap();
    assert!(
        out.starts_with(&format!("http://127.0.0.1:{port}/#t={token}\n")),
        "{out}"
    );
    assert!(held(h, "view.lock") && held(h, "worker.lock"));
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        c,
        "GET /api/repos HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: {token}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    std::io::Read::read_to_string(&mut c, &mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
}

/// Resident test 5 (R7): with the port held by another program, `oboete view` says why and
/// serves on an address of its own; it sends that program nothing.
#[test]
fn view_serves_on_its_own_address_when_the_port_is_in_use() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let foreign = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    foreign.set_nonblocking(true).unwrap();
    let port = foreign.local_addr().unwrap().port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let _detached = Detached(h);
    let mut view = Worker(
        Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(h)
            .arg("view")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(view.0.stdout.take().unwrap()),
        &mut first,
    )
    .unwrap();
    assert!(first.starts_with("http://127.0.0.1:"), "{first}");
    assert!(
        !first.starts_with(&format!("http://127.0.0.1:{port}/")),
        "{first}"
    );
    let _ = view.0.kill();
    let mut why = String::new();
    std::io::Read::read_to_string(&mut view.0.stderr.take().unwrap(), &mut why).unwrap();
    assert!(why.contains("port in use"), "{why}");
    assert!(matches!(
        foreign.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
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
    // The lock and the outcome, made before the token, are the owner's to open again too
    // (Codex on #376).
    for name in ["view-token", "view.lock", "view-outcome"] {
        let mode = std::fs::metadata(h.join("state").join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
}
