//! Parity row 3, decision 57: a card or a session summary removed from the page alone, reversibly
//! (docs/card-hide.md). A `hide` op names it, with no text; the cards and turns consumers keep each
//! in `hides` and mark the card or the summary (`hidden`), and every reader for a person or an agent
//! leaves a marked one out. Erasing one stays forgetting its session or the prompt it came from.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::raw::{Op, OpKind};

/// What a `hide` op names: a card of a window op (`n`, its place there) or a turn op's summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct HideOp {
    /// `card` or `summary`.
    pub target: String,
    pub device: String,
    pub op_seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<i64>,
    pub hidden: bool,
}

pub(crate) const CARD: &str = "card";
pub(crate) const SUMMARY: &str = "summary";

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "-- Each hide op kept, with its time: the newest one's mark is a card's or a summary's,
         -- whatever order the devices' ops are read in, and a rewind marks them again from those
         -- left.
         CREATE TABLE IF NOT EXISTS hides(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, ts INTEGER NOT NULL,
           target TEXT NOT NULL, device TEXT NOT NULL, target_seq INTEGER NOT NULL,
           n INTEGER NOT NULL DEFAULT -1, hidden INTEGER NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS hides_target ON hides(target, device, target_seq, n);",
    )?;
    Ok(())
}

/// A `hide` op of `target` kept in `hides`: what it names, or None for another target's, a
/// forgotten op or one that does not read.
pub(crate) fn keep(k: &Connection, op: &Op, target: &str) -> Result<Option<HideOp>> {
    if op.kind != OpKind::Hide || op.forgotten() {
        return Ok(None);
    }
    let Ok(h) = serde_json::from_value::<HideOp>(op.body.clone()) else {
        return Ok(None);
    };
    if h.target != target {
        return Ok(None);
    }
    schema(k)?;
    k.execute(
        "INSERT OR REPLACE INTO hides(op_device, op_seq, ts, target, device, target_seq, n,
           hidden)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            op.device,
            op.op_seq,
            op.ts,
            h.target,
            h.device,
            h.op_seq,
            h.n.unwrap_or(-1),
            h.hidden
        ],
    )?;
    Ok(Some(h))
}

/// A rewind of `device`'s ops past `to`: its hide ops of `target` go, and each card or summary
/// they named takes the mark of the newest one left (none: shown). The ones whose mark changed,
/// as (device, op seq, n).
pub(crate) fn rewind(
    k: &Connection,
    device: &str,
    to: i64,
    target: &str,
) -> Result<Vec<(String, i64, i64, bool)>> {
    schema(k)?;
    let named: Vec<(String, i64, i64)> = k
        .prepare(
            "SELECT DISTINCT device, target_seq, n FROM hides
             WHERE op_device = ?1 AND op_seq > ?2 AND target = ?3",
        )?
        .query_map(params![device, to, target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    k.execute(
        "DELETE FROM hides WHERE op_device = ?1 AND op_seq > ?2 AND target = ?3",
        params![device, to, target],
    )?;
    let mut marks = Vec::new();
    for (d, seq, n) in named {
        marks.push((d.clone(), seq, n, hidden(k, target, &d, seq, n)?));
    }
    Ok(marks)
}

/// The mark the newest kept hide op of `target` gives (`device`, `op_seq`, `n`), by time and
/// then by device and op seq, as `newest` orders them: false with none.
pub(crate) fn hidden(
    k: &Connection,
    target: &str,
    device: &str,
    op_seq: i64,
    n: i64,
) -> Result<bool> {
    Ok(k.query_row(
        "SELECT hidden FROM hides WHERE target = ?1 AND device = ?2 AND target_seq = ?3
               AND n = ?4 ORDER BY ts DESC, op_device DESC, op_seq DESC LIMIT 1",
        params![target, device, op_seq, n],
        |r| r.get::<_, bool>(0),
    )
    .optional()?
    .unwrap_or(false))
}

/// The mark the newest hide op of `target` in raw.db gives (`device`, `op_seq`, `n`), applied or
/// not, or none with none: what a removal on the page compares with, so an Undo pressed before
/// the worker applied the removal is recorded.
pub(crate) fn newest(
    raw: &crate::raw::Raw,
    target: &str,
    (device, op_seq, n): (&str, i64, Option<i64>),
) -> Result<Option<bool>> {
    Ok(raw
        .every_op_of(OpKind::Hide)?
        .into_iter()
        .filter_map(|(ts, op_device, seq, body)| {
            let h = serde_json::from_value::<HideOp>(body).ok()?;
            (h.target == target && h.device == device && h.op_seq == op_seq && h.n == n)
                .then_some(((ts, op_device, seq), h.hidden))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, hidden)| hidden))
}

/// `AND <alias>hidden = 0` for a reader of `table` (`cards` or `turns`), or nothing while the
/// store has no such column yet: a hook can read a store the worker has not given it.
pub(crate) fn shown(k: &Connection, table: &str, alias: &str) -> Result<String> {
    let has: bool = k.query_row(
        &format!("SELECT count(*) > 0 FROM pragma_table_info('{table}') WHERE name = 'hidden'"),
        [],
        |r| r.get(0),
    )?;
    Ok(if has {
        format!("AND {alias}hidden = 0")
    } else {
        String::new()
    })
}
