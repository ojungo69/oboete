//! G (#1037): every repository's counts and newest times, kept in knowledge.db by triggers, so the
//! repository list (`view::repos_in`) and its names (`view::repo_names_in`) read a row for each
//! repository and leg, not the tables they count. Counting those read 118.6 MB in 0.8 to 2.9 s
//! on a copy of this PC's knowledge.db (2026-11-17); the kept counts read 0.1 MB in 1.6 ms.
//!
//! `repo_counts` holds three legs for each repository, each counted as `view::REPOS` counts it:
//! 'c' its claims (its derivations, each uid once, the newest `valid_from`), 'i' its imported
//! documents (each uid once, the newest `ts`) and 'r' its records (its `raw_docs` rows, the newest
//! `ts`). Triggers on the three tables change it in the transaction of each row written, so it is
//! right whatever writes: `raw_docs` by `Fts::step` (inserts), `reindex` (Fts and FtsText) and
//! `Fts::rewind` (deletes); `derivations` by `Claims::step` (inserts), `Claims::rewind` and
//! `Anchors` (deletes); `imported` by `Imported::step` (inserts), `Imported::rewind` and
//! `forget::history::remove` (deletes). None of them updates a column counted (`Claims` sets a
//! derivation's status, which is not one), and such an update aborts (the `_kept` triggers). An
//! `INSERT OR REPLACE` would delete the row it replaces with no trigger fired (recursive triggers
//! are off), which a test keeps out.
//!
//! The Fts consumer makes them (`keep`), never a reader: readers make the stores' schema too. A
//! knowledge.db made new (a rebuild, a restore, a retention pass, a forget of a claim) holds none
//! until its first pass, a store of an older build holds none, and a newer build's may differ. On
//! each the readers count the tables as before (`kept`), and the next step makes this build's again.

use anyhow::Result;
use rusqlite::Connection;

/// A row for each repository and leg (`c`, `i`, `r`): how many it counts, and the newest time. The
/// SQL here and in `TRIGGERS` is as sqlite_master keeps it, which `kept` compares.
const TABLE: &str = "CREATE TABLE repo_counts (
  repo TEXT NOT NULL, leg TEXT NOT NULL, n INTEGER NOT NULL, last INTEGER NOT NULL,
  PRIMARY KEY (repo, leg)
) WITHOUT ROWID";

/// Each leg's two triggers, then the guards. A uid is looked up as `uid = .. AND +repo = ..`: the
/// `+` keeps the lookup on the uid's index, where `repo = ..` read imported's `imported_time` and
/// walked the repository's documents for each one imported (62,709 in this PC's largest). A newest
/// time is one seek of the repository's index (`raw_docs_repo`, `derivations_repo`,
/// `imported_time`).
const TRIGGERS: [&str; 9] = [
    "CREATE TRIGGER repo_counts_r_add AFTER INSERT ON raw_docs WHEN NEW.repo IS NOT NULL BEGIN
  INSERT INTO repo_counts(repo, leg, n, last) VALUES (NEW.repo, 'r', 1, NEW.ts)
    ON CONFLICT (repo, leg) DO UPDATE SET n = n + 1, last = MAX(last, excluded.last);
END",
    "CREATE TRIGGER repo_counts_r_drop AFTER DELETE ON raw_docs WHEN OLD.repo IS NOT NULL BEGIN
  UPDATE repo_counts SET n = n - 1,
      last = COALESCE((SELECT MAX(ts) FROM raw_docs WHERE repo = OLD.repo), last)
    WHERE repo = OLD.repo AND leg = 'r';
  DELETE FROM repo_counts WHERE repo = OLD.repo AND leg = 'r' AND n <= 0;
END",
    "CREATE TRIGGER repo_counts_c_add AFTER INSERT ON derivations WHEN NEW.repo IS NOT NULL BEGIN
  INSERT INTO repo_counts(repo, leg, n, last) VALUES (NEW.repo, 'c',
      NOT EXISTS (SELECT 1 FROM derivations WHERE uid = NEW.uid AND +repo = NEW.repo
                  AND rowid <> NEW.rowid), NEW.valid_from)
    ON CONFLICT (repo, leg) DO UPDATE SET n = n + excluded.n, last = MAX(last, excluded.last);
END",
    "CREATE TRIGGER repo_counts_c_drop AFTER DELETE ON derivations WHEN OLD.repo IS NOT NULL BEGIN
  UPDATE repo_counts SET
      n = n - NOT EXISTS (SELECT 1 FROM derivations WHERE uid = OLD.uid AND +repo = OLD.repo),
      last = COALESCE((SELECT MAX(valid_from) FROM derivations WHERE repo = OLD.repo), last)
    WHERE repo = OLD.repo AND leg = 'c';
  DELETE FROM repo_counts WHERE repo = OLD.repo AND leg = 'c' AND n <= 0;
END",
    "CREATE TRIGGER repo_counts_i_add AFTER INSERT ON imported BEGIN
  INSERT INTO repo_counts(repo, leg, n, last) VALUES (NEW.repo, 'i',
      NOT EXISTS (SELECT 1 FROM imported WHERE uid = NEW.uid AND +repo = NEW.repo
                  AND rowid <> NEW.rowid), NEW.ts)
    ON CONFLICT (repo, leg) DO UPDATE SET n = n + excluded.n, last = MAX(last, excluded.last);
END",
    "CREATE TRIGGER repo_counts_i_drop AFTER DELETE ON imported BEGIN
  UPDATE repo_counts SET
      n = n - NOT EXISTS (SELECT 1 FROM imported WHERE uid = OLD.uid AND +repo = OLD.repo),
      last = COALESCE((SELECT MAX(ts) FROM imported WHERE repo = OLD.repo), last)
    WHERE repo = OLD.repo AND leg = 'i';
  DELETE FROM repo_counts WHERE repo = OLD.repo AND leg = 'i' AND n <= 0;
END",
    "CREATE TRIGGER repo_counts_r_kept BEFORE UPDATE OF repo, ts ON raw_docs BEGIN
  SELECT RAISE(ABORT, 'repo_counts counts raw_docs by repo and ts: delete the row, then insert it');
END",
    "CREATE TRIGGER repo_counts_c_kept BEFORE UPDATE OF repo, uid, valid_from ON derivations BEGIN
  SELECT RAISE(ABORT, 'repo_counts counts derivations by repo, uid and valid_from: delete the row, then insert it');
END",
    "CREATE TRIGGER repo_counts_i_kept BEFORE UPDATE OF repo, uid, ts ON imported BEGIN
  SELECT RAISE(ABORT, 'repo_counts counts imported by repo, uid and ts: delete the row, then insert it');
END",
];

/// The tables counted as `view::REPOS` counts them, once, with the triggers.
const BACKFILL: &str = "INSERT INTO repo_counts
  SELECT repo, 'r', COUNT(*), MAX(ts) FROM raw_docs WHERE repo IS NOT NULL GROUP BY repo;
INSERT INTO repo_counts
  SELECT repo, 'c', COUNT(DISTINCT uid), MAX(valid_from) FROM derivations
  WHERE repo IS NOT NULL GROUP BY repo;
INSERT INTO repo_counts
  SELECT repo, 'i', COUNT(DISTINCT uid), MAX(ts) FROM imported GROUP BY repo";

/// Whether `k` holds this build's table and triggers and no other: then `repo_counts` holds what
/// the tables count (they are made together, `keep`, and kept by the triggers since).
pub(crate) fn kept(k: &Connection) -> Result<bool> {
    let mut st = k.prepare_cached(
        "SELECT name, sql FROM sqlite_master
         WHERE (type = 'table' AND name = 'repo_counts')
            OR (type = 'trigger' AND name GLOB 'repo_counts_*')",
    )?;
    let found = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
    Ok(found.len() == 1 + TRIGGERS.len()
        && std::iter::once(TABLE).chain(TRIGGERS).all(|sql| {
            found
                .get(name(sql))
                .is_some_and(|s| s.as_deref() == Some(sql))
        }))
}

/// An object's name: the third word of its SQL.
fn name(sql: &str) -> &str {
    sql.split_whitespace().nth(2).unwrap_or_default()
}

/// The Fts consumer's step, after `raw_docs_repo`: this build's table and triggers, counted from
/// the tables, unless `k` holds them (`kept`), in the step's transaction.
pub(crate) fn keep(k: &Connection) -> Result<()> {
    if kept(k)? {
        return Ok(());
    }
    let tx = if k.is_autocommit() {
        Some(k.unchecked_transaction()?)
    } else {
        None
    };
    // The tables the triggers are on, and the index imported's newest time is read from
    // (`imported_time`): the Fts consumer runs before Claims and Imported make them.
    crate::claims::schema(k)?;
    crate::consumer::imported::schema(k)?;
    crate::consumer::imported::cover(k)?;
    // Every build's triggers go: an older or a newer build's left beside these would count twice.
    let theirs: Vec<String> = k
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND name GLOB 'repo_counts_*'",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for name in theirs {
        k.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    k.execute_batch("DROP TABLE IF EXISTS repo_counts")?;
    k.execute_batch(TABLE)?;
    k.execute_batch(BACKFILL)?;
    for sql in TRIGGERS {
        k.execute_batch(sql)?;
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
