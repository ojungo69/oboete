//! Milestone 4 Task 9: the cut-over from v1's `oboete.db` to Design B's stores (spec 7.4, D6).

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// v1's store, read only (spec 7.4: the old store is left byte for byte), with the stores' busy
/// wait. Never `immutable`, which skips the WAL, where the newest events are, and assumes no one
/// writes: v1's hooks may write meanwhile.
pub fn open_v1(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(path: &Path) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
    }

    /// A v1 store with its newest event only in the WAL: the hashes of the store and its WAL,
    /// and its device id.
    fn v1_store(home: &Path) -> (String, String, String) {
        let conn = crate::db::open(home).unwrap();
        conn.execute(
            "INSERT INTO events(session_id, event, ts, payload) VALUES ('s', 'Stop', 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint = 0")
            .unwrap();
        conn.execute(
            "INSERT INTO events(session_id, event, ts, payload) VALUES ('s', 'Stop', 2, '{}')",
            [],
        )
        .unwrap();
        let device = crate::db::device_id(&conn).unwrap();
        // Left open: closing the last connection would move the WAL into the store.
        std::mem::forget(conn);
        let wal = home.join("oboete.db-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);
        (sha(&home.join("oboete.db")), sha(&wal), device)
    }

    /// Spec 7.4: doctor reads v1's store without a write, its WAL's events counted, also on a
    /// copy, where v1's open would have given the store a new device id.
    #[test]
    fn the_old_store_is_left_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        let before = v1_store(home.path());
        let copy = tempfile::tempdir().unwrap();
        for f in ["oboete.db", "oboete.db-wal"] {
            std::fs::copy(home.path().join(f), copy.path().join(f)).unwrap();
        }
        for h in [home.path(), copy.path()] {
            crate::setup::doctor(h).ok();
            let conn = open_v1(&h.join("oboete.db")).unwrap();
            let events: i64 = conn
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(events, 2);
            let device = crate::db::device_id(&conn).unwrap();
            drop(conn);
            let after = (
                sha(&h.join("oboete.db")),
                sha(&h.join("oboete.db-wal")),
                device,
            );
            assert_eq!(after, before, "{}", h.display());
        }
    }
}
