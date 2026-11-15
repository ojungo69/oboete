//! The two marks of a history forget as tests set them, until 5b's registration and 5c's apply
//! write them: each through a connection of its own to the home's store.

use std::path::Path;

/// A history forget as a registration leaves it in `home`'s raw.db.
pub(crate) fn register(home: &Path, job: &str, token: &str) {
    rusqlite::Connection::open(home.join("raw.db"))
        .unwrap()
        .execute(
            "INSERT INTO forget_history(job, token) VALUES (?1, ?2)",
            [job, token],
        )
        .unwrap();
}

/// `home`'s knowledge.db's mark that it removed a history forget's rows.
pub(crate) fn mark_applied(home: &Path, job: &str, token: &str) {
    crate::knowledge::open(home)
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO history_applied(job, token) VALUES (?1, ?2)",
            [job, token],
        )
        .unwrap();
}

/// The proof, as a test that registers no history forget may take it to call a reader directly.
pub(crate) fn proof() -> super::Applied {
    super::Applied(())
}

/// A history forget covering the imported documents (their files, notes) `uids` name, as a
/// registration leaves it in `home`'s raw.db: its job with `token`, and what it covers, in one
/// transaction (5b). A job registered again takes the new token.
pub(crate) fn register_covering(home: &Path, job: &str, token: &str, uids: &[&str]) {
    let mut raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    let tx = raw.transaction().unwrap();
    tx.execute(
        "INSERT OR REPLACE INTO forget_history(job, token) VALUES (?1, ?2)",
        [job, token],
    )
    .unwrap();
    for uid in uids {
        tx.execute(
            "INSERT OR IGNORE INTO denied_imports(kind, key, job) VALUES ('uid', ?1, ?2)",
            [uid, &job],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

/// `home`'s raw.db as one restored to before `job` leaves it: neither the job nor what it covers.
pub(crate) fn lose(home: &Path, job: &str) {
    let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    raw.execute("DELETE FROM forget_history WHERE job = ?1", [job])
        .unwrap();
    raw.execute("DELETE FROM denied_imports WHERE job = ?1", [job])
        .unwrap();
}
