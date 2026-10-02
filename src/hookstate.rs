//! Per-session hook state (milestone 2 Task 2b): the few flags a hook keeps between calls of one
//! session, such as "context was injected" or "the session was compacted". Neither store fits:
//! raw.db has no session key (spec 1.6), and knowledge.db is the worker's, which SessionStart only
//! reads. So each flag is a file under `<home>/state/hooks/<agent>/<session hash>/`, as
//! `failure.rs` keeps its marker. `File::create_new` makes a claim atomic between concurrent
//! hooks (parallel tool calls). Losing the files costs one extra injection or one duplicate
//! prompt, never a record, so they are neither backed up nor synced.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A session's flags live this long after its last change (agy and OpenCode send no SessionEnd).
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

fn root(home: &Path) -> PathBuf {
    home.join("state").join("hooks")
}

fn dir(home: &Path, agent: &str, session: &str) -> PathBuf {
    let hash: String = Sha256::digest(session.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    root(home).join(agent).join(hash)
}

/// Sets `flag` and says whether this call set it first. A flag that cannot be written counts as
/// set by this call: a second injection is the cheaper failure.
pub fn claim(home: &Path, agent: &str, session: &str, flag: &str) -> bool {
    let dir = dir(home, agent, session);
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::File::create_new(dir.join(flag)) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::AlreadyExists,
    }
}

/// Sets `flag`, whether or not it was set.
pub fn set(home: &Path, agent: &str, session: &str, flag: &str) -> std::io::Result<()> {
    let dir = dir(home, agent, session);
    std::fs::create_dir_all(&dir)?;
    match std::fs::File::create_new(dir.join(flag)) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// Clears `flag` and says whether this call cleared it: of concurrent calls, one does. The
/// removal alone does not say so: on macOS and Windows concurrent removals of one file can all
/// succeed (all 4 threads in 1,479 of 2,000 rounds on the owner's iMac, and a rename is no better
/// on Windows), so takes of a session run one at a time under its lock. A lock that cannot be had
/// leaves the removal to decide: a second injection is the cheaper failure.
pub fn take(home: &Path, agent: &str, session: &str, flag: &str) -> bool {
    let dir = dir(home, agent, session);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"));
    if let Ok(lock) = &lock {
        let _ = lock.lock();
    }
    std::fs::remove_file(dir.join(flag)).is_ok()
}

/// Replaces the session's value `name` with what `f` makes of it (`None` removes it), under the
/// session's lock, so concurrent hooks of one session (parallel tool calls) lose no change. A value
/// that cannot be read is none to `f`.
// ponytail: each value is one small file rewritten whole per change; a session's shown set holds
// at most the claims shown to it, and goes with the session after `KEEP`.
pub fn update(
    home: &Path,
    agent: &str,
    session: &str,
    name: &str,
    f: impl FnOnce(Option<String>) -> Option<String>,
) -> std::io::Result<()> {
    let dir = dir(home, agent, session);
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))?;
    lock.lock()?;
    let path = dir.join(name);
    match f(std::fs::read_to_string(&path).ok()) {
        Some(v) => {
            // Renamed into place, so a reader without the lock sees the old value or the new.
            let part = dir.join(format!(".{name}.part"));
            std::fs::write(&part, v)?;
            std::fs::rename(&part, &path)
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// The session's value `name`: none when it has none or it cannot be read.
// The prompt point (Task 8 Step 6) reads it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn value(home: &Path, agent: &str, session: &str, name: &str) -> Option<String> {
    std::fs::read_to_string(dir(home, agent, session).join(name)).ok()
}

/// Removes the flags of sessions unchanged for `keep` (the worker, at its idle exit).
pub fn prune(home: &Path, keep: Duration) {
    let Ok(agents) = std::fs::read_dir(root(home)) else {
        return;
    };
    let now = SystemTime::now();
    for agent in agents.flatten() {
        let Ok(sessions) = std::fs::read_dir(agent.path()) else {
            continue;
        };
        for s in sessions.flatten() {
            let changed = s.metadata().and_then(|m| m.modified()).unwrap_or(now);
            if now.duration_since(changed).unwrap_or_default() > keep {
                let _ = std::fs::remove_dir_all(s.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_is_won_once_across_threads_and_a_take_clears_it_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let won: usize = (0..8)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || claim(&p, "grok", "s/1", "injected"))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap() as usize)
            .sum();
        assert_eq!(won, 1);
        assert!(!claim(&p, "grok", "s/1", "injected"));
        assert!(claim(&p, "grok", "s/2", "injected")); // another session
        set(&p, "cursor", "s/1", "compacted").unwrap();
        set(&p, "cursor", "s/1", "compacted").unwrap(); // already set
        assert!(take(&p, "cursor", "s/1", "compacted"));
        assert!(!take(&p, "cursor", "s/1", "compacted"));
    }

    #[test]
    fn a_flag_set_once_is_taken_by_one_of_concurrent_takes() {
        let home = tempfile::tempdir().unwrap();
        for _ in 0..200 {
            set(home.path(), "cursor", "s", "compacted").unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(4));
            let won: usize = (0..4)
                .map(|_| {
                    let (p, start) = (home.path().to_path_buf(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        take(&p, "cursor", "s", "compacted")
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|t| t.join().unwrap() as usize)
                .sum();
            assert_eq!(won, 1);
        }
    }

    /// Task 8 Step 5: concurrent updates of one session's value each see the others' changes.
    #[test]
    fn concurrent_updates_keep_every_change() {
        let home = tempfile::tempdir().unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let (p, start) = (home.path().to_path_buf(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    update(&p, "claude", "s", "shown", |v| {
                        Some(v.unwrap_or_default() + &format!("{i}\n"))
                    })
                    .unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let mut seen: Vec<String> = value(home.path(), "claude", "s", "shown")
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        seen.sort();
        assert_eq!(seen, (0..8).map(|i| i.to_string()).collect::<Vec<_>>());
        update(home.path(), "claude", "s", "shown", |_| None).unwrap();
        assert_eq!(value(home.path(), "claude", "s", "shown"), None);
    }

    #[cfg(unix)] // setting a directory's time needs another open on Windows
    #[test]
    fn prune_removes_only_sessions_older_than_the_limit() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        set(p, "agy", "old", "injected").unwrap();
        set(p, "agy", "new", "injected").unwrap();
        let old = dir(p, "agy", "old");
        let past = SystemTime::now() - KEEP - Duration::from_secs(60);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        prune(p, KEEP);
        assert!(!old.exists());
        assert!(dir(p, "agy", "new").exists());
    }
}
