//! Design B's derived store (docs/milestone-2-plan.md D10): `knowledge.db`, rebuilt from `raw.db`
//! when lost, so synchronous=NORMAL. Each consumer's checkpoint is the highest seq of a device it
//! has finished, moved in the same transaction as its output.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub fn open(home: &Path) -> Result<Connection> {
    let path = home.join("knowledge.db");
    crate::db::private(home, 0o700);
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    crate::db::wal(&conn, "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS checkpoints(
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

    /// 0 when the consumer has not started on this device.
    pub fn get(k: &Connection, consumer: &str, device: &str) -> Result<i64> {
        Ok(k.query_row(
            "SELECT seq FROM checkpoints WHERE consumer = ?1 AND device = ?2",
            params![consumer, device],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
    }

    pub fn set(k: &Connection, consumer: &str, device: &str, seq: i64) -> Result<()> {
        k.execute(
            "INSERT INTO checkpoints(consumer, device, seq) VALUES(?1, ?2, ?3)
             ON CONFLICT(consumer, device) DO UPDATE SET seq = excluded.seq",
            params![consumer, device, seq],
        )?;
        Ok(())
    }

    /// MUST-M14: a checkpoint above raw's highest seq for this device means raw lost commits that
    /// a consumer had already processed. The consumer's output above that seq, the checkpoint and
    /// a `rewinds` row for doctor move in one transaction, so nothing of a lost seq survives to
    /// collide with the event that reuses it, and no rewind goes unreported. Returns
    /// (consumer, was, now).
    pub fn rewind(
        raw: &Raw,
        k: &Connection,
        consumers: &mut [Box<dyn Consumer>],
    ) -> Result<Vec<(String, i64, i64)>> {
        let device = raw.device().to_owned();
        let top = raw.max_seq()?;
        let mut moved = Vec::new();
        for c in consumers.iter_mut() {
            let was = get(k, c.name(), &device)?;
            if was <= top {
                continue;
            }
            // Immediate, as a pass's (worker.rs): a read before the write must not lose the lock.
            let tx =
                rusqlite::Transaction::new_unchecked(k, rusqlite::TransactionBehavior::Immediate)?;
            c.rewind(&tx, &device, top)?;
            set(&tx, c.name(), &device, top)?;
            tx.execute(
                "INSERT INTO rewinds(ts, consumer, device, was, now) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![crate::db::now_ms(), c.name(), device, was, top],
            )?;
            tx.commit()?;
            moved.push((c.name().to_owned(), was, top));
        }
        Ok(moved)
    }
}
