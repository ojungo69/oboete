//! MUST-M16: a full disk never blocks the agent; doctor and the next SessionStart say recording has
//! failed.
#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_never_blocks_the_agent_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path().join("home");
    std::fs::create_dir(&h).unwrap();
    let b = env!("CARGO_BIN_EXE_oboete");
    // In a private mount namespace a 1 MiB tmpfs becomes the home. One hook creates raw.db and the
    // marker, `dd` takes the rest of the space, then one more hook, doctor and a new SessionStart.
    let script = format!(
        r#"
        echo inside
        mount -t tmpfs -o size=1m tmpfs {h} || exit 77
        # No worker: one started by a hook would write to the tmpfs while `dd` fills it.
        export OBOETE_NO_SPAWN=1
        echo '{{"session_id":"s","prompt":"first"}}' | {b} --home {h} hook claude UserPromptSubmit
        test -f {h}/state/recording-failed || echo "no marker after the first hook"
        dd if=/dev/zero of={h}/fill bs=4k 2>/dev/null
        echo '{{"session_id":"s","prompt":"second"}}' | {b} --home {h} hook claude UserPromptSubmit; echo "hook=$?"
        {b} --home {h} doctor; echo "doctor=$?"
        echo '{{"session_id":"t","source":"startup"}}' | {b} --home {h} hook claude SessionStart
    "#,
        h = h.display()
    );
    let out = std::process::Command::new("unshare")
        .args(["-rm", "sh", "-c", &script])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    // No user namespace (GitHub's Ubuntu runners restrict them with AppArmor) or no tmpfs in one:
    // the script never ran, or stopped at the mount.
    if !s.contains("inside") || out.status.code() == Some(77) {
        eprintln!(
            "skipped: no unprivileged tmpfs mount here: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return;
    }
    assert!(!s.contains("no marker"), "{s}");
    assert!(s.contains("hook=0"), "{s}"); // the agent is not blocked
    assert!(!s.contains("doctor=0"), "{s}"); // doctor is red
    assert_eq!(s.matches("recording has failed since").count(), 2, "{s}"); // doctor and SessionStart
    assert!(s.contains("(disk full)"), "{s}"); // SQLite's I/O error on a full disk is named for what it is
}
