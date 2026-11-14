//! The cards consumer (docs/cards.md): it reads the op log of every device that has ops and keeps
//! each curated window's cards in knowledge.db (`cards::schema`).

use crate::cards::schema;
use crate::raw::{OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub struct Cards;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;
impl Consumer for Cards {
    fn name(&self) -> &'static str {
        "cards"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        mark(raw, k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in &ops {
            // Parity row 3: a card removed from the page, or brought back.
            if let Some(h) = crate::hide::keep(k, op, crate::hide::CARD)? {
                // The newest op's mark, not the one read last: devices are read in turn.
                let n = h.n.unwrap_or(-1);
                let mark = crate::hide::hidden(k, crate::hide::CARD, &h.device, h.op_seq, n)?;
                hide_mark(k, &h.device, h.op_seq, n, mark)?;
                continue;
            }
            if op.kind != OpKind::Window {
                continue;
            }
            let Some(span) = crate::curate::op_span(&op.body) else {
                continue;
            };
            if op.body["recurate"] == true {
                replace(k, device, op.op_seq, &op.body)?;
            }
            if op.body["outcome"] != "curated" {
                continue;
            }
            // K1: its observations, or without that list its summary, as a card with no title. A
            // summary as long as the op keeps one may have been cut there, ungated (K6): no card.
            let summary = op.body["summary"].as_str().unwrap_or("").trim();
            let of_summary = [serde_json::json!({"narrative": summary})];
            let cards: &[serde_json::Value] = match op.body["observations"].as_array() {
                Some(cards) => cards,
                None if summary.is_empty()
                    || summary.chars().count() >= crate::curate::MAX_SUMMARY_CHARS =>
                {
                    continue;
                }
                None => &of_summary,
            };
            let labels = raw.labels_in(device, span.from, span.to)?;
            let (agent, session) = labels.session.unzip();
            let subagent = raw.subagent_only(device, span.from, span.to)?;
            // A list as the table keeps it; none where the op or the card gives none. An op
            // that lists no removal: every removal from its records hides the card (K4).
            let list = |v: &serde_json::Value| match v {
                list @ serde_json::Value::Array(_) => list.to_string(),
                _ => "[]".to_owned(),
            };
            let (goals, removed) = (list(&op.body["goals"]), list(&op.body["removed"]));
            for (n, c) in cards.iter().enumerate() {
                let text = |field: &str| c[field].as_str().unwrap_or("");
                let hidden =
                    crate::hide::hidden(k, crate::hide::CARD, device, op.op_seq, n as i64)?;
                k.execute(
                    "INSERT INTO cards(device, op_seq, n, from_seq, from_offset, to_seq,
                       to_offset, goals, removed, ts, agent, session, repo, type, title,
                       subtitle, narrative, facts, concepts, files_read, files_modified, subagent,
                       hidden)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                    params![
                        device,
                        op.op_seq,
                        n as i64,
                        span.from,
                        span.from_offset,
                        span.to,
                        span.to_offset,
                        goals,
                        removed,
                        labels.ts.unwrap_or(op.ts),
                        agent,
                        session,
                        labels.repo,
                        c["type"].as_str(),
                        text("title"),
                        text("subtitle"),
                        text("narrative"),
                        list(&c["facts"]),
                        list(&c["concepts"]),
                        list(&c["files_read"]),
                        list(&c["files_modified"]),
                        i64::from(subagent),
                        hidden
                    ],
                )?;
                crate::consumer::fts::cards(k, Some(k.last_insert_rowid()))?;
                crate::cards::index_files(
                    k,
                    device,
                    op.op_seq,
                    n as i64,
                    [&c["files_read"], &c["files_modified"]],
                )?;
            }
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        let lost = keys(k, device, "op_seq > ?2", to)?;
        k.execute(
            "DELETE FROM cards_fts WHERE rowid IN
               (SELECT rowid FROM cards WHERE device = ?1 AND op_seq > ?2)",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM card_files WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM cards WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        // What a lost recuration replaced is current again (K5).
        k.execute(
            "UPDATE cards SET replaced_by = NULL WHERE device = ?1 AND replaced_by > ?2",
            params![device, to],
        )?;
        crate::consumer::fts::cards(k, None)?;
        // Their vectors go with them (docs/tools.md V3).
        for key in lost {
            crate::embed_phase::touched(None, k, "o", &key)?;
        }
        for (device, op_seq, n, hidden) in crate::hide::rewind(k, device, to, crate::hide::CARD)? {
            hide_mark(k, &device, op_seq, n, hidden)?;
        }
        Ok(())
    }
}

/// Parity row 3: card `n` of `device`'s window op `op_seq` marked removed or not, its full-text
/// row and its vector following the mark.
fn hide_mark(k: &Connection, device: &str, op_seq: i64, n: i64, hidden: bool) -> Result<()> {
    k.execute(
        "UPDATE cards SET hidden = ?4 WHERE device = ?1 AND op_seq = ?2 AND n = ?3",
        params![device, op_seq, n, hidden],
    )?;
    let rowid: Option<i64> = k
        .query_row(
            "SELECT rowid FROM cards WHERE device = ?1 AND op_seq = ?2 AND n = ?3",
            params![device, op_seq, n],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(rowid) = rowid {
        k.execute("DELETE FROM cards_fts WHERE rowid = ?1", [rowid])?;
        // Indexed again only while it is current and shown.
        crate::consumer::fts::cards(k, Some(rowid))?;
    }
    crate::embed_phase::touched(None, k, "o", &format!("{device}.{op_seq}.{n}"))
}

/// The vector keys (`<device>.<op seq>.<n>`) of `device`'s cards where `clause` holds for `?2`.
fn keys(k: &Connection, device: &str, clause: &str, value: i64) -> Result<Vec<String>> {
    Ok(k.prepare(&format!(
        "SELECT device || '.' || op_seq || '.' || n FROM cards WHERE device = ?1 AND {clause}"
    ))?
    .query_map(params![device, value], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

/// K3: recuration op `op_seq` replaces the cards of `device`'s earlier windows that overlap what
/// it curated: its range, less the records it kept back. Not what it `covers`, which holds the
/// run's own earlier windows.
fn replace(k: &Connection, device: &str, op_seq: i64, op: &serde_json::Value) -> Result<()> {
    for part in crate::curate::curated_parts(op, op) {
        // Chosen by records; `minus` judges the offsets of a record two windows share.
        let mut st = k.prepare(
            "SELECT DISTINCT op_seq, from_seq, from_offset, to_seq, to_offset FROM cards
             WHERE device = ?1 AND op_seq < ?2 AND replaced_by IS NULL
               AND from_seq <= ?3 AND to_seq >= ?4",
        )?;
        let over = st
            .query_map(params![device, op_seq, part.to, part.from], |r| {
                let span = crate::curate::Span {
                    from: r.get(1)?,
                    from_offset: r.get(2)?,
                    to: r.get(3)?,
                    to_offset: r.get(4)?,
                };
                Ok((r.get::<_, i64>(0)?, span))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (earlier, span) in over {
            if span.minus(&part) != [span] {
                k.execute(
                    "UPDATE cards SET replaced_by = ?1 WHERE device = ?2 AND op_seq = ?3",
                    params![op_seq, device, earlier],
                )?;
                k.execute(
                    "DELETE FROM cards_fts WHERE rowid IN
                       (SELECT rowid FROM cards WHERE device = ?1 AND op_seq = ?2)",
                    params![device, earlier],
                )?;
                for key in keys(k, device, "op_seq = ?2", earlier)? {
                    crate::embed_phase::touched(None, k, "o", &key)?;
                }
            }
        }
    }
    Ok(())
}

/// Parity row X1: the cards kept before `cards.subagent` get it, all at the first step after the
/// change, since a worker that has caught up steps again only when an op comes (Codex on #561).
fn mark(raw: &Raw, k: &Connection) -> Result<()> {
    let windows: Vec<(String, i64, i64, i64)> = k
        .prepare(
            "SELECT DISTINCT device, op_seq, from_seq, to_seq FROM cards WHERE subagent IS NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (device, op_seq, from, to) in windows {
        let subagent = raw.subagent_only(&device, from, to)?;
        k.execute(
            "UPDATE cards SET subagent = ?3 WHERE device = ?1 AND op_seq = ?2",
            params![device, op_seq, i64::from(subagent)],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
