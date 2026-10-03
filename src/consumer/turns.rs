//! The session summaries consumer (docs/summaries.md T6): it reads the op log of every device that
//! has ops and keeps each turn op in knowledge.db (`turns::schema`).

use crate::raw::{OpKind, Raw};
use crate::turns::{TurnOp, schema};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Turns;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;

impl Consumer for Turns {
    fn name(&self) -> &'static str {
        "turns"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in ops.iter().filter(|o| o.kind == OpKind::Turn) {
            // A synced op may come from any device: one that does not read as a turn is none.
            let Ok(t) = serde_json::from_value::<TurnOp>(op.body.clone()) else {
                continue;
            };
            k.execute(
                "INSERT OR REPLACE INTO turns(device, op_seq, ts, agent, session, repo, from_seq,
                   through, read, goals, removed, fields, skipped)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    device,
                    op.op_seq,
                    t.ts,
                    t.agent,
                    t.session,
                    t.repo,
                    t.from,
                    t.through,
                    serde_json::to_string(&t.read)?,
                    serde_json::to_string(&t.goals)?,
                    serde_json::to_string(&t.removed)?,
                    serde_json::to_string(&t.fields)?,
                    t.skipped
                ],
            )?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM turns WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}
