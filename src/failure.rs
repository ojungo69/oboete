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

/// After a successful write: the marker exists at its full size before any failure needs it.
pub fn prepare(home: &Path) {
    let path = marker(home);
    if !path.exists() {
        let _ = std::fs::create_dir_all(home.join("state"));
        let _ = std::fs::write(&path, padded("ok"));
    }
}

/// After a failed write: the class and the first failure's time, kept until a write succeeds.
pub fn mark(home: &Path, class: Class) {
    if since(home).is_some() {
        return;
    }
    // SQLite reports a full disk as an I/O error while it sets up its journal (SQLITE_IOERR 4874
    // on a full tmpfs): the free space tells them apart.
    let class = if class == Class::Io && free_bytes(home).is_some_and(|b| b < 1024 * 1024) {
        Class::DiskFull
    } else {
        class
    };
    let text = padded(&format!("failed {} {}", class.name(), crate::db::now_ms()));
    if rewrite(home, &text).is_err() {
        // Never prepared (the first write failed): creating it may fail on a full disk too.
        let _ = std::fs::create_dir_all(home.join("state"));
        let _ = std::fs::write(marker(home), text);
    }
}

pub fn since(home: &Path) -> Option<(Class, i64)> {
    let text = std::fs::read_to_string(marker(home)).ok()?;
    let mut words = text.split_whitespace();
    (words.next()? == "failed").then_some(())?;
    let class = Class::parse(words.next()?);
    Some((class, words.next()?.parse().ok()?))
}

/// After a successful write, in place.
pub fn clear(home: &Path) {
    if since(home).is_some() {
        let _ = rewrite(home, &padded("ok"));
    }
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
        utc(ts)
    )
}

fn marker(home: &Path) -> PathBuf {
    home.join("state").join("recording-failed")
}

fn padded(text: &str) -> String {
    format!("{text:<width$}\n", width = SIZE - 1)
}

/// Overwrite the existing marker from its first byte, without truncating or growing it.
fn rewrite(home: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).open(marker(home))?;
    f.write_all(text.as_bytes())
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
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(home)
        .output()
        .ok()?;
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
        mark(home, Class::DiskFull);
        let (class, first) = since(home).unwrap();
        assert_eq!(class, Class::DiskFull);
        assert!((crate::db::now_ms() - first).abs() < 60_000, "{first}");
        std::thread::sleep(std::time::Duration::from_millis(5));
        mark(home, Class::Io);
        assert_eq!(since(home), Some((Class::DiskFull, first)));
        assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
        clear(home);
        assert_eq!(since(home), None);
        assert_eq!(std::fs::metadata(marker(home)).unwrap().len(), 64);
        // A failure before any success still leaves a marker when the disk allows it.
        let fresh = tempfile::tempdir().unwrap();
        mark(fresh.path(), Class::Busy);
        assert_eq!(since(fresh.path()).map(|f| f.0), Some(Class::Busy));
    }

    #[test]
    fn the_line_names_the_time_in_utc_and_the_class() {
        // 2026-09-27 04:05 UTC.
        let line = line((Class::DiskFull, 1_790_481_900_000));
        assert!(
            line.contains("recording has failed since 2026-09-27 04:05 UTC"),
            "{line}"
        );
        assert!(line.contains("disk full"), "{line}");
    }
}
