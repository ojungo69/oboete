//! Records a hook could not write (docs/unwritten.md, parity row 12): the events of a hook call
//! whose write failed, kept after the gate in a file of their own, and written back by the next
//! hook call whose store opens.

use crate::capture::Captured;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Under the home. Not `spool/`: v1 left a directory of that name, which `migrate --finish`
/// deletes (U1).
const DIR: &str = "unwritten";
/// U5: nothing more is kept once the files hold this much.
const BOUND: u64 = 64 << 20;
/// U3: the files one hook call visits, the oldest first.
const PER_CALL: usize = 16;
/// #443: serialized event bytes (ledger and labels included) one call writes back. Leave time
/// for the hook's own keep and marker within the shortest, 3 s, deadline after a 2 s store wait.
const BYTE_BUDGET: usize = 256 << 10;
/// U5: how long a keep waits for the one in progress: a SessionEnd hook has 3 s in all, and the
/// store's wait takes 2 of them (Codex on #440).
pub(crate) const KEEP_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const VERSION: u64 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    v: u64,
    ruleset: String,
    events: Vec<Captured>,
    /// #454: random to each keep, so the file's hash, which a write-back keeps, names this keep
    /// alone: two calls in one millisecond with one payload keep the same events. None in a file
    /// kept before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

/// U1, U5: `events`, as the gate left them under `ruleset`, in a file of their own, written to a
/// temporary name and renamed: whole or absent. An error when it cannot be, or past the bound.
pub fn keep(home: &Path, events: &[Captured], ruleset: &str) -> Result<()> {
    use std::io::Write;
    if events.is_empty() {
        return Ok(());
    }
    let dir = home.join(DIR);
    std::fs::create_dir_all(&dir)?;
    crate::db::private(&dir, 0o700);
    // One keep at a time from the bound's check to the rename: overlapping failed calls would each
    // pass it, and name their files in no order (Codex on #440).
    let _lock = keep_lock(&dir)?;
    // A temporary file found under the lock is a keep's that was killed before its rename: it
    // would hold its name, and every later keep would fail on it (Codex on #440).
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "tmp") {
            std::fs::remove_file(&path)?;
        }
    }
    let kept = files(&dir)?;
    let bad = files(&dir.join("bad"))?;
    // `bad/` counts too: oboete never deletes what it set aside there (Codex on #440).
    let held: u64 = kept
        .iter()
        .chain(&bad)
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum();
    let mut id = [0_u8; 16];
    getrandom::fill(&mut id)?;
    let body = serde_json::to_vec(&Kept {
        v: VERSION,
        ruleset: ruleset.to_owned(),
        events: events.to_vec(),
        id: Some(id.iter().map(|b| format!("{b:02x}")).collect()),
    })?;
    // The new file counts too: one call may hold many events (Codex on #440).
    anyhow::ensure!(
        held + body.len() as u64 <= BOUND,
        "{DIR}/ holds {held} bytes, and {} more would pass its bound",
        body.len()
    );
    // One past the newest file here or in `bad/`, chosen under the lock: the files sort as they
    // were kept, those of two processes in one millisecond too, and a file set aside never takes
    // the name of one set aside before (Codex on #440).
    let next = kept
        .iter()
        .chain(&bad)
        .filter_map(|p| p.file_stem()?.to_str()?.parse::<u64>().ok())
        .max()
        .map_or(0, |n| n + 1);
    let name = format!("{next:020}");
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp).and_then(|mut f| {
        f.write_all(&body)?;
        f.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, dir.join(format!("{name}.json")))) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    // The rename is the only copy's entry: synced with its directory, and the directory's own entry
    // with the home at every keep, since one killed after making the directory left it unsynced
    // (Codex and CodeRabbit on #440). The file is in place even when a sync fails.
    #[cfg(unix)]
    if let Err(e) = std::fs::File::open(&dir)
        .and_then(|d| d.sync_all())
        .and_then(|()| std::fs::File::open(home)?.sync_all())
    {
        eprintln!("oboete: {DIR}/ not synced after a keep: {e}");
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// The store error a write-back meets, as a damaged or full raw.db gives it, after that many
    /// files written back.
    pub(crate) static FAILS: std::cell::Cell<Option<(usize, std::os::raw::c_int)>> =
        const { std::cell::Cell::new(None) };
    /// A killed call after the part's durable commit, before any kept-file cleanup.
    static STOP_AFTER_PART: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// U3, U4: the oldest kept files, `PER_CALL` at most and `BYTE_BUDGET` bytes across their parts.
/// Each part and its consumed offset commit together; a completed file is removed afterwards.
/// An event that names no session is named for this device. One that cannot be
/// read, or that raw refuses, goes to `bad/`; a busy or failing store stops it, the files kept.
/// How many were written back, also beside the error that stopped it (Greptile and Codex on
/// #440).
pub fn write_back(
    home: &Path,
    raw: &mut crate::raw::Raw,
    settings: &crate::capture::Settings,
) -> (usize, Result<()>) {
    let mut written = 0;
    let back = write_back_counting(home, raw, &mut written, settings);
    (written, back)
}

fn write_back_counting(
    home: &Path,
    raw: &mut crate::raw::Raw,
    written: &mut usize,
    settings: &crate::capture::Settings,
) -> Result<()> {
    let dir = home.join(DIR);
    if !dir.is_dir() {
        return Ok(());
    }
    // One call at a time: two calls at once would append a file twice. The other keeps its own
    // events behind what remains (hook::record). Under `cargo test`, a lock a call just released
    // can still be held by a child another test thread forked (`worker::try_lock`).
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("write-back.lock"))?;
    match crate::worker::try_lock(&lock) {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let mut used = 0;
    for path in files(&dir)?.into_iter().take(PER_CALL) {
        if used >= BYTE_BUDGET {
            break;
        }
        let bytes = std::fs::read(&path).ok();
        let kept = bytes
            .as_deref()
            .and_then(|b| serde_json::from_slice::<Kept>(b).ok())
            .filter(|k| k.v == VERSION);
        let (Some(mut kept), Some(bytes)) = (kept, bytes) else {
            set_aside(&dir, &path)?;
            continue;
        };
        #[cfg(test)]
        if let Some((after, code)) = FAILS.get()
            && after == *written
        {
            FAILS.set(None);
            let e = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            return Err(e.into());
        }
        // #454: the file's hash (its keep's random id within) is its receipt and names its
        // offset. A file kept before the id has none, and two of them can hold the same bytes:
        // it is written whole, as before #443, with no receipt or offset, and written again if
        // left behind (#460).
        let hash = kept.id.is_some().then(|| crate::forget::hash(&bytes));
        let from = match &hash {
            Some(hash) => raw.kept_progress(hash, kept.events.len())?,
            None => 0,
        };
        anyhow::ensure!(
            from <= kept.events.len(),
            "kept offset is past the file's end"
        );
        if from == kept.events.len() {
            std::fs::remove_file(&path)?;
            continue;
        }
        let mut through = from;
        for c in &mut kept.events[from..] {
            c.event.session = crate::hook::own_session(std::mem::take(&mut c.event.session), raw);
            let size = serde_json::to_vec(c)?.len();
            // An event is indivisible. Admit the first even above the budget to avoid leaving
            // it first forever; the capture cap is per field, not per whole event.
            if hash.is_some() && used > 0 && size > BYTE_BUDGET.saturating_sub(used) {
                break;
            }
            used += size;
            through += 1;
            if hash.is_some() && used >= BYTE_BUDGET {
                break;
            }
        }
        if through == from {
            break;
        }
        let complete = through == kept.events.len();
        let progress = hash.as_deref().map(|hash| crate::raw::KeptProgress {
            hash,
            from,
            through,
            complete,
        });
        match raw.append_kept(
            &kept.events[from..through],
            &kept.ruleset,
            progress,
            settings,
        ) {
            Ok(appended) => {
                *written += usize::from(appended);
                #[cfg(test)]
                if STOP_AFTER_PART.replace(false) {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
                }
                if complete {
                    std::fs::remove_file(&path)?;
                } else {
                    // The oldest file still has events: later files and this call's own events
                    // must remain behind it, even if the next event did not fit the spare bytes.
                    break;
                }
            }
            // Not the store failing (busy, full, damaged): raw refused these events.
            Err(e)
                if !e
                    .chain()
                    .any(|c| c.is::<rusqlite::Error>() || c.is::<std::io::Error>()) =>
            {
                eprintln!("oboete: kept events refused: {e:#}");
                set_aside(&dir, &path)?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// U6: the hook calls kept, and the files set aside in `bad/`; an error when either cannot be
/// listed, which may hide some (Codex on #440).
pub fn counts(home: &Path) -> Result<(usize, usize)> {
    let dir = home.join(DIR);
    Ok((files(&dir)?.len(), files(&dir.join("bad"))?.len()))
}

/// The kept files in `dir`, the oldest first (their names start with the time).
fn files(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// U5: `unwritten/keep.lock`, held by a keep from its look at the files to its rename and by a move
/// to `bad/`, so the bound sees each file once (Codex on #440). Waited for `KEEP_WAIT` at most, so
/// the hook still keeps its events and sets its marker before the agent's deadline.
fn keep_lock(dir: &Path) -> Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("keep.lock"))?;
    let until = std::time::Instant::now() + KEEP_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("another keep held {DIR}/ for {} ms", KEEP_WAIT.as_millis())
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

fn set_aside(dir: &Path, path: &Path) -> Result<()> {
    let _lock = keep_lock(dir)?;
    let bad = dir.join("bad");
    std::fs::create_dir_all(&bad)?;
    if let Some(name) = path.file_name() {
        std::fs::rename(path, bad.join(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
