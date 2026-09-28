//! M2's crash harness (spec 8.2): every store's commits are counted on this thread, and from a
//! chosen one on each is rolled back, as if the process had died just before it.

use rusqlite::Connection;
use std::cell::Cell;

thread_local! {
    static COMMITS: Cell<u64> = const { Cell::new(0) };
    static AT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Counts this connection's commits, and rolls them back once the crash point is reached.
pub fn arm(conn: &Connection) {
    conn.commit_hook(Some(|| {
        let n = COMMITS.get() + 1;
        COMMITS.set(n);
        AT.get().is_some_and(|at| n >= at)
    }))
    .unwrap();
}

/// From the `n`th commit on (counted from now), nothing lands.
pub fn at(n: u64) {
    COMMITS.set(0);
    AT.set(Some(n));
}

/// Commits land again, counted from zero.
pub fn off() {
    COMMITS.set(0);
    AT.set(None);
}

/// The commits since `at` or `off`, rolled back or not.
pub fn count() -> u64 {
    COMMITS.get()
}
