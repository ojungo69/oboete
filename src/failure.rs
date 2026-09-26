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
    if f.lock().is_err() || f.read_to_end(&mut buf).is_err() || buf.len() == SIZE {
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
    let new = State::Failed {
        class,
        first: at,
        last: at,
    };
    let written = transition(home, |s| match s {
        Some(State::Ok(ok)) if ok > at => None,
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
    if written.is_err() {
        // Never prepared (the first write failed): creating it may fail on a full disk too.
        let _ = std::fs::create_dir_all(home.join("state"));
        let _ = std::fs::write(marker(home), new.text());
    }
}

pub fn since(home: &Path) -> Option<(Class, i64)> {
    match State::parse(&std::fs::read_to_string(marker(home)).ok()?)? {
        State::Failed { class, first, .. } => Some((class, first)),
        State::Ok(_) => None,
    }
}

/// After a write that succeeded when it ended at `at`, in place. Nothing when a write that ended
/// after `at` has failed.
pub fn clear(home: &Path, at: i64) {
    let _ = transition(home, |s| match s {
        // A tie keeps the failure: when the order is unknown, report rather than hide.
        Some(State::Failed { last, .. }) if last >= at => None,
        Some(State::Ok(ok)) if ok >= at => None,
        _ => Some(State::Ok(at)),
    });
}

/// The marker read, changed by `change` (`None` leaves it) and written back in place, under the
/// file's lock, so two hooks cannot interleave their read and write.
fn transition(
    home: &Path,
    change: impl FnOnce(Option<State>) -> Option<State>,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(marker(home))?;
    f.lock()?;
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
        utc(ts / 1_000_000)
    )
}

fn marker(home: &Path) -> PathBuf {
    home.join("state").join("recording-failed")
}

fn padded(text: &str) -> String {
    format!("{text:<width$}\n", width = SIZE - 1)
}

/// `YYYY-MM-DD HH:MM UTC` for a Unix time in ms (days to a civil date: H. Hinnant's algorithm).
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// Free bytes where `home` lives, from `df -Pk` (Linux, macOS); `None` where that is unavailable.
/// ponytail: `df` instead of statvfs (no unsafe, no new dependency); Windows gets none, add
/// GetDiskFreeSpaceExW when doctor runs there.
pub fn free_bytes(home: &Path) -> Option<u64> {
    df("df".as_ref(), home)
}

/// How long `df` gets. On a stalled network or FUSE mount it can hang, and a hook calls it after
/// a failed write: it must never block the agent (MUST-M16).
const DF_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

fn df(program: &std::ffi::OsStr, home: &Path) -> Option<u64> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .arg("-Pk")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + DF_TIMEOUT;
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() >= deadline {
            // Not waited for: a `df` stuck in the kernel may not die at once.
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kb: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
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
