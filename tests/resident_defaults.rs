//! Resident Slice 5: setup, settings and doctor use the same home configuration.
//! All native commands run in a private home, with no provider or detached child.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oboete"));
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
        .env("OBOETE_NO_SPAWN", "1");
    command
}

fn run(home: &Path, args: &[&str]) -> Output {
    let result = command(home, args).output().unwrap();
    assert!(
        result.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

struct Owned(std::process::Child);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn setup_waits_for_the_shared_config_writer_before_reading_defaults() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir(h.join("state")).unwrap();
    let fence = std::fs::File::create(h.join("state/config.lock")).unwrap();
    fence.lock().unwrap();
    let mut setup = Owned(command(h, &["setup", "claude"]).spawn().unwrap());
    let start = Instant::now();
    loop {
        if h.join("config.toml").exists() || setup.0.try_wait().unwrap().is_some() {
            break;
        }
        let waiting =
            std::fs::read_to_string(format!("/proc/{}/wchan", setup.0.id())).unwrap_or_default();
        if waiting.contains("locks_") || waiting.contains("flock_") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "setup did not reach its writer lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !h.join("config.toml").exists(),
        "setup wrote while another config writer held the lock"
    );
    assert!(setup.0.try_wait().unwrap().is_none());
    // The first writer's explicit choice is what the waiting setup must read and retain.
    let chosen = "[summary]\ncurate = false\n[worker]\nresident = false\n[view]\nport = 17374\n";
    std::fs::write(h.join("config.toml"), chosen).unwrap();
    fence.unlock().unwrap();
    assert!(setup.0.wait().unwrap().success());
    assert_eq!(
        std::fs::read_to_string(h.join("config.toml")).unwrap(),
        chosen
    );
}

#[test]
fn doctor_reads_resident_locks_and_outcomes_without_starting_or_renumbering_them() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(h.join("config.toml"), "providers = []\n[summary]\ncurate = false\n[worker]\nresident = true\n[view]\nport = 17374\n").unwrap();
    let first = run(h, &["doctor"]);
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(
        text.contains("worker: resident, not running now (the next hook starts it)"),
        "{text}"
    );
    assert!(text.contains("page: not running:"), "{text}");
    assert!(!h.join("state/worker.lock").exists());
    assert!(!h.join("state/view.lock").exists());

    std::fs::create_dir(h.join("state")).unwrap();
    let worker = std::fs::File::create(h.join("state/worker.lock")).unwrap();
    worker.lock().unwrap();
    let viewer = std::fs::File::create(h.join("state/view.lock")).unwrap();
    viewer.lock().unwrap();
    std::fs::write(h.join("state/view-outcome"), "listening 17374\n").unwrap();
    let running = run(h, &["doctor"]);
    let text = String::from_utf8(running.stdout).unwrap();
    assert!(text.contains("worker: resident, running"), "{text}");
    assert!(
        text.contains("page: http://127.0.0.1:17374 is up"),
        "{text}"
    );
    assert!(!h.join("state/worker-gen").exists());
    assert!(!h.join("state/view-token").exists());

    std::fs::write(h.join("state/view-outcome"), "listening 17375\n").unwrap();
    let stale = run(h, &["doctor"]);
    assert!(
        String::from_utf8(stale.stdout)
            .unwrap()
            .contains("page: not running:")
    );
    drop(viewer);
    std::fs::write(h.join("state/view-outcome"), "port in use\n").unwrap();
    let busy = run(h, &["doctor"]);
    let text = String::from_utf8(busy.stdout).unwrap();
    assert!(
        text.contains("another program or another oboete home holds port 17374"),
        "{text}"
    );
    assert!(!h.join("state/worker-gen").exists());
}

#[test]
fn rebuild_owns_its_status_lock_and_releases_it_on_success_failure_and_kill() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(
        h.join("config.toml"),
        "providers = []\n[summary]\ncurate = false\n[worker]\nresident = true\n",
    )
    .unwrap();
    run(
        h,
        &["pref", "add", "Synthetic preference for rebuild status"],
    );
    run(h, &["rebuild"]);
    let open_status = || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(h.join("state/rebuild.lock"))
            .expect("rebuild has its own persistent status lock")
    };
    let status = open_status();
    status.try_lock().unwrap();
    drop(status);

    // A transient shared probe's retry is synchronized at genuine contention in worker's unit
    // test. The native command keeps the bound and database-preservation proof below.
    // A holder which does not release must fail within a bound, before moving the database.
    let probe = open_status();
    probe.lock().unwrap();
    let before = std::fs::read(h.join("knowledge.db")).unwrap();
    let mut blocked = Owned(
        command(h, &["rebuild"])
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    let outcome = loop {
        if let Some(outcome) = blocked.0.try_wait().unwrap() {
            break outcome;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!outcome.success());
    let mut error = String::new();
    std::io::Read::read_to_string(blocked.0.stderr.as_mut().unwrap(), &mut error).unwrap();
    assert!(
        error.contains("the rebuild status lock is in use"),
        "{error}"
    );
    assert_eq!(std::fs::read(h.join("knowledge.db")).unwrap(), before);
    drop(probe);

    std::fs::write(h.join("knowledge.db"), b"invented damaged database").unwrap();
    let failed = command(h, &["rebuild"]).output().unwrap();
    assert!(!failed.status.success());
    let status = open_status();
    status.try_lock().unwrap();
    drop(status);

    // A reader holds the swap while the actual rebuild owns its status lock.
    let reader = std::fs::OpenOptions::new()
        .write(true)
        .open(h.join("raw.lock"))
        .unwrap();
    reader.lock_shared().unwrap();
    let mut rebuilding = Owned(command(h, &["rebuild"]).spawn().unwrap());
    let start = Instant::now();
    loop {
        let status = open_status();
        let locked = matches!(status.try_lock(), Err(std::fs::TryLockError::WouldBlock));
        drop(status);
        if locked {
            break;
        }
        assert!(
            rebuilding.0.try_wait().unwrap().is_none(),
            "rebuild exited before taking its status lock"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "rebuild never held its status lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    rebuilding.0.kill().unwrap();
    rebuilding.0.wait().unwrap();
    let status = open_status();
    status.try_lock().unwrap();
}

#[test]
fn broken_corpus_settings_do_not_prevent_agent_wiring_but_keep_setup_failed() {
    for invalid_config in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let original = if invalid_config {
            "[redaction]\nrules = 'synthetic-private-config-canary'\n"
        } else {
            "providers = []\n[worker]\nresident = false\n"
        };
        std::fs::write(h.join("config.toml"), original).unwrap();
        if !invalid_config {
            // A write/lock failure is deterministic even when tests run as root.
            std::fs::create_dir_all(h.join("state/config.lock")).unwrap();
        }
        let result = command(h, &["setup", "claude"]).output().unwrap();
        assert!(
            !result.status.success(),
            "defaults failure was reported as success"
        );
        assert!(
            h.join("claude/settings.json").is_file(),
            "a corpus settings failure prevented independent agent wiring"
        );
        assert_eq!(
            std::fs::read_to_string(h.join("config.toml")).unwrap(),
            original
        );
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            diagnostics.contains("claude: hooks written to"),
            "{diagnostics}"
        );
        assert!(
            diagnostics.contains("resident defaults not written"),
            "{diagnostics}"
        );
        assert!(!diagnostics.contains("synthetic-private-config-canary"));
        assert!(!h.join("state/worker.lock").exists());
        assert!(!h.join("state/view.lock").exists());
    }
}

#[test]
fn setup_fills_resident_defaults_without_overwriting_an_explicit_choice() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let out = run(h, &["setup", "claude"]);
    let config = h.join("config.toml");
    let first = std::fs::read_to_string(&config).expect("setup writes resident defaults");
    let parsed: toml::Value = toml::from_str(&first).unwrap();
    assert_eq!(parsed["worker"]["resident"].as_bool(), Some(true));
    assert_eq!(parsed["view"]["port"].as_integer(), Some(17373));
    assert!(String::from_utf8_lossy(&out.stdout).contains("http://127.0.0.1:17373"));
    assert!(!h.join("state/worker.lock").exists());
    assert!(!h.join("state/view.lock").exists());
    run(h, &["setup", "claude"]);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), first);

    let chosen = "# Keep the owner's choices\n[summary]\ncurate = false\n[worker]\nresident = false # intentionally off\n[view]\nport = 17374 # another home\n";
    std::fs::write(&config, chosen).unwrap();
    let out = run(h, &["setup", "claude"]);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), chosen);
    assert!(String::from_utf8_lossy(&out.stdout).contains("http://127.0.0.1:17374"));
    run(h, &["setup", "claude", "--remove"]);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), chosen);
}
