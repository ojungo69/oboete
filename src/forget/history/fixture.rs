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
