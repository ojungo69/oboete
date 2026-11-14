//! What the stores share: opening SQLite files with the stores' busy waits and schema, a store
//! file's identity and the device id it is given, a file's privacy, and the clock.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

/// `PRAGMA quick_check`: Ok when SQLite answers "ok", else the first problem it names.
pub(crate) fn quick_check(conn: &Connection, name: &str) -> Result<()> {
    let first: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    anyhow::ensure!(first == "ok", "{name}: quick_check: {first}");
    Ok(())
}

/// The whole `quick_check` of the file at `path` but for the virtual tables' own checks, on a
/// connection of its own: FTS5 answers any quick_check with its whole-index check (it ignores
/// isQuick), about 10 s at every worker start on a 70,000-record home, where this takes half a
/// second. The connection's modules are dropped before it reads the schema, and SQLite leaves out
/// a virtual table whose module it does not have; every b-tree (the shadow tables' too), the
/// freelist and each page's one owner are still checked. doctor runs the whole check.
pub(crate) fn quick_check_without_vtabs(path: &Path, name: &str) -> Result<()> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", path.display()))?;
    // This read-only check waits 2 s, matching the stores' SQLite busy timeout. Initialization
    // retries share 10 s on the normal raw path, or 2 s on hooks; rusqlite's default is 5 s.
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    // SAFETY: the handle is this live connection's, and a null list keeps no module, FTS5's
    // included, which is what leaves the search indexes out. It must come before any statement
    // uses a virtual table: one already connected keeps its module, and the check would run it.
    let rc = unsafe { rusqlite::ffi::sqlite3_drop_modules(conn.handle(), std::ptr::null_mut()) };
    anyhow::ensure!(rc == rusqlite::ffi::SQLITE_OK, "{name}: drop modules: {rc}");
    quick_check(&conn, name)
}

pub(crate) const OPEN_WRITE_WAIT: Duration = Duration::from_secs(10);

/// WAL with a 2 s SQLite busy timeout and the given `synchronous` level. This path retries up
/// to 10 s; raw hooks call `wal_until` with their shared 2 s initialization deadline instead.
#[cfg(test)]
pub(crate) fn wal(conn: &Connection, synchronous: &str) -> Result<()> {
    wal_until(conn, synchronous, Instant::now() + OPEN_WRITE_WAIT)
}

pub(crate) fn wal_until(conn: &Connection, synchronous: &str, deadline: Instant) -> Result<()> {
    // Normal operations keep the 2 s timeout; initialization retries clamp it to the deadline.
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    // Switching a file to WAL takes an exclusive lock that the busy handler does not cover:
    // openers racing on a fresh or pre-WAL file wait for each other here instead.
    retry_busy(conn, deadline, || {
        conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous={synchronous};"
        ))?;
        Ok(())
    })
    .context("journal mode")
}

#[cfg(test)]
thread_local! {
    // A test opener observes its first real retry, without replacing SQLite's busy handler.
    pub(crate) static BUSY_RETRY_NOTICE: std::cell::Cell<Option<std::sync::mpsc::Sender<()>>> =
        const { std::cell::Cell::new(None) };
}

/// Retry an open-time write until the shared deadline, including SQLite's busy-handler waits.
/// The connection keeps its busy timeout afterwards; non-lock errors return immediately.
pub(crate) fn retry_busy<T>(
    conn: &Connection,
    deadline: Instant,
    mut run: impl FnMut() -> Result<T>,
) -> Result<T> {
    let timeout: u32 = conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
    let timeout = Duration::from_millis(timeout.into());
    let result = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        conn.busy_timeout(timeout.min(remaining))?;
        match run() {
            Err(e)
                if e.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
                    matches!(
                        e.sqlite_error_code(),
                        Some(
                            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                        )
                    )
                }) && Instant::now() < deadline =>
            {
                #[cfg(test)]
                BUSY_RETRY_NOTICE.with(|notice| {
                    if let Some(retried) = notice.take() {
                        let _ = retried.send(());
                    }
                });
                std::thread::sleep(
                    Duration::from_millis(20)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => break result,
        }
    };
    conn.busy_timeout(timeout)?;
    result
}

/// Create missing schema objects under one write lock and the caller's initialization deadline.
/// A complete schema stays read-only; autocommit CREATEs would each restart the busy timeout.
pub(crate) fn ensure_schema_until(
    conn: &Connection,
    schema: &str,
    deadline: Instant,
) -> Result<bool> {
    if schema_present(conn, schema)? {
        return Ok(false);
    }
    retry_busy(conn, deadline, || {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let before: i64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        let changes = tx.total_changes();
        tx.execute_batch(schema)?;
        let changed = tx.total_changes() != changes
            || tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))? != before;
        tx.commit()?;
        Ok(changed)
    })
}

/// The stores use single-line CREATE headers and unquoted names, including VIRTUAL TABLE.
/// Derive the required objects so a new table or index cannot leave this read check behind.
fn schema_present(conn: &Connection, schema: &str) -> Result<bool> {
    let mut exists = conn.prepare(
        "SELECT 1 FROM sqlite_master WHERE type = lower(?1) AND name = ?2 COLLATE NOCASE",
    )?;
    for create in schema
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("CREATE "))
    {
        let (kind, definition) = create
            .split_once(" IF NOT EXISTS ")
            .context("schema CREATE header")?;
        let kind = kind
            .split_ascii_whitespace()
            .last()
            .context("schema object type")?;
        let name = definition
            .trim_start()
            .split(|c: char| c.is_ascii_whitespace() || c == '(')
            .next()
            .context("schema object name")?;
        if !exists.exists(params![kind, name])? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The store file's identity: a copy on another machine (or a restored backup) is another file.
/// ponytail: file identity instead of host name + OS machine id (proposal §4.2 3), which needs no
/// new dependency; a restore on the same machine also gets a new id, which costs nothing.
#[cfg(unix)]
pub(crate) fn store_file(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or_else(|_| String::new(), |m| format!("{}:{}", m.dev(), m.ino()))
}

/// The volume and NTFS file index, which a rename keeps. Not the creation time: NTFS gives a file
/// renamed into a name freed less than 15 s before that name's old creation time ("tunneling"),
/// so a restore's swap made the restored raw.db look like another file and changed the device.
#[cfg(windows)]
pub(crate) fn store_file(path: &Path) -> String {
    std::fs::File::open(path)
        .and_then(|file| store_file_from(&file))
        .unwrap_or_default()
}

/// Identity from the admitted descriptor, without reopening a source path.
pub(crate) fn store_file_from(file: &std::fs::File) -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(file)?;
        Ok(format!(
            "{}:{}",
            info.volume_serial_number(),
            info.file_index()
        ))
    }
}

/// The identity a Windows store was given before #122: its creation time.
#[cfg(windows)]
pub(crate) fn legacy_store_file(path: &Path) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.creation_time().to_string())
}

#[cfg(unix)]
fn legacy_store_file(_: &Path) -> Option<String> {
    None
}

/// This device's id, 8 hex digits, chosen when the store is created and again when the store
/// turns up as another file (a `~/.oboete` copied to another machine), so two devices never share
/// one. It prefixes ids that must be unique across devices.
pub(crate) fn ensure_device(conn: &Connection, path: &Path) -> Result<()> {
    ensure_device_until(conn, path, Instant::now() + OPEN_WRITE_WAIT).map(|_| ())
}

pub(crate) fn ensure_device_until(
    conn: &Connection,
    path: &Path,
    deadline: Instant,
) -> Result<bool> {
    let here = store_file(path);
    let known: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='store_file'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if known.as_deref() == Some(here.as_str()) {
        return Ok(false);
    }
    // A store from before #122 on Windows holds its creation time: the same file, so it keeps its
    // device id and takes the new identity (its records stay under their device, Codex on #122).
    if known.is_some() && known == legacy_store_file(path) {
        return retry_busy(conn, deadline, || {
            let changed = conn.execute(
                "UPDATE meta SET value=?1 WHERE key='store_file'",
                params![here],
            )?;
            Ok(changed != 0)
        });
    }
    let mut raw = [0u8; 4];
    getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("random device id: {e}"))?;
    let id: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    // Two first opens may race: the one whose update finds the old identity still in place wins,
    // the other keeps what it reads back.
    retry_busy(conn, deadline, || {
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "INSERT INTO meta(key, value) VALUES('store_file', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value",
            params![here],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('device_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(changed != 0)
    })
}

pub fn device_id(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT value FROM meta WHERE key='device_id'", [], |r| {
            r.get(0)
        })?,
    )
}

/// sqlite-vec, compiled in and registered for every connection this process opens (`vec_docs`
/// is a `vec0` table, which a connection without the module cannot read).
pub(crate) fn register_sqlite_vec() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        type Init = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        // SAFETY: sqlite3_vec_init is the extension's entry point with exactly this signature;
        // the crate declares it as `fn()` only to avoid depending on SQLite's types.
        unsafe {
            let init =
                std::mem::transmute::<*const (), Init>(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

/// The read check keeps the hook path free of write locks; the write transaction re-checks,
/// so hooks that open an old database at the same moment do not race on the ALTER.
#[cfg(test)]
pub(crate) fn ensure_column(
    conn: &mut Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<()> {
    ensure_column_until(conn, table, column, decl, Instant::now() + OPEN_WRITE_WAIT).map(|_| ())
}

pub(crate) fn ensure_column_until(
    conn: &mut Connection,
    table: &str,
    column: &str,
    decl: &str,
    deadline: Instant,
) -> Result<bool> {
    if has_column(conn, table, column)? {
        return Ok(false);
    }
    retry_busy(conn, deadline, || {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let changed = !has_column(&tx, table, column)?;
        if changed {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
        }
        tx.commit()?;
        Ok(changed)
    })
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .any(|c| c.as_deref() == Ok(column));
    Ok(found)
}

/// `ms` since the Unix epoch as `YYYY-MM-DD HH:MM UTC` (civil-from-days, no time crate).
pub fn utc(ms: i64) -> String {
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

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The store holds prompts and tool output from every project: owner-only whatever the umask
/// (the directory first, so the file SQLite creates is never reachable at 0644). Best effort: a
/// filesystem without Unix modes (a Windows drive under WSL) keeps its own rules.
pub(crate) fn private(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

#[cfg(test)]
mod tests;
