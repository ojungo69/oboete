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
/// U3: the files one hook call writes back, the oldest first.
const PER_CALL: usize = 16;
const VERSION: u64 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    v: u64,
    ruleset: String,
    events: Vec<Captured>,
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
    let held: u64 = files(&dir)?
        .iter()
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum();
    anyhow::ensure!(held < BOUND, "{DIR}/ holds {held} bytes already");
    let body = serde_json::to_vec(&Kept {
        v: VERSION,
        ruleset: ruleset.to_owned(),
        events: events.to_vec(),
    })?;
    let name = format!("{}-{}", crate::db::now_ms(), std::process::id());
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
    Ok(())
}

/// U3, U4: the oldest kept files, `PER_CALL` at most, each appended in one transaction (an event
/// that names no session named for this device) and removed after its commit. One that cannot be
/// read, or that raw refuses, goes to `bad/`; a busy or failing store stops it, the files kept.
/// How many were written back.
pub fn write_back(home: &Path, raw: &mut crate::raw::Raw) -> Result<usize> {
    let dir = home.join(DIR);
    if !dir.is_dir() {
        return Ok(0);
    }
    // One call at a time: two calls at once would append a file twice. The other's own events
    // go first then, the kept ones at its next call.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("write-back.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(0),
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let mut written = 0;
    for path in files(&dir)?.into_iter().take(PER_CALL) {
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Kept>(&b).ok())
            .filter(|k| k.v == VERSION);
        let Some(mut kept) = kept else {
            set_aside(&dir, &path)?;
            continue;
        };
        for c in &mut kept.events {
            c.event.session = crate::hook::own_session(std::mem::take(&mut c.event.session), raw);
        }
        match raw.append_kept(&kept.events, &kept.ruleset) {
            Ok(()) => {
                // shortcut: a crash before this removal writes the file again later, a duplicate
                // and never a loss; a seen-files table in raw.db would make it exact.
                std::fs::remove_file(&path)?;
                written += 1;
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
    Ok(written)
}

/// U6: the hook calls kept, and the files set aside in `bad/`.
pub fn counts(home: &Path) -> (usize, usize) {
    let dir = home.join(DIR);
    let n = |d: &Path| files(d).map_or(0, |f| f.len());
    (n(&dir), n(&dir.join("bad")))
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

fn set_aside(dir: &Path, path: &Path) -> Result<()> {
    let bad = dir.join("bad");
    std::fs::create_dir_all(&bad)?;
    if let Some(name) = path.file_name() {
        std::fs::rename(path, bad.join(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt(text: &str, ts: i64) -> Captured {
        let payload = json!({"session_id": "s", "cwd": "/w", "prompt": text});
        let settings = crate::capture::Settings::default();
        crate::capture::events("claude", "UserPromptSubmit", &payload, ts, &settings)
            .pop()
            .unwrap()
    }

    fn events(raw: &crate::raw::Raw) -> Vec<crate::raw::Event> {
        raw.after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                crate::raw::Item::Event(e) => Some(*e),
                _ => None,
            })
            .collect()
    }

    /// U1, U3: a kept file holds the masked text, never the secret, and is the owner's alone; it
    /// is written back in one go, before what comes after, and is gone then.
    #[test]
    fn kept_events_are_masked_and_written_back_in_order() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let secret = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let ruleset = crate::capture::Settings::default()
            .rules
            .version()
            .to_owned();
        // One built without a store names no session yet (U2).
        let mut nameless = prompt("second", 1_001);
        nameless.event.session = "unknown".into();
        let kept = [prompt(&format!("use {secret} here"), 1_000), nameless];
        keep(h, &kept, &ruleset).unwrap();
        let file = files(&h.join(DIR)).unwrap().pop().unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains(&secret));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        let mut raw = crate::raw::open(h).unwrap();
        assert_eq!(write_back(h, &mut raw).unwrap(), 1);
        assert_eq!(counts(h), (0, 0));
        let later = prompt("third", 2_000);
        raw.append_with_ledger(&later.event, &later.ledger, &ruleset)
            .unwrap();
        let events = events(&raw);
        assert_eq!(events.len(), 3);
        assert!(!events[0].body.contains(&secret) && events[0].body.contains("use "));
        assert!(events[1].body.contains("second") && events[2].body.contains("third"));
        assert_eq!(events[1].session, format!("unknown-{}", raw.device()));
    }

    /// U3: one write-back at a time: while another call holds it, a call writes nothing back, and
    /// the file waits for the next.
    #[test]
    fn one_write_back_at_a_time() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        keep(h, &[prompt("kept", 1)], "r").unwrap();
        let mut raw = crate::raw::open(h).unwrap();
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(h.join(DIR).join("write-back.lock"))
            .unwrap();
        held.lock().unwrap();
        assert_eq!(write_back(h, &mut raw).unwrap(), 0);
        assert_eq!(counts(h), (1, 0));
        drop(held);
        assert_eq!(write_back(h, &mut raw).unwrap(), 1);
        assert_eq!(counts(h), (0, 0));
    }

    /// U4: a file that cannot be read back goes to `bad/` and stays; U5: nothing is kept past the
    /// bound.
    #[test]
    fn a_bad_file_goes_aside_and_nothing_is_kept_past_the_bound() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let dir = h.join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1-1.json"), "not json").unwrap();
        let mut raw = crate::raw::open(h).unwrap();
        assert_eq!(write_back(h, &mut raw).unwrap(), 0);
        assert_eq!(counts(h), (0, 1));
        assert!(events(&raw).is_empty());
        // A store that fails the write keeps the file for a later call: it is no bad file.
        keep(h, &[prompt("kept", 5)], "r").unwrap();
        let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON records BEGIN SELECT RAISE(ABORT, 'test'); END;",
        )
        .unwrap();
        assert!(write_back(h, &mut raw).is_err());
        assert_eq!(counts(h), (1, 1));
        conn.execute_batch("DROP TRIGGER refuse;").unwrap();
        assert_eq!(write_back(h, &mut raw).unwrap(), 1);
        std::fs::File::create(dir.join("2-1.json"))
            .unwrap()
            .set_len(BOUND)
            .unwrap();
        let e = keep(h, &[prompt("one", 1)], "r").unwrap_err();
        assert!(e.to_string().contains("holds"), "{e}");
        assert_eq!(counts(h), (1, 1));
    }
}
