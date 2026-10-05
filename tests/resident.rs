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
    until_within(what, Duration::from_secs(20), &mut done);
}

fn until_within(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < within, "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn copied_command(executable: &Path, home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("--home")
        .arg(home)
        .args(args)
        .current_dir(home)
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            let key = key.to_string_lossy().to_ascii_uppercase();
            !["KEY", "TOKEN", "SECRET", "PASSWORD"]
                .iter()
                .any(|word| key.contains(word))
        }))
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn copied_spawn(command: &mut Command) -> Child {
    let start = Instant::now();
    loop {
        match command.spawn() {
            Ok(child) => return child,
            // Another test's fork can retain a copy's write FD until its exec closes it.
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY)
                    && start.elapsed() < Duration::from_secs(20) =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("copied binary failed to start: {error}"),
        }
    }
}

fn repos_response(home: &Path, port: u16) -> String {
    repos_response_on(
        home,
        port,
        std::net::TcpStream::connect(("127.0.0.1", port)).unwrap(),
    )
}

/// Wait within the existing startup budget for this replacement's actual listener, then send
/// one strict request. Image visibility and an inherited lock alone do not establish readiness.
fn ready_repos_response(
    home: &Path,
    port: u16,
    previous: &std::fs::File,
    process: &mut Child,
) -> String {
    until("the new viewer republishes its bound listener", || {
        assert!(
            process.try_wait().unwrap().is_none(),
            "the replacement viewer exited before binding"
        );
        fresh_viewer_listener(home, port, previous)
    });
    repos_response(home, port)
}

/// An image can change before the new viewer has republished its bound listener.
fn fresh_viewer_listener(home: &Path, port: u16, previous: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(mut outcome) = std::fs::File::open(home.join("state/view-outcome")) else {
        return false;
    };
    let (Ok(old), Ok(current)) = (previous.metadata(), outcome.metadata()) else {
        return false;
    };
    if (old.dev(), old.ino()) == (current.dev(), current.ino()) || !held(home, "view.lock") {
        return false;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut outcome, &mut text).is_ok()
        && text.trim() == format!("listening {port}")
}

#[test]
fn post_exec_viewer_readiness_requires_a_fresh_listening_outcome() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let lock = std::fs::File::create(state.join("view.lock")).unwrap();
    lock.lock().unwrap();
    let outcome = state.join("view-outcome");
    std::fs::write(&outcome, "listening 12345").unwrap();
    // Keep the old inode allocated across the staged renames, as the copied-binary fixture does.
    let previous = std::fs::File::open(&outcome).unwrap();
    assert!(
        !fresh_viewer_listener(home.path(), 12345, &previous),
        "the previous image's listening outcome counted as a new listener"
    );
    let publish = |text: &str| {
        let next = state.join("view-outcome.next");
        std::fs::write(&next, text).unwrap();
        std::fs::rename(next, &outcome).unwrap();
    };
    publish("starting");
    assert!(!fresh_viewer_listener(home.path(), 12345, &previous));
    publish("port in use");
    assert!(!fresh_viewer_listener(home.path(), 12345, &previous));
    publish("listening 12346");
    assert!(!fresh_viewer_listener(home.path(), 12345, &previous));
    publish("listening 12345");
    assert!(fresh_viewer_listener(home.path(), 12345, &previous));
    lock.unlock().unwrap();
    assert!(!fresh_viewer_listener(home.path(), 12345, &previous));
}

fn repos_response_on(home: &Path, port: u16, mut stream: std::net::TcpStream) -> String {
    let token = std::fs::read_to_string(home.join("state/view-token")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /api/repos HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: {token}\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
    response
}

fn copied_viewer(executable: &Path, home: &Path, mut port: u16) -> (Worker, u16) {
    let start = Instant::now();
    loop {
        let mut viewer = Worker(copied_spawn(&mut copied_command(
            executable,
            home,
            &["view", "--resident"],
        )));
        until_within(
            "the copied viewer's initial bind result",
            Duration::from_secs(20).saturating_sub(start.elapsed()),
            || {
                viewer.0.try_wait().unwrap().is_some()
                    || std::fs::read_to_string(home.join("state/view-outcome"))
                        .is_ok_and(|out| out.trim() == format!("listening {port}"))
            },
        );
        let out = std::fs::read_to_string(home.join("state/view-outcome")).unwrap();
        if out.trim() != "port in use" {
            assert_eq!(out.trim(), format!("listening {port}"));
            return (viewer, port);
        }
        // The native listener reported AddrInUse before any R9 measurement. Reap that failed
        // start and change only this private fixture's port; never retry a later HTTP failure.
        assert!(viewer.0.wait().unwrap().success());
        assert!(start.elapsed() < Duration::from_secs(20));
        let next = free_port();
        let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        let old = format!("\nport = {port}\n");
        assert!(config.contains(&old));
        std::fs::write(
            home.join("config.toml"),
            config.replace(&old, &format!("\nport = {next}\n")),
        )
        .unwrap();
        port = next;
    }
}

#[test]
fn a_copied_viewer_retries_only_a_port_lost_before_initial_bind() {
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let first = occupied.local_addr().unwrap().port();
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {first}\n"),
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let (_viewer, port) = copied_viewer(&executable, home.path(), first);
    assert_ne!(port, first);
    assert!(held(home.path(), "view.lock"));
    assert!(repos_response(home.path(), port).starts_with("HTTP/1.1 200 OK\r\n"));
}

#[test]
fn a_renamed_binary_replaces_the_viewer_at_its_minute_check_with_the_same_port() {
    use std::os::unix::fs::MetadataExt;
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let (mut viewer, port) = copied_viewer(&executable, h, port);
    let pid = viewer.0.id();
    assert!(held(h, "view.lock"));
    assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    let arguments = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    let previous_outcome = std::fs::File::open(h.join("state/view-outcome")).unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    std::fs::rename(&next, &executable).unwrap();
    until_within(
        "the viewer executes the renamed binary",
        Duration::from_secs(75),
        || {
            std::fs::metadata(format!("/proc/{pid}/exe")).is_ok_and(|current| {
                (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
            })
        },
    );
    until("the replacement viewer holds its home", || {
        held(h, "view.lock")
    });
    assert!(viewer.0.try_wait().unwrap().is_none());
    assert!(
        ready_repos_response(h, port, &previous_outcome, &mut viewer.0)
            .starts_with("HTTP/1.1 200 OK\r\n")
    );
    assert_eq!(
        std::fs::read(format!("/proc/{pid}/cmdline")).unwrap(),
        arguments
    );
}

#[test]
fn a_replaced_worker_reaps_its_existing_viewer_child_without_starting_duplicates() {
    use std::os::unix::fs::MetadataExt;
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let _detached = Detached(h);
    let mut worker = Worker(copied_spawn(
        copied_command(&executable, h, &["worker"]).env_remove("OBOETE_NO_SPAWN"),
    ));
    let pid = worker.0.id();
    until("the worker's viewer", || {
        held(h, "worker.lock")
            && held(h, "view.lock")
            && std::fs::read_to_string(h.join("state/view-outcome"))
                .is_ok_and(|out| out.trim() == format!("listening {port}"))
    });
    let viewers: Vec<_> = started(h)
        .into_iter()
        .filter(|child| *child != pid)
        .collect();
    assert_eq!(viewers.len(), 1, "the worker started duplicate viewers");
    let child = viewers[0];
    let previous_generation = std::fs::read(h.join("state/worker-gen")).unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    std::fs::rename(next, &executable).unwrap();
    until("the worker's replacement image", || {
        std::fs::metadata(format!("/proc/{pid}/exe")).is_ok_and(|current| {
            (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
        })
    });
    let mut ready_processes = Vec::new();
    until(
        "replacement ownership and both existing process arguments",
        || {
            assert!(worker.0.try_wait().unwrap().is_none());
            let generation_changed = std::fs::read(h.join("state/worker-gen"))
                .is_ok_and(|generation| generation != previous_generation);
            let running = started(h);
            if generation_changed
                && held(h, "worker.lock")
                && running.contains(&pid)
                && running.contains(&child)
            {
                ready_processes = running;
                true
            } else {
                false
            }
        },
    );
    assert!(worker.0.try_wait().unwrap().is_none());
    // `/proc/PID/exe` can change before argv is readable. Use the same ready snapshot for the
    // strict count; a third process still fails, and loss of the original child cannot pass.
    assert_eq!(ready_processes.len(), 2, "exec duplicated the viewer");
    assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    // SAFETY: this is the specific viewer PID created in this private home by our worker.
    assert_eq!(
        unsafe { libc::kill(i32::try_from(child).unwrap(), libc::SIGTERM) },
        0
    );
    until("the replacement worker reaps its old viewer", || {
        !Path::new(&format!("/proc/{child}")).exists()
    });
    assert_eq!(zombies(pid), 0);
    assert_eq!(
        started(h).len(),
        1,
        "the minute restart deadline was bypassed"
    );
}

#[test]
fn a_worker_keeps_serving_failed_images_and_temporary_absence_then_leaves_a_missing_path() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(
        h.join("config.toml"),
        "[worker]\nresident = true\n[summary]\ncurate = false\n",
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let mut worker = Worker(copied_spawn(&mut copied_command(
        &executable,
        h,
        &["worker"],
    )));
    let pid = worker.0.id();
    until("the copied resident worker", || {
        held(h, "worker.lock") && h.join("state/worker-outcome").exists()
    });
    let generation = std::fs::read_to_string(h.join("state/worker-gen")).unwrap();
    let image = format!("/proc/{pid}/exe");
    let old = std::fs::metadata(&image).unwrap();
    let next = scratch.path().join("invalid.next");
    std::fs::write(&next, "synthetic invalid executable image").unwrap();
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(&next, &executable).unwrap();
    // Real signal delivery after failed replacement must retain Rust's SIGPIPE immunity.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        unsafe { libc::kill(i32::try_from(pid).unwrap(), libc::SIGPIPE) },
        0
    );
    assert!(worker.0.try_wait().unwrap().is_none());
    assert!(held(h, "worker.lock"));
    assert_eq!(
        (
            std::fs::metadata(&image).unwrap().dev(),
            std::fs::metadata(&image).unwrap().ino()
        ),
        (old.dev(), old.ino())
    );
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::rename(&next, &executable).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        unsafe { libc::kill(i32::try_from(pid).unwrap(), libc::SIGPIPE) },
        0
    );
    assert!(worker.0.try_wait().unwrap().is_none() && held(h, "worker.lock"));
    let missing = scratch.path().join("temporarily-gone");
    std::fs::rename(&executable, &missing).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(worker.0.try_wait().unwrap().is_none() && held(h, "worker.lock"));
    std::fs::rename(&missing, &executable).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let replacement = std::fs::metadata(&executable).unwrap();
    until("the executable's recovery", || {
        std::fs::metadata(&image).is_ok_and(|current| {
            (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
        })
    });
    // Kernel image visibility can precede main's startup-path capture. The new taking is
    // written only after executable::init, so do not move that path again before it completes.
    until("recovered ownership", || {
        held(h, "worker.lock")
            && std::fs::read_to_string(h.join("state/worker-gen"))
                .is_ok_and(|current| current != generation)
    });
    std::fs::rename(&executable, &missing).unwrap();
    let absent = Instant::now();
    until_within(
        "a still-missing executable ends the worker",
        Duration::from_secs(75),
        || worker.0.try_wait().unwrap().is_some(),
    );
    assert!(absent.elapsed() >= Duration::from_secs(59));
    assert!(worker.0.wait().unwrap().success());
    assert!(!held(h, "worker.lock"));
}

#[test]
fn viewer_replacement_waits_for_live_requests_and_answers_them_before_exec() {
    use std::os::unix::fs::MetadataExt;
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let (mut viewer, port) = copied_viewer(&executable, h, port);
    let pid = viewer.0.id();
    assert!(held(h, "view.lock"));
    assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    let previous_outcome = std::fs::File::open(h.join("state/view-outcome")).unwrap();
    let token = std::fs::read_to_string(h.join("state/view-token")).unwrap();
    let image = format!("/proc/{pid}/exe");
    let old = std::fs::metadata(&image).unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    let mut pending = Vec::<(std::net::TcpStream, Instant)>::new();
    let mut answered = 0;
    let start = Instant::now();
    // Overlap real partial HTTP heads, each completed within REQUEST_TIME, so at least one
    // request remains live across the first actual minute tick without a runtime test switch.
    while start.elapsed() < Duration::from_secs(65) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET /api/repos HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: {token}\r\n"
        )
        .unwrap();
        pending.push((stream, Instant::now()));
        if start.elapsed() >= Duration::from_secs(1) && next.exists() {
            std::fs::rename(&next, &executable).unwrap();
        }
        if pending[0].1.elapsed() >= Duration::from_secs(1) {
            let (mut stream, _) = pending.remove(0);
            stream.write_all(b"\r\n").unwrap();
            let mut response = String::new();
            std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
            answered += 1;
        }
        let current = std::fs::metadata(&image).unwrap();
        assert_eq!(
            (current.dev(), current.ino()),
            (old.dev(), old.ino()),
            "the viewer exec cut an in-flight request off"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    for (mut stream, _) in pending {
        stream.write_all(b"\r\n").unwrap();
        let mut response = String::new();
        std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        answered += 1;
    }
    assert!(answered > 0);
    until_within(
        "the drained viewer executes at its next minute",
        Duration::from_secs(75),
        || {
            std::fs::metadata(&image).is_ok_and(|current| {
                (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
            })
        },
    );
    until("the replacement listens", || held(h, "view.lock"));
    assert!(viewer.0.try_wait().unwrap().is_none());
    assert!(
        ready_repos_response(h, port, &previous_outcome, &mut viewer.0)
            .starts_with("HTTP/1.1 200 OK\r\n")
    );
}

fn viewer_failed_image_or_missing(mode: &str) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let (mut viewer, port) = copied_viewer(&executable, h, port);
    let pid = viewer.0.id();
    assert!(held(h, "view.lock"));
    assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    let previous_outcome = std::fs::File::open(h.join("state/view-outcome")).unwrap();
    let image = format!("/proc/{pid}/exe");
    let old = std::fs::metadata(&image).unwrap();
    let next = scratch.path().join("candidate.next");
    if mode == "missing" {
        std::fs::rename(&executable, scratch.path().join("missing.copy")).unwrap();
    } else {
        if mode == "invalid" {
            std::fs::write(&next, "synthetic invalid image").unwrap();
        } else {
            std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
        }
        std::fs::set_permissions(
            &next,
            std::fs::Permissions::from_mode(if mode == "invalid" { 0o700 } else { 0o600 }),
        )
        .unwrap();
        std::fs::rename(&next, &executable).unwrap();
    }
    let first = Instant::now();
    until_within(
        "the first actual minute keeps the original viewer serving",
        Duration::from_secs(75),
        || {
            assert!(
                viewer.0.try_wait().unwrap().is_none(),
                "{mode} ended the original viewer"
            );
            first.elapsed() >= Duration::from_secs(65)
        },
    );
    let current = std::fs::metadata(&image).unwrap();
    assert_eq!((current.dev(), current.ino()), (old.dev(), old.ino()));
    assert!(held(h, "view.lock"));
    // SAFETY: only this owned synthetic viewer receives the signal.
    assert_eq!(
        unsafe { libc::kill(i32::try_from(pid).unwrap(), libc::SIGPIPE) },
        0
    );
    assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o700)).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    std::fs::rename(&next, &executable).unwrap();
    until_within(
        "the viewer recovers at its next minute",
        Duration::from_secs(75),
        || {
            std::fs::metadata(&image).is_ok_and(|current| {
                (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
            })
        },
    );
    until("recovered viewer ownership", || held(h, "view.lock"));
    assert!(viewer.0.try_wait().unwrap().is_none());
    assert!(
        ready_repos_response(h, port, &previous_outcome, &mut viewer.0)
            .starts_with("HTTP/1.1 200 OK\r\n")
    );
    if mode == "missing" {
        std::fs::rename(&executable, scratch.path().join("permanently-missing")).unwrap();
        let absent = Instant::now();
        until_within(
            "the second absent minute ends the viewer",
            Duration::from_secs(135),
            || viewer.0.try_wait().unwrap().is_some(),
        );
        assert!(absent.elapsed() >= Duration::from_secs(115));
        assert!(viewer.0.wait().unwrap().success());
        assert!(!held(h, "view.lock"));
    }
}

#[test]
fn r9_viewer_invalid_image_keeps_http_and_sigpipe_immunity() {
    viewer_failed_image_or_missing("invalid");
}

#[test]
fn r9_viewer_unexecutable_image_keeps_http_and_sigpipe_immunity() {
    viewer_failed_image_or_missing("unexecutable");
}

#[test]
fn r9_viewer_recovers_short_absence_and_exits_continued_absence() {
    viewer_failed_image_or_missing("missing");
}

/// R9/test 9: updating the installed path by rename replaces the resident worker
/// in place, with its command line and ownership of the same home intact.
#[test]
fn a_renamed_binary_replaces_the_worker_with_its_arguments_and_lock() {
    use std::os::unix::fs::MetadataExt;

    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(
        h.join("config.toml"),
        "[worker]\nresident = true\n[summary]\ncurate = false\n",
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let mut worker = Worker(copied_spawn(&mut copied_command(
        &executable,
        h,
        &["worker"],
    )));
    let pid = worker.0.id();
    let image = format!("/proc/{pid}/exe");
    until("the copied worker holds its home lock", || {
        held(h, "worker.lock")
            && std::fs::read_to_string(h.join("state/worker-outcome"))
                .is_ok_and(|outcome| outcome.ends_with('\n'))
    });
    let old = std::fs::metadata(&image).unwrap();
    let arguments = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    let generation = std::fs::read_to_string(h.join("state/worker-gen")).unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    assert_ne!(
        (old.dev(), old.ino()),
        (replacement.dev(), replacement.ino())
    );
    std::fs::rename(&next, &executable).unwrap();
    until("the worker executes the renamed binary", || {
        std::fs::metadata(&image).is_ok_and(|current| {
            (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
        })
    });
    until("the replacement worker holds its home lock", || {
        held(h, "worker.lock")
            && std::fs::read_to_string(h.join("state/worker-gen"))
                .is_ok_and(|current| current != generation)
            && std::fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|arguments| !arguments.is_empty())
    });
    assert!(worker.0.try_wait().unwrap().is_none());
    assert_eq!(
        std::fs::read(format!("/proc/{pid}/cmdline")).unwrap(),
        arguments
    );
    assert_eq!(worker.0.id(), pid);
}

#[test]
fn a_missing_home_ends_the_old_worker_before_it_executes_a_replacement() {
    let scratch = tempfile::tempdir().unwrap();
    let h = scratch.path().join("home");
    std::fs::create_dir(&h).unwrap();
    std::fs::write(
        h.join("config.toml"),
        "[worker]\nresident = true\n[summary]\ncurate = false\n",
    )
    .unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let mut worker = Worker(copied_spawn(&mut copied_command(
        &executable,
        &h,
        &["worker"],
    )));
    let pid = i32::try_from(worker.0.id()).unwrap();
    until("the copied worker sleeps in its settled wait", || {
        std::fs::read_to_string(h.join("state/worker-outcome"))
            .is_ok_and(|outcome| outcome.trim().parse::<u64>().is_ok())
            && std::fs::read_to_string(format!("/proc/{pid}/wchan"))
                .is_ok_and(|where_| where_.trim() == "hrtimer_nanosleep")
    });
    // Stop only our owned worker, then confirm the stop before replacing either pathname.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    until("the owned worker is stopped", || {
        let mut status = 0;
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        assert!(waited >= 0);
        waited == pid && libc::WIFSTOPPED(status)
    });
    std::fs::rename(&h, scratch.path().join("old-home")).unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    std::fs::rename(next, executable).unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    until("the old worker notices the removed home", || {
        h.exists() || worker.0.try_wait().unwrap().is_some()
    });
    assert!(
        !h.exists(),
        "R9 exec recreated the removed home before the R3 gone check"
    );
    assert!(!worker.0.wait().unwrap().success());
}

fn private_files(home: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(home).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(private_files(&path));
        } else {
            files.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    files.sort();
    files
}

/// The old process has already passed gone/leaving and successfully execed the shim. Its
/// same-PID shell waits outside the home, before initializing the replacement product image.
fn exec_keeps_the_original_home(role: &str, remake: bool) {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let h = scratch.path().join("home");
    std::fs::create_dir(&h).unwrap();
    let port = free_port();
    let config =
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n");
    std::fs::write(h.join("config.toml"), &config).unwrap();
    let executable = scratch.path().join("oboete");
    let next_image = scratch.path().join("replacement-product");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next_image).unwrap();
    let (mut process, port) = if role == "worker" {
        (
            Worker(copied_spawn(&mut copied_command(
                &executable,
                &h,
                &["worker"],
            ))),
            port,
        )
    } else {
        copied_viewer(&executable, &h, port)
    };
    let pid = process.0.id();
    let lock = if role == "worker" {
        "worker.lock"
    } else {
        "view.lock"
    };
    until("the original resident holds its home", || held(&h, lock));
    if role == "viewer" {
        assert!(repos_response(&h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    }
    let ready = scratch.path().join("replacement-ready");
    let release = scratch.path().join("replacement-release");
    assert!(
        Command::new("mkfifo")
            .arg(&release)
            .env_clear()
            .status()
            .unwrap()
            .success()
    );
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let shim = scratch.path().join("replacement-shim");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > {}\nIFS= read -r release < {}\nexec {} \"$@\"\n",
            quote(&ready),
            quote(&release),
            quote(&next_image),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(shim, executable).unwrap();
    until_within(
        "the same-PID exec reaches its external barrier",
        Duration::from_secs(75),
        || std::fs::read_to_string(&ready).is_ok_and(|ready| ready.trim() == pid.to_string()),
    );
    assert_eq!(
        std::fs::read_to_string(ready).unwrap().trim(),
        pid.to_string()
    );
    std::fs::rename(&h, scratch.path().join("old-home")).unwrap();
    let before = if remake {
        std::fs::create_dir_all(h.join("state")).unwrap();
        std::fs::write(h.join("config.toml"), config).unwrap();
        std::fs::write(h.join("state").join(lock), "synthetic foreign home lock").unwrap();
        std::fs::write(h.join("untouched"), "synthetic replacement home canary").unwrap();
        private_files(&h)
    } else {
        Vec::new()
    };
    std::fs::OpenOptions::new()
        .write(true)
        .open(release)
        .unwrap()
        .write_all(b"go\n")
        .unwrap();
    until("the replacement rejects its changed home", || {
        process.0.try_wait().unwrap().is_some()
            || if remake {
                private_files(&h) != before
            } else {
                h.exists()
            }
    });
    assert!(
        if remake {
            private_files(&h) == before
        } else {
            !h.exists()
        },
        "the replacement image wrote into a home that the old resident never held"
    );
    assert!(!process.0.wait().unwrap().success());
}

#[test]
fn worker_exec_cannot_recreate_a_home_removed_before_the_new_image_starts() {
    exec_keeps_the_original_home("worker", false);
}

#[test]
fn worker_exec_cannot_adopt_a_home_replaced_before_the_new_image_starts() {
    exec_keeps_the_original_home("worker", true);
}

#[test]
fn viewer_exec_cannot_recreate_a_home_removed_before_the_new_image_starts() {
    exec_keeps_the_original_home("viewer", false);
}

#[test]
fn viewer_exec_cannot_adopt_a_home_replaced_before_the_new_image_starts() {
    exec_keeps_the_original_home("viewer", true);
}

#[test]
fn malformed_or_wrong_role_exec_metadata_refuses_before_creating_a_home() {
    let scratch = tempfile::tempdir().unwrap();
    for (index, tail) in ["unknown:2:3", "worker.lock:2:3:extra", "view.lock:2:3"]
        .into_iter()
        .enumerate()
    {
        let home = scratch.path().join(format!("refused-{index}"));
        let environment =
            copied_command(env!("CARGO_BIN_EXE_oboete").as_ref(), scratch.path(), &[]);
        let result = Command::new("/bin/sh")
            .args([
                "-c",
                "export OBOETE_EXEC_HOME=\"$$:$1\"; exec \"$2\" --home \"$3\" worker --idle-ms 0",
                "private-handoff-fixture",
                tail,
                env!("CARGO_BIN_EXE_oboete"),
            ])
            .arg(&home)
            .current_dir(scratch.path())
            .env_clear()
            .envs(
                environment
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(!home.exists(), "rejected metadata created the home");
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.contains("invalid resident exec home handoff")
                || error.contains("resident exec changed its command")
        );
    }
}

fn binary_update_waits_for_loadable_config(viewer: bool) {
    use std::os::unix::fs::MetadataExt;
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    let config =
        format!("[worker]\nresident = true\n[summary]\ncurate = false\n[view]\nport = {port}\n");
    std::fs::write(h.join("config.toml"), &config).unwrap();
    let executable = scratch.path().join("oboete");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &executable).unwrap();
    let (mut process, port) = if viewer {
        copied_viewer(&executable, h, port)
    } else {
        (
            Worker(copied_spawn(&mut copied_command(
                &executable,
                h,
                &["worker"],
            ))),
            port,
        )
    };
    let pid = process.0.id();
    let lock = if viewer { "view.lock" } else { "worker.lock" };
    until("the original resident holds its home", || held(h, lock));
    if viewer {
        assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    }
    let image = format!("/proc/{pid}/exe");
    let original = std::fs::metadata(&image).unwrap();
    let generation =
        (!viewer).then(|| std::fs::read_to_string(h.join("state/worker-gen")).unwrap());
    // Pin the old marker: /proc/exe can change before the new listener is initialized.
    let previous_outcome =
        viewer.then(|| std::fs::File::open(h.join("state/view-outcome")).unwrap());
    let valid = std::fs::read_to_string(h.join("config.toml")).unwrap();
    std::fs::write(h.join("config.toml"), "[worker\nresident = true\n").unwrap();
    let next = scratch.path().join("oboete.next");
    std::fs::copy(env!("CARGO_BIN_EXE_oboete"), &next).unwrap();
    let replacement = std::fs::metadata(&next).unwrap();
    std::fs::rename(next, executable).unwrap();
    let waiting = Instant::now();
    let observation = if viewer {
        Duration::from_secs(65)
    } else {
        Duration::from_secs(1)
    };
    until_within(
        "the unloadable config keeps the old resident serving",
        Duration::from_secs(75),
        || {
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "replacement exited instead of keeping the old resident"
            );
            assert!(held(h, lock), "replacement released the old lock");
            let current = std::fs::metadata(&image).unwrap();
            assert_eq!(
                (current.dev(), current.ino()),
                (original.dev(), original.ino()),
                "replacement ran with unloadable config"
            );
            waiting.elapsed() >= observation
        },
    );
    if viewer {
        assert!(repos_response(h, port).starts_with("HTTP/1.1 200 OK\r\n"));
    }
    std::fs::write(h.join("config.toml"), valid).unwrap();
    until_within(
        "the repaired config allows the new image",
        Duration::from_secs(75),
        || {
            std::fs::metadata(&image).is_ok_and(|current| {
                (current.dev(), current.ino()) == (replacement.dev(), replacement.ino())
            })
        },
    );
    if viewer {
        assert!(
            ready_repos_response(h, port, previous_outcome.as_ref().unwrap(), &mut process.0)
                .starts_with("HTTP/1.1 200 OK\r\n")
        );
    } else {
        until("the new worker finished initialization", || {
            held(h, lock)
                && std::fs::read_to_string(h.join("state/worker-gen"))
                    .is_ok_and(|current| Some(&current) != generation.as_ref())
        });
    }
    assert!(held(h, lock));
    assert!(process.0.try_wait().unwrap().is_none());
}

#[test]
fn worker_update_defers_until_malformed_config_is_repaired() {
    binary_update_waits_for_loadable_config(false);
}

#[test]
fn viewer_update_defers_until_malformed_config_is_repaired() {
    binary_update_waits_for_loadable_config(true);
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
        for pid in started(self.0) {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
}

/// The processes running with `--home <home>` in their command line.
fn started(home: &Path) -> Vec<u32> {
    let want = format!("--home\0{}\0", home.display());
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        if String::from_utf8_lossy(&cmdline).contains(&want) {
            pids.push(pid);
        }
    }
    pids
}

/// The children of `pid` that have exited and wait to be reaped.
fn zombies(pid: u32) -> usize {
    let parent = pid.to_string();
    let mut found = 0;
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
        // pid (name) state ppid …: the name may hold spaces, so it is read after its `)`.
        let mut rest = stat
            .rsplit_once(')')
            .map_or("", |(_, r)| r)
            .split_whitespace();
        if rest.next() == Some("Z") && rest.next() == Some(parent.as_str()) {
            found += 1;
        }
    }
    found
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
    // They run in the home, not in the folder `oboete view` ran in, which can then go (R1;
    // Codex on #378).
    let pids = started(h);
    assert!(pids.len() >= 2, "{pids:?}");
    for pid in pids {
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap();
        assert_eq!(cwd, h.canonicalize().unwrap(), "{pid}");
    }
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
    // The resident viewer it started, which found the port taken and left, is reaped (Codex on
    // #378).
    assert_eq!(zombies(view.0.id()), 0);
    // So is the worker it started, when it leaves while this run serves on (Codex on #378).
    let parent = view.0.id().to_string();
    let worker = started(h)
        .into_iter()
        .find(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let mut rest = stat
                .rsplit_once(')')
                .map_or("", |(_, r)| r)
                .split_whitespace();
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            rest.nth(1) == Some(parent.as_str())
                && String::from_utf8_lossy(&cmdline).contains("\0worker")
        })
        .expect("the worker oboete view started");
    assert!(
        Command::new("kill")
            .arg(worker.to_string())
            .status()
            .unwrap()
            .success()
    );
    until("the worker is reaped", || {
        !Path::new(&format!("/proc/{worker}")).exists()
    });
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

/// R7: `--port 0` asks for this run's own viewer on any free port, in a resident home too: it
/// starts nothing and serves here (Codex on #378).
#[test]
fn view_with_port_0_serves_here_in_a_resident_home() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    // Keep the configured port reserved: a dropped free-port probe can be selected again by
    // the kernel's bind(0), which would make a correct independent viewer fail this assertion.
    let configured_port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = configured_port.local_addr().unwrap().port();
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
            .args(["view", "--port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
    assert!(view.0.try_wait().unwrap().is_none());
    assert!(!held(h, "view.lock") && !held(h, "worker.lock"));
}

/// R4: a resident worker whose store does not open still starts the viewer first, so the page
/// is there when the store needs a look (Codex on #378).
#[test]
fn a_worker_whose_store_does_not_open_still_starts_the_viewer() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
    )
    .unwrap();
    std::fs::write(h.join("raw.db"), b"not a database, only text").unwrap();
    let _detached = Detached(h);
    let worker = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(h)
        .arg("worker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!worker.success());
    until("the viewer", || held(h, "view.lock"));
}

/// R7: `oboete view` says so when the worker it is to start cannot be (Codex on #378); the
/// viewer still comes up.
#[test]
fn view_says_when_the_worker_does_not_start() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let port = free_port();
    std::fs::write(
        h.join("config.toml"),
        format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
    )
    .unwrap();
    std::fs::create_dir_all(h.join("state").join("worker.lock")).unwrap();
    let _detached = Detached(h);
    let out = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(h)
        .arg("view")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("the worker did not start"), "{said}");
    assert!(held(h, "view.lock"));
}
