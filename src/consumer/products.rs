//! The work product consumer (docs/my-work.md, slice 4): where each tool call that may have made
//! something a person opens is, by its record and conversation, and nothing it said. A tool
//! record's text is in no index (#317), so My Work reads these records live when a request's page
//! opens: a forgotten or expired one is no call, and what it made goes with it.

use crate::raw::{Item, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Products;

/// Records per step, as the FTS consumer's.
const BATCH: usize = 500;

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS product_calls(
           device TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL,
           agent TEXT NOT NULL, session TEXT NOT NULL,
           PRIMARY KEY (device, seq)
         );
         CREATE INDEX IF NOT EXISTS product_calls_session
           ON product_calls(agent, session, device, seq);",
    )?;
    Ok(())
}

impl Consumer for Products {
    fn name(&self) -> &'static str {
        "products"
    }

    fn reads_ops(&self) -> bool {
        false
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let recs = raw.after(device, after, BATCH)?;
        let Some(last) = recs.last().map(|r| r.seq) else {
            return Ok(after);
        };
        for r in &recs {
            let Item::Event(e) = &r.item else { continue };
            // A live session's, as My Work's requests are: an imported transcript is no request.
            let live = crate::raw::is_live(&e.source);
            if !live || e.kind != "tool" || !crate::work_product::may_make(&e.body) {
                continue;
            }
            k.execute(
                "INSERT OR IGNORE INTO product_calls(device, seq, ts, agent, session)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![device, r.seq, e.ts, e.agent, e.session],
            )?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM product_calls WHERE device = ?1 AND seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
