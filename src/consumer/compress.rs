//! Milestone 2 Task 10 (spec 2.4): each record's body as zstd once every other consumer has
//! passed it. `Raw::after` decompresses, so no reader sees the difference.

use crate::raw::Raw;
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::Connection;

pub struct Compress;

impl Consumer for Compress {
    fn name(&self) -> &'static str {
        "compress"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        self.step_report(raw, k, device, after, &mut || {})
    }

    fn step_report(
        &mut self,
        raw: &Raw,
        k: &Connection,
        _device: &str,
        after: i64,
        committed: &mut dyn FnMut(),
    ) -> Result<i64> {
        // The lowest checkpoint of the others; all of raw when there are none. A consumer that
        // has not started yet is not waited for: it reads compressed records the same way.
        let others: Option<i64> = k.query_row(
            "SELECT MIN(seq) FROM checkpoints WHERE device = ?1 AND consumer <> ?2",
            (raw.device(), self.name()),
            |r| r.get(0),
        )?;
        let through = match others {
            Some(s) => s,
            None => raw.max_seq()?,
        };
        if through <= after {
            return Ok(after);
        }
        raw.compress_through_report(raw.device(), after, through, committed)?;
        Ok(through)
    }

    fn rewind(&mut self, _k: &Connection, _device: &str, _to: i64) -> Result<()> {
        Ok(()) // its output is in raw itself, which the rewind is about
    }
}

#[cfg(test)]
mod tests;
