use super::*;

/// What a df test that is not about `DF_TIMEOUT` gives it: a loaded runner can take longer
/// than that to start a shell, and the answer then read as none (the coverage run of
/// 2026-11-11).
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const PATIENT: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(windows)]
#[test]
fn w6d_df_windows_pipe_retains_success_and_failure_results() {
    use std::process::{Command, Stdio};
    let root = tempfile::tempdir().unwrap();
    let system = std::env::var_os("SystemRoot").unwrap();
    let exe = PathBuf::from(&system).join("System32").join("cmd.exe");
    for (body, limit, expected) in [
        (
            "echo Filesystem 1024-blocks Used Available Capacity Mounted & echo fake 100 10 4 capacity /",
            PATIENT,
            Some(4096),
        ),
        (
            "echo Filesystem 1024-blocks Used Available Capacity Mounted & echo fake 100 10 4 capacity / & exit /b 7",
            PATIENT,
            None,
        ),
        // The one about the limit: it never ends.
        ("for /L %i in (1,1,2147483647) do @rem", DF_TIMEOUT, None),
    ] {
        let mut child = Command::new(&exe)
            .env_clear()
            .env("SystemRoot", &system)
            .env("TEMP", root.path())
            .env("TMP", root.path())
            .current_dir(root.path())
            .args(["/d", "/c", body])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        assert_eq!(
            df_bounded_child(child, |chunk| df_read_available(&mut stdout, chunk), limit),
            expected
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn w6d_df_requires_successful_output_and_a_representable_size() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("df");
    for (status, available, expected) in [
        (0, "4", Some(4096)),
        (7, "4", None),
        (0, "18446744073709551615", None),
    ] {
        std::fs::write(&program,format!("#!/bin/sh\nprintf '%s\\n' 'Filesystem 1024-blocks Used Available Capacity Mounted' 'fake 100 10 {available} 1% /'\nexit {status}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(df(program.as_os_str(), dir.path(), PATIENT), expected);
    }
    // One slower than `DF_TIMEOUT`, as a shell on a loaded runner is: the limit given is the
    // one kept.
    std::fs::write(&program, "#!/bin/sh\nsleep 1\nprintf '%s\\n' 'Filesystem 1024-blocks Used Available Capacity Mounted' 'fake 100 10 4 1% /'\n").unwrap();
    assert_eq!(df(program.as_os_str(), dir.path(), PATIENT), Some(4096));
    assert_eq!(df(program.as_os_str(), dir.path(), DF_TIMEOUT), None);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn w6d_df_bounds_inherited_stdout_and_excess_output() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("df");
    let table = "printf '%s\\n' 'Filesystem 1024-blocks Used Available Capacity Mounted' 'fake 100 10 4 1% /'";
    let lines = |n: u32| {
        format!(
            "i=0\nwhile [ \"$i\" -lt {n} ]; do printf '%s\\n' '{}'; i=$((i+1)); done",
            "x".repeat(64)
        )
    };
    for (script, limit, expected) in [
        // Its output never ends: a child of its own holds it open, and the limit ends the wait.
        (
            format!("#!/bin/sh\n{table}\nsleep 2 &\nexit 0\n"),
            DF_TIMEOUT,
            None,
        ),
        // A table that reads, then more than the output's cap: only the cap makes it none,
        // as the same table under the cap shows.
        (
            format!("#!/bin/sh\n{table}\n{}\n", lines(200)),
            PATIENT,
            None,
        ),
        (
            format!("#!/bin/sh\n{table}\n{}\n", lines(20)),
            PATIENT,
            Some(4096),
        ),
    ] {
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(df(program.as_os_str(), dir.path(), limit), expected);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "bounded df did not return"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn w6d_df_reaps_a_normally_killable_timed_out_child() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("df");
    std::fs::write(
        &program,
        "#!/bin/sh\necho $$ > \"$2/df.pid\"\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Long enough for the shell to have written its id on a loaded runner; the wait ends
    // at the limit all the same, the child never ending.
    let limit = std::time::Duration::from_secs(3);
    assert_eq!(df(program.as_os_str(), dir.path(), limit), None);
    let pid = std::fs::read_to_string(dir.path().join("df.pid"))
        .unwrap()
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();
    assert!(pid > 0);
    // SAFETY: signal zero only observes the PID recorded by this private spawned fixture.
    let status = unsafe { libc::kill(pid, 0) };
    let error = std::io::Error::last_os_error();
    assert!(
        status == -1 && error.raw_os_error() == Some(libc::ESRCH),
        "timed-out df child was not reaped"
    );
}

#[test]
fn a_marker_cut_short_is_written_whole_after_the_next_write() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join("state")).unwrap();
    std::fs::write(marker(home), "").unwrap();
    prepare(home);
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), SIZE as u64);
    assert_eq!(since(home), None);
    // A failure cut short keeps what it said: `clear` decides by the times.
    std::fs::write(marker(home), "failed disk-full 17").unwrap();
    prepare(home);
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), SIZE as u64);
    assert_eq!(since(home), Some((Class::DiskFull, 17)));
    clear(home, 10);
    assert_eq!(since(home), Some((Class::DiskFull, 17)));
    clear(home, 20);
    assert_eq!(since(home), None);
}

/// The system's own `df -Pk`, given the time a loaded runner needs: its table is the one read.
/// (A df that never ends is the reap test's, above.)
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_systems_df_is_read() {
    let dir = tempfile::tempdir().unwrap();
    assert!(df("df".as_ref(), dir.path(), PATIENT).is_some());
}

/// SQLite reports a full disk as an I/O error while it sets up its journal: a new record of
/// one is a full disk's when under 1 MiB is free, and the disk is asked for a new record of
/// an I/O error alone.
#[test]
fn an_io_error_on_a_full_disk_is_recorded_as_a_full_disk() {
    const MIB: u64 = 1024 * 1024;
    for (class, free, recorded) in [
        (Class::Io, Some(0), Class::DiskFull),
        (Class::Io, Some(MIB - 1), Class::DiskFull),
        (Class::Io, Some(MIB), Class::Io),
        (Class::Io, None, Class::Io),
    ] {
        let home = tempfile::tempdir().unwrap();
        mark_with(home.path(), class, 10, || free);
        assert_eq!(since(home.path()), Some((recorded, 10)), "{free:?}");
        // A record already there keeps its class, and the disk is not asked again.
        mark_with(home.path(), Class::Io, 20, || panic!("asked again"));
        assert_eq!(since(home.path()), Some((recorded, 10)));
    }
    let home = tempfile::tempdir().unwrap();
    mark_with(home.path(), Class::Busy, 10, || {
        panic!("asked for a busy store")
    });
    assert_eq!(since(home.path()), Some((Class::Busy, 10)));
}

fn sqlite(code: i32) -> anyhow::Error {
    anyhow::Error::new(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(code),
        None,
    ))
    .context("append")
}

#[test]
fn write_failures_are_classified_through_the_context_chain() {
    assert_eq!(
        classify(&sqlite(rusqlite::ffi::SQLITE_FULL)),
        Class::DiskFull
    );
    assert_eq!(classify(&sqlite(rusqlite::ffi::SQLITE_IOERR)), Class::Io);
    assert_eq!(classify(&sqlite(rusqlite::ffi::SQLITE_BUSY)), Class::Busy);
    assert_eq!(classify(&sqlite(rusqlite::ffi::SQLITE_LOCKED)), Class::Busy);
    let enospc = std::io::Error::from(std::io::ErrorKind::StorageFull);
    assert_eq!(
        classify(&anyhow::Error::new(enospc).context("write")),
        Class::DiskFull
    );
    let io = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert_eq!(classify(&anyhow::Error::new(io)), Class::Io);
    let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert_eq!(classify(&anyhow::Error::new(json)), Class::Other);
    let newer = anyhow::Error::new(crate::raw::NewerFormat { found: 9 }).context("open");
    assert_eq!(classify(&newer), Class::Newer);
}

#[test]
fn the_marker_keeps_its_size_and_the_first_failure_time() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    assert_eq!(since(home), None);
    prepare(home);
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
    assert_eq!(since(home), None);
    mark(home, Class::DiskFull, 1_000);
    assert_eq!(since(home), Some((Class::DiskFull, 1_000)));
    mark(home, Class::Io, 2_000);
    assert_eq!(since(home), Some((Class::DiskFull, 1_000)));
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
    clear(home, 3_000);
    assert_eq!(since(home), None);
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
    // A failure before any success still leaves a marker when the disk allows it.
    let fresh = tempfile::tempdir().unwrap();
    mark(fresh.path(), Class::Busy, 5);
    assert_eq!(since(fresh.path()).map(|f| f.0), Some(Class::Busy));
}

#[test]
fn transitions_follow_when_each_write_ended_not_when_its_hook_reached_the_marker() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    prepare(home);
    clear(home, 2_000); // a write that ended at 2,000 succeeded
    mark(home, Class::Busy, 1_000); // one that failed earlier arrives late: not a failure now
    assert_eq!(since(home), None);
    mark(home, Class::Busy, 3_000);
    clear(home, 2_500); // a success from before that failure arrives late: the failure stays
    assert_eq!(since(home), Some((Class::Busy, 3_000)));
    mark(home, Class::Busy, 4_000);
    clear(home, 3_500); // still before the last failure
    assert_eq!(since(home), Some((Class::Busy, 3_000)));
    clear(home, 4_500);
    assert_eq!(since(home), None);
    assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
}

#[test]
fn a_marker_held_by_a_stopped_process_never_blocks_a_hook() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    prepare(home);
    clear(home, 5);
    let held = std::fs::OpenOptions::new()
        .write(true)
        .open(marker(home))
        .unwrap();
    held.lock().unwrap(); // a hook stopped inside its update
    let t = std::time::Instant::now();
    mark(home, Class::Busy, 9);
    clear(home, 10);
    prepare(home);
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(since(home), None); // left as it was
}

#[test]
fn a_clock_that_went_back_neither_hides_a_failure_nor_keeps_one() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let minute = 60_000_000_000;
    prepare(home);
    clear(home, 10 * minute);
    mark(home, Class::Busy, 9 * minute); // the clock stepped back a minute
    assert_eq!(since(home), Some((Class::Busy, 9 * minute)));
    clear(home, 8 * minute); // and back again: a later success still clears it
    assert_eq!(since(home), None);
}

#[test]
fn a_tie_keeps_the_failure() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    prepare(home);
    clear(home, 5);
    mark(home, Class::Busy, 5); // a success and a failure that ended together
    assert_eq!(since(home), Some((Class::Busy, 5)));
    clear(home, 5);
    assert_eq!(since(home), Some((Class::Busy, 5)));
    clear(home, 6);
    assert_eq!(since(home), None);
}

#[test]
fn the_first_marker_is_created_under_the_lock_and_ordered_like_any_other() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().join("never-prepared");
    clear(&home, 20); // a success that ended later reached the marker first
    mark(&home, Class::Busy, 10);
    assert_eq!(since(&home), None);
    mark(&home, Class::Busy, 30);
    assert_eq!(since(&home), Some((Class::Busy, 30)));
}

#[test]
fn the_line_names_the_time_in_utc_and_the_class() {
    // 2026-09-27 04:05 UTC.
    let line = line((Class::DiskFull, 1_790_481_900_000_000_000), 0);
    assert!(
        line.contains("recording has failed since 2026-09-27 04:05 UTC"),
        "{line}"
    );
    assert!(line.contains("disk full"), "{line}");
    // docs/store-format.md: the marker keeps the class, and the line says what to do.
    let home = tempfile::tempdir().unwrap();
    prepare(home.path());
    mark(home.path(), Class::Newer, 1_000);
    let failed = since(home.path()).unwrap();
    assert_eq!(failed.0, Class::Newer);
    let said = super::line(failed, 0);
    assert!(
        said.contains("a newer oboete wrote this home: update oboete"),
        "{said}"
    );
}
