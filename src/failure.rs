//! Write failures (MUST-M16): a hook that cannot record never blocks the agent, and the failure
//! is kept outside the database, in a small file whose blocks exist before the disk fills, so
//! `oboete doctor` and the next SessionStart can say that recording has failed.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    DiskFull,
    Io,
    Busy,
    Other,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Class::DiskFull => "disk-full",
            Class::Io => "io",
            Class::Busy => "busy",
            Class::Other => "other",
        }
    }

    fn parse(name: &str) -> Class {
        match name {
            "disk-full" => Class::DiskFull,
            "io" => Class::Io,
            "busy" => Class::Busy,
            _ => Class::Other,
        }
    }
}

/// The marker's size: rewritten in place, it needs no new block on a full disk. A copy-on-write
/// filesystem (APFS, btrfs) can still refuse it; doctor's free-space check covers that.
const SIZE: usize = 64;

/// SQLITE_FULL or ENOSPC, SQLITE_IOERR or another I/O error, SQLITE_BUSY or SQLITE_LOCKED, the
/// first found along the error's chain; anything else (a malformed payload) is `Other`.
pub fn classify(e: &anyhow::Error) -> Class {
    for cause in e.chain() {
        if let Some(code) = cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
        {
            use rusqlite::ErrorCode as C;
            match code {
                C::DiskFull => return Class::DiskFull,
                C::SystemIoFailure => return Class::Io,
                C::DatabaseBusy | C::DatabaseLocked => return Class::Busy,
                _ => {}
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return if io.kind() == std::io::ErrorKind::StorageFull {
                Class::DiskFull
            } else {
                Class::Io
            };
        }
    }
    Class::Other
}

/// What the marker holds: when the last write that succeeded ended, or the class and the first
/// and last end times of the failures since. Hooks overlap, and one can reach the marker long
/// after its write ended (the free-space probe takes up to 500 ms), so each transition compares
/// these times instead of trusting the order in which hooks arrive.
#[derive(Debug, PartialEq)]
enum State {
    Ok(i64),
    Failed { class: Class, first: i64, last: i64 },
}

impl State {
    fn parse(text: &str) -> Option<State> {
        let mut words = text.split_whitespace();
        match words.next()? {
            "ok" => Some(State::Ok(
                words.next().and_then(|t| t.parse().ok()).unwrap_or(0),
            )),
            "failed" => {
                let class = Class::parse(words.next()?);
                let first = words.next()?.parse().ok()?;
                let last = words.next().and_then(|t| t.parse().ok()).unwrap_or(first);
                Some(State::Failed { class, first, last })
            }
            _ => None,
        }
    }

    fn text(&self) -> String {
        padded(&match self {
            State::Ok(at) => format!("ok {at}"),
            State::Failed { class, first, last } => {
                format!("failed {} {first} {last}", class.name())
            }
        })
    }
}

/// After a successful write: the marker exists at its full size before any failure needs it. One
/// cut short (a failure's own write that ran out of space) is written again whole, with what it
/// said, under the lock `mark` and `clear` take: only its size changes, never its state.
pub fn prepare(home: &Path) {
    use std::io::{Read, Seek, Write};
    let path = marker(home);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == SIZE as u64) {
        return;
    }
    let _ = std::fs::create_dir_all(home.join("state"));
    let Ok(mut f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    else {
        return;
    };
    let mut buf = Vec::new();
    if locked(&f).is_err() || f.read_to_end(&mut buf).is_err() || buf.len() == SIZE {
        return;
    }
    let state = State::parse(&String::from_utf8_lossy(&buf)).unwrap_or(State::Ok(0));
    let _ = f
        .set_len(0)
        .and_then(|_| f.seek(std::io::SeekFrom::Start(0)))
        .and_then(|_| f.write_all(state.text().as_bytes()));
}

/// After a write that failed when it ended at `at`: a failure record with the class and the
/// first failure's time, kept until a write that ends later succeeds. Nothing when a write that
/// ended after `at` has already succeeded.
pub fn mark(home: &Path, class: Class, at: i64) {
    let failing = std::fs::read_to_string(marker(home))
        .ok()
        .and_then(|t| State::parse(&t))
        .is_some_and(|s| matches!(s, State::Failed { .. }));
    // SQLite reports a full disk as an I/O error while it sets up its journal (SQLITE_IOERR 4874
    // on a full tmpfs): the free space tells them apart. Only a new record needs the class.
    let class =
        if !failing && class == Class::Io && free_bytes(home).is_some_and(|b| b < 1024 * 1024) {
            Class::DiskFull
        } else {
            class
        };
    let _ = transition(home, |s| match s {
        Some(State::Ok(ok)) if later(ok, at) => None,
        Some(State::Failed { class, first, last }) => Some(State::Failed {
            class,
            first,
            last: last.max(at),
        }),
        _ => Some(State::Failed {
            class,
            first: at,
            last: at,
        }),
    });
}

pub fn since(home: &Path) -> Option<(Class, i64)> {
    parse_since(&std::fs::read_to_string(marker(home)).ok()?)?
}

/// The native marker parser, retaining invalid input separately from a successful clear.
pub(crate) fn parse_since(text: &str) -> Option<Option<(Class, i64)>> {
    Some(match State::parse(text)? {
        State::Failed { class, first, .. } => Some((class, first)),
        State::Ok(_) => None,
    })
}

/// After a write that succeeded when it ended at `at`, in place. Nothing when a write that ended
/// after `at` has failed.
pub fn clear(home: &Path, at: i64) {
    let _ = transition(home, |s| match s {
        // A tie keeps the failure: when the order is unknown, report rather than hide.
        Some(State::Failed { last, .. }) if last == at || later(last, at) => None,
        Some(State::Ok(ok)) if ok >= at => None,
        _ => Some(State::Ok(at)),
    });
}

/// How far apart two overlapping hooks can end: a stored time further ahead of a new one than
/// this is a wall clock that went back (a VM resumed, a time correction), not an overlap, so the
/// new state wins instead of waiting for the clock to catch up.
const OVERLAP_NS: i64 = 5_000_000_000;

/// Whether the stored time `stored` is from a write that ended after the one that ended at `at`.
fn later(stored: i64, at: i64) -> bool {
    stored > at && stored - at <= OVERLAP_NS
}

/// The marker read, changed by `change` (`None` leaves it) and written back in place, under the
/// file's lock, so two hooks cannot interleave their read and write. Created when missing (the
/// first write failed before any `prepare`), under the same lock: creating it may fail on a full
/// disk too.
fn transition(
    home: &Path,
    change: impl FnOnce(Option<State>) -> Option<State>,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, Write};
    std::fs::create_dir_all(home.join("state"))?;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(marker(home))?;
    locked(&f)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if let Some(next) = change(State::parse(&String::from_utf8_lossy(&buf))) {
        f.seek(std::io::SeekFrom::Start(0))?;
        f.write_all(next.text().as_bytes())?;
    }
    Ok(())
}

/// The time the marker's states are ordered by: nanoseconds since the Unix epoch, so two hooks
/// ending in the same millisecond still have an order (and a tie keeps a failure).
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// What doctor and SessionStart say while recording fails.
pub fn line((class, ts): (Class, i64)) -> String {
    let what = match class {
        Class::DiskFull => "disk full",
        Class::Io => "I/O error",
        Class::Busy => "store busy",
        Class::Other => "error",
    };
    format!(
        "oboete: recording has failed since {} ({what}); events from then on are not recorded. Run `oboete doctor`.",
        crate::db::utc(ts / 1_000_000)
    )
}

/// How long a hook waits for another to finish its marker update before it leaves the marker as
/// it is: the lock is held for one small read and write, so only a stopped process holds it long,
/// and a hook never waits on one (MUST-M16: a failure never blocks the agent).
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

fn locked(f: &std::fs::File) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::other(
                    "the marker is locked by another process",
                ));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

fn marker(home: &Path) -> PathBuf {
    home.join("state").join("recording-failed")
}

fn padded(text: &str) -> String {
    format!("{text:<width$}\n", width = SIZE - 1)
}

/// Free bytes where `home` lives, from `GetDiskFreeSpaceExW` on Windows and successful `df -Pk`
/// elsewhere (or where Windows refuses); unavailable data stays None. Either gets `DF_TIMEOUT`.
/// ponytail: fixed OS utility on Unix; use native filesystem queries if df availability becomes a limit.
pub fn free_bytes(home: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        // Asked on a thread: a stalled network volume must not hold a hook past `DF_TIMEOUT`
        // (MUST-M16); a late answer is dropped with its thread.
        let (send, answer) = std::sync::mpsc::channel();
        let path = home.to_owned();
        std::thread::spawn(move || send.send(windows_free_bytes(&path)));
        match answer.recv_timeout(DF_TIMEOUT) {
            Ok(Some(bytes)) => return Some(bytes),
            Ok(None) => {}
            Err(_) => return None,
        }
    }
    df("df".as_ref(), home)
}

#[cfg(windows)]
fn windows_free_bytes(home: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut path: Vec<u16> = home.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return None;
    }
    path.push(0);
    let mut available = 0u64;
    // SAFETY: the terminated path and writable byte count stay alive throughout the call.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

/// How long `df` gets. On a stalled network or FUSE mount it can hang, and a hook calls it after
/// a failed write: it must never block the agent (MUST-M16).
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const DF_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn df(program: &std::ffi::OsStr, home: &Path) -> Option<u64> {
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};

    let (mut stdout, writer) = UnixStream::pair().ok()?;
    stdout.set_nonblocking(true).ok()?;
    let mut command = Command::new(program);
    command
        .arg("-Pk")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::null());
    let child = crate::provider::own_group(&mut command).spawn().ok()?;
    drop(command); // Release the parent's configured stdout writer so EOF is observable.
    df_bounded_child(child, |chunk| stdout.read(chunk))
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn df_bounded_child(
    mut child: std::process::Child,
    mut read: impl FnMut(&mut [u8]) -> std::io::Result<usize>,
) -> Option<u64> {
    use std::io::ErrorKind;
    use std::time::{Duration, Instant};
    const OUTPUT_LIMIT: usize = 8 * 1024;
    // A killed child gets this long to be reaped. 50 ms left it a zombie on GitHub's macOS
    // runners, whose 5 ms sleeps overran it (3 of about 15 runs, 2026-10-09); df itself takes
    // milliseconds, so the work keeps most of the half second.
    const CLEANUP_RESERVE: Duration = Duration::from_millis(200);
    const POLL: Duration = Duration::from_millis(5);
    let deadline = Instant::now() + DF_TIMEOUT;
    let work_deadline = deadline - CLEANUP_RESERVE;
    let mut bytes = Vec::new();
    let mut eof = false;

    'poll: loop {
        let mut chunk = [0u8; 1024];
        loop {
            match read(&mut chunk) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n)
                    if bytes
                        .len()
                        .checked_add(n)
                        .is_some_and(|len| len <= OUTPUT_LIMIT) =>
                {
                    bytes.extend_from_slice(&chunk[..n]);
                }
                Ok(_) => break 'poll, // cap exceeded: terminate owned child/group
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break 'poll,
            }
        }
        if eof {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        parse_df_output(&bytes)
                    } else {
                        None
                    };
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
        let now = Instant::now();
        if now >= work_deadline {
            break;
        }
        std::thread::sleep(POLL.min(work_deadline.saturating_duration_since(now)));
    }

    // The child has not been reaped, so its process-group ID cannot be reused yet.
    crate::provider::kill_tree(&mut child);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())))
            }
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn parse_df_output(output: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(output); // preserve the existing numeric-field behavior
    let kb = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()?;
    kb.checked_mul(1024)
}

#[cfg(windows)]
fn df_read_available(
    stdout: &mut std::process::ChildStdout,
    chunk: &mut [u8],
) -> std::io::Result<usize> {
    use std::io::{ErrorKind, Read};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};

    let mut available = 0u32;
    // SAFETY: stdout owns this live read handle for the whole call. This zero-buffer
    // peek writes only the available-byte count; no other reader consumes the pipe.
    let ok = unsafe {
        PeekNamedPipe(
            stdout.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
            Ok(0) // all writers closed: EOF
        } else {
            Err(error)
        };
    }
    if available == 0 {
        return Err(ErrorKind::WouldBlock.into());
    }
    // Peek did not consume bytes; this is the sole reader, and a request no larger
    // than the reported queue is not waiting for future child output.
    let count = chunk.len().min(available as usize);
    match stdout.read(&mut chunk[..count]) {
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(0),
        result => result,
    }
}

#[cfg(windows)]
fn df(program: &std::ffi::OsStr, home: &Path) -> Option<u64> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(program);
    command
        .arg("-Pk")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    drop(command);
    let Some(mut stdout) = child.stdout.take() else {
        return df_bounded_child(child, |_| Err(std::io::ErrorKind::Other.into()));
    };
    df_bounded_child(child, |chunk| df_read_available(&mut stdout, chunk))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn df(_program: &std::ffi::OsStr, _home: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn w6d_df_windows_pipe_retains_success_and_failure_results() {
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        let exe = PathBuf::from(&system).join("System32").join("cmd.exe");
        for (body, expected) in [
            (
                "echo Filesystem 1024-blocks Used Available Capacity Mounted & echo fake 100 10 4 capacity /",
                Some(4096),
            ),
            (
                "echo Filesystem 1024-blocks Used Available Capacity Mounted & echo fake 100 10 4 capacity / & exit /b 7",
                None,
            ),
            ("for /L %i in (1,1,2147483647) do @rem", None),
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
                df_bounded_child(child, |chunk| df_read_available(&mut stdout, chunk)),
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
            assert_eq!(df(program.as_os_str(), dir.path()), expected);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn w6d_df_bounds_inherited_stdout_and_excess_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("df");
        for script in [
            "#!/bin/sh\nprintf '%s\\n' 'Filesystem 1024-blocks Used Available Capacity Mounted' 'fake 100 10 4 1% /'\nsleep 2 &\nexit 0\n",
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 200 ]; do printf '%s\\n' 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; i=$((i+1)); done\n",
        ] {
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(df(program.as_os_str(), dir.path()), None);
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
        assert_eq!(df(program.as_os_str(), dir.path()), None);
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_df_that_hangs_is_given_up_on() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let stuck = dir.path().join("df");
        std::fs::write(&stuck, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755)).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(df(stuck.as_os_str(), dir.path()), None);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
        assert!(free_bytes(dir.path()).is_some()); // the real one answers
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
        mark(home, Class::Io, 9 * minute); // the clock stepped back a minute
        assert_eq!(since(home), Some((Class::Io, 9 * minute)));
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
        mark(&home, Class::Io, 10);
        assert_eq!(since(&home), None);
        mark(&home, Class::Io, 30);
        assert_eq!(since(&home), Some((Class::Io, 30)));
    }

    #[test]
    fn the_line_names_the_time_in_utc_and_the_class() {
        // 2026-09-27 04:05 UTC.
        let line = line((Class::DiskFull, 1_790_481_900_000_000_000));
        assert!(
            line.contains("recording has failed since 2026-09-27 04:05 UTC"),
            "{line}"
        );
        assert!(line.contains("disk full"), "{line}");
    }
}
