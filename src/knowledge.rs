//! Design B's derived store (docs/milestone-2-plan.md D10): `knowledge.db`, rebuilt from `raw.db`
//! when lost, so synchronous=NORMAL. Each consumer's checkpoint is the highest seq of a device it
//! has finished, moved in the same transaction as its output.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub fn open(home: &Path) -> Result<Connection> {
    let path = home.join("knowledge.db");
    crate::db::private(home, 0o700);
    // Before the open: `vec_index` is a vec0 table, which a connection without the module cannot
    // read (milestone 4 D8).
    crate::db::register_sqlite_vec();
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(test)]
    crate::crash::arm(&conn);
    crate::db::wal(&conn, "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS checkpoints(
           consumer TEXT NOT NULL, device TEXT NOT NULL, seq INTEGER NOT NULL,
           PRIMARY KEY (consumer, device)
         );
         -- An op consumer's checkpoints (milestone 3 D4): an op_seq per origin device. Apart from
         -- the seqs above, which compression takes the lowest of to know what every reader passed.
         CREATE TABLE IF NOT EXISTS op_checkpoints(
           consumer TEXT NOT NULL, device TEXT NOT NULL, seq INTEGER NOT NULL,
           PRIMARY KEY (consumer, device)
         );
         -- MUST-M14's report: each checkpoint moved back because raw lost commits, for doctor.
         CREATE TABLE IF NOT EXISTS rewinds(
           ts INTEGER NOT NULL, consumer TEXT NOT NULL, device TEXT NOT NULL,
           was INTEGER NOT NULL, now INTEGER NOT NULL
         );",
    )
    .context("knowledge schema")?;
    // Milestone 4 D8: vectors by embedder and the SHA-256 of the stored text they were made from
    // (fp32, little-endian); each document's key (a claim uid, an imported uid, a record's
    // `device:seq`) to its vector, or why it has none; the searched index, whose rowid is the
    // key's id; and the embedders' generations.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS vectors(
           embedder TEXT NOT NULL, src_sha TEXT NOT NULL, vec BLOB NOT NULL,
           PRIMARY KEY (embedder, src_sha)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS vector_keys(
           id INTEGER PRIMARY KEY, embedder TEXT NOT NULL, kind TEXT NOT NULL, key TEXT NOT NULL,
           src_sha TEXT, skipped TEXT,
           UNIQUE (kind, key, embedder)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS vec_index USING vec0(
           embedder TEXT PARTITION KEY, kind TEXT PARTITION KEY,
           repo TEXT, ts INTEGER, session TEXT,
           embedding bit[{}]
         );
         CREATE TABLE IF NOT EXISTS vec_generation(
           embedder TEXT PRIMARY KEY, state TEXT NOT NULL, exclusions TEXT
         );",
        crate::embed::DIM
    ))
    .context("knowledge vector schema")?;
    for file in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    Ok(conn)
}

pub mod checkpoint {
    use crate::raw::Raw;
    use crate::worker::Consumer;
    use anyhow::Result;
    use rusqlite::{Connection, OptionalExtension, params};

    /// The table of raw seqs, and of op_seqs (`Consumer::checkpoints`).
    pub const SEQS: &str = "checkpoints";
    pub const OPS: &str = "op_checkpoints";

    /// 0 when the consumer has not started on this device.
    pub fn get(k: &Connection, consumer: &str, device: &str) -> Result<i64> {
        get_in(k, SEQS, consumer, device)
    }

    /// `get` in `table`, `SEQS` or `OPS`.
    pub fn get_in(k: &Connection, table: &str, consumer: &str, device: &str) -> Result<i64> {
        Ok(k.query_row(
            &format!("SELECT seq FROM {table} WHERE consumer = ?1 AND device = ?2"),
            params![consumer, device],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
    }

    pub fn set_in(
        k: &Connection,
        table: &str,
        consumer: &str,
        device: &str,
        seq: i64,
    ) -> Result<()> {
        k.execute(
            &format!(
                "INSERT INTO {table}(consumer, device, seq) VALUES(?1, ?2, ?3)
                 ON CONFLICT(consumer, device) DO UPDATE SET seq = excluded.seq"
            ),
            params![consumer, device, seq],
        )?;
        Ok(())
    }

    /// MUST-M14: a checkpoint above the highest seq raw holds for its device (an op_seq for an op
    /// consumer, `Consumer::top`) means raw lost commits that a consumer had already processed.
    /// The consumer's output above that seq, the checkpoint and a `rewinds` row for doctor move in
    /// one transaction, so nothing of a lost seq survives to collide with the event that reuses
    /// it, and no rewind goes unreported. Every checkpoint the consumer has is checked, not only
    /// the devices it reads now: a device that lost all its ops is no longer one of them.
    /// Returns (consumer, was, now).
    pub fn rewind(
        raw: &Raw,
        k: &Connection,
        consumers: &mut [Box<dyn Consumer>],
    ) -> Result<Vec<(String, i64, i64)>> {
        let mut moved = Vec::new();
        for c in consumers.iter_mut() {
            let table = c.checkpoints();
            let rows: Vec<(String, i64)> = k
                .prepare(&format!(
                    "SELECT device, seq FROM {table} WHERE consumer = ?1 ORDER BY device"
                ))?
                .query_map([c.name()], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (device, was) in rows {
                let top = c.top(raw, &device)?;
                if was <= top {
                    continue;
                }
                // Immediate, as a pass's (worker.rs): a read before the write must not lose the
                // lock.
                let tx = rusqlite::Transaction::new_unchecked(
                    k,
                    rusqlite::TransactionBehavior::Immediate,
                )?;
                c.rewind(&tx, &device, top)?;
                set_in(&tx, table, c.name(), &device, top)?;
                tx.execute(
                    "INSERT INTO rewinds(ts, consumer, device, was, now) VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![crate::db::now_ms(), c.name(), device, was, top],
                )?;
                tx.commit()?;
                moved.push((c.name().to_owned(), was, top));
            }
        }
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    /// Milestone 4 D8: a process whose first store is knowledge.db reads `vec_index`, so `open`
    /// registers sqlite-vec itself. Run again in a child process of its own, since another test's
    /// `db::open` registers the module for every connection of this one.
    #[test]
    fn a_knowledge_db_opened_alone_reads_its_vector_index() {
        const HOME: &str = "OBOETE_TEST_KNOWLEDGE_ALONE";
        let zero = vec![0u8; crate::embed::DIM / 8];
        let nearest = |k: &rusqlite::Connection| -> i64 {
            k.query_row(
                "SELECT rowid FROM vec_index WHERE embedding MATCH vec_bit(?1) AND k = 1
                   AND embedder = 'bge-m3' AND kind = 'c'",
                [&zero],
                |r| r.get(0),
            )
            .unwrap()
        };
        if let Ok(home) = std::env::var(HOME) {
            let k = super::open(std::path::Path::new(&home)).unwrap();
            assert_eq!(nearest(&k), 7);
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let k = super::open(home.path()).unwrap();
        k.execute(
            "INSERT INTO vec_index(rowid, embedder, kind, repo, ts, session, embedding)
             VALUES (7, 'bge-m3', 'c', '', 0, '', vec_bit(?1))",
            [&zero],
        )
        .unwrap();
        assert_eq!(nearest(&k), 7);
        drop(k);
        let name = "knowledge::tests::a_knowledge_db_opened_alone_reads_its_vector_index";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name])
            .env(HOME, home.path())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success() && said.contains("1 passed"), "{said}");
    }
}
