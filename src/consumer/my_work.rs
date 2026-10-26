//! My Work's ops consumer (docs/my-work-model.md S4): it reads the op log of every device that has
//! ops and keeps each `my_work` op in knowledge.db (`my_work_ops::schema`). `oboete rebuild` makes
//! the same rows from the ops, with no provider call.

use crate::my_work_ops::{JOBS, MyWorkOp, schema};
use crate::raw::{OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct MyWork;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;

impl Consumer for MyWork {
    fn name(&self) -> &'static str {
        "my_work"
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
        for op in ops.iter().filter(|op| op.kind == OpKind::MyWork) {
            // A synced op may come from any device: one that does not read as a My Work op is none,
            // and so is one of a job this binary does not know.
            let Ok(w) = serde_json::from_value::<MyWorkOp>(op.body.clone()) else {
                continue;
            };
            if !JOBS.contains(&w.job.as_str()) {
                continue;
            }
            k.execute(
                "INSERT OR REPLACE INTO my_work(device, op_seq, ts, job, about, entry, read,
                   removed, fields, skipped, forgotten)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    device,
                    op.op_seq,
                    w.ts,
                    w.job,
                    w.about,
                    w.entry,
                    serde_json::to_string(&w.read)?,
                    serde_json::to_string(&w.removed)?,
                    serde_json::to_string(&w.fields)?,
                    w.skipped,
                    w.forgotten
                ],
            )?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM my_work WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}
