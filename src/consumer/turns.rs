//! The session summaries consumer (docs/summaries.md T6): it reads the op log of every device that
//! has ops and keeps each turn op in knowledge.db (`turns::schema`).

use crate::raw::{OpKind, Raw};
use crate::turns::{TurnOp, schema};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

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
        for op in &ops {
            // Parity row 3: a summary removed from the page, or brought back.
            if let Some(h) = crate::hide::keep(k, op, crate::hide::SUMMARY)? {
                // The newest op's mark, not the one read last: devices are read in turn.
                let mark = crate::hide::hidden(k, crate::hide::SUMMARY, &h.device, h.op_seq, -1)?;
                hide_mark(k, &h.device, h.op_seq, mark)?;
                continue;
            }
            if op.kind != OpKind::Turn {
                continue;
            }
            // A synced op may come from any device: one that does not read as a turn is none.
            let Ok(t) = serde_json::from_value::<TurnOp>(op.body.clone()) else {
                continue;
            };
            k.execute(
                "DELETE FROM turns_fts WHERE rowid IN
                   (SELECT rowid FROM turns WHERE device = ?1 AND op_seq = ?2)",
                params![device, op.op_seq],
            )?;
            // A row written again keeps the mark its hide ops give it.
            let hidden = crate::hide::hidden(k, crate::hide::SUMMARY, device, op.op_seq, -1)?;
            k.execute(
                "INSERT OR REPLACE INTO turns(device, op_seq, ts, agent, session, repo, from_seq,
                   through, read, goals, removed, fields, skipped, excluded, hidden)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                    t.excluded,
                    hidden
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
        for (device, op_seq, _, hidden) in crate::hide::rewind(k, device, to, crate::hide::SUMMARY)?
        {
            hide_mark(k, &device, op_seq, hidden)?;
        }
        Ok(())
    }
}

/// Parity row 3: the summary of `device`'s turn op `op_seq` marked removed or not, its full-text
/// row and its vector following the mark. Its row stays: what was asked is not asked again.
fn hide_mark(k: &Connection, device: &str, op_seq: i64, hidden: bool) -> Result<()> {
    k.execute(
        "UPDATE turns SET hidden = ?3 WHERE device = ?1 AND op_seq = ?2",
        params![device, op_seq, hidden],
    )?;
    let rowid: Option<i64> = k
        .query_row(
            "SELECT rowid FROM turns WHERE device = ?1 AND op_seq = ?2",
            params![device, op_seq],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(rowid) = rowid {
        k.execute("DELETE FROM turns_fts WHERE rowid = ?1", [rowid])?;
        // Indexed again only while it is shown.
        crate::consumer::fts::turns(k, Some(rowid))?;
    }
    crate::embed_phase::touched(None, k, "s", &format!("S{device}.{op_seq}"))
}
