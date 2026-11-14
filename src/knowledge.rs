//! Design B's derived store (docs/milestone-2-plan.md D10): `knowledge.db`, rebuilt from `raw.db`
//! when lost, so synchronous=NORMAL. Each consumer's checkpoint is the highest seq of a device it
//! has finished, moved in the same transaction as its output.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub fn open(home: &Path) -> Result<Connection> {
    open_report(home, &mut || Ok(()))
}

pub(crate) fn rewind_facts(conn: &Connection) -> Result<(i64, Option<i64>, Option<String>)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), CASE WHEN typeof(MAX(ts))='integer' THEN MAX(ts) END,
         strftime('%Y-%m-%d %H:%M', MAX(ts) / 1000, 'unixepoch', 'localtime') FROM rewinds",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?)
}

pub(crate) fn open_report(
    home: &Path,
    committed: &mut impl FnMut() -> Result<()>,
) -> Result<Connection> {
    crate::raw::refuse_newer_home(home)?;
    let deadline = std::time::Instant::now() + crate::db::OPEN_WRITE_WAIT;
    let path = home.join("knowledge.db");
    crate::db::private(home, 0o700);
    // Before the open: `vec_index` is a vec0 table, which a connection without the module cannot
    // read (milestone 4 D8).
    crate::db::register_sqlite_vec();
    let mut create = std::fs::OpenOptions::new();
    create.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut create, 0o600);
    match create.open(&path) {
        Ok(_) => committed()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| format!("create {}", path.display())),
    }
    // rusqlite 0.41 registers a hook through `&mut`: only the test build's harness needs it.
    #[cfg_attr(not(test), allow(unused_mut))]
    let mut conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(test)]
    crate::crash::arm(&mut conn);
    crate::db::wal_until(&conn, "NORMAL", deadline)?;
    // The base and vector objects commit together, including vec0's shadow tables.
    let schema = format!(
        concat!(
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
         );\n",
            // Milestone 4 D8: vectors by embedder and the SHA-256 of the stored text they were made from
            // (fp32, little-endian); each document's key (a claim uid, an imported uid, a record's
            // `device:seq`) to its vector, or why it has none; the searched index, whose rowid is the
            // key's id; and the embedders' generations.
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
         );"
        ),
        crate::embed::DIM
    );
    if crate::db::ensure_schema_until(&conn, &schema, deadline).context("knowledge schema")? {
        committed()?;
    }
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
        rewind_report(raw, k, consumers, &mut |_, _, _| {})
    }

    pub(crate) fn rewind_report(
        raw: &Raw,
        k: &Connection,
        consumers: &mut [Box<dyn Consumer>],
        committed: &mut impl FnMut(&'static str, bool, i64),
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
                committed(c.name(), c.reads_ops(), top);
                moved.push((c.name().to_owned(), was, top));
            }
        }
        Ok(moved)
    }
}

#[cfg(test)]
mod tests;
