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
                "DELETE FROM turns_fts WHERE rowid IN
                   (SELECT rowid FROM turns WHERE device = ?1 AND op_seq = ?2)",
                params![device, op.op_seq],
            )?;
            k.execute(
                "INSERT OR REPLACE INTO turns(device, op_seq, ts, agent, session, repo, from_seq,
                   through, read, goals, removed, fields, skipped, excluded)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                    t.skipped,
                    t.excluded
                ],
            )?;
            crate::consumer::fts::turns(k, Some(k.last_insert_rowid()))?;
            // A row written again over its key: its vector follows its text (docs/tools.md V3).
            let key = format!("S{device}.{}", op.op_seq);
            crate::embed_phase::touched(None, k, "s", &key)?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        let lost: Vec<String> = k
            .prepare(
                "SELECT 'S' || device || '.' || op_seq FROM turns
                 WHERE device = ?1 AND op_seq > ?2",
            )?
            .query_map(params![device, to], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        k.execute(
            "DELETE FROM turns_fts WHERE rowid IN
               (SELECT rowid FROM turns WHERE device = ?1 AND op_seq > ?2)",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM turns WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        for key in lost {
            crate::embed_phase::touched(None, k, "s", &key)?;
        }
        Ok(())
    }
}
