//! The none tier's search (docs/milestone-2-plan.md Task 6): every event's text in an FTS5 trigram
//! index in knowledge.db, `raw_fts`, with its (device, seq) and labels in `raw_docs`.

use crate::raw::{Item, Raw, Record, Target};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub struct Fts;

/// Records per step: one knowledge.db transaction each, so a long backlog commits as it goes.
const BATCH: usize = 500;

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS raw_docs(
           rowid INTEGER PRIMARY KEY, device TEXT NOT NULL, seq INTEGER NOT NULL,
           kind TEXT NOT NULL, ts INTEGER NOT NULL, repo TEXT, session TEXT,
           UNIQUE (device, seq)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS raw_fts USING fts5(text, tokenize='trigram');
         -- The repositories each session touched: derived, so a rewind cannot leave one behind.
         CREATE VIEW IF NOT EXISTS session_repos AS
           SELECT DISTINCT session, repo FROM raw_docs WHERE repo IS NOT NULL;",
    )?;
    Ok(())
}

/// The text a person would search for: every string in the event's JSON body (not its keys), or
/// the body itself when it is not JSON.
pub(crate) fn text(body: &str) -> String {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(xs) => xs.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(m) => m.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
            let mut out = Vec::new();
            walk(&v, &mut out);
            out.join("\n")
        }
        _ => body.to_owned(),
    }
}

/// Q4: index the stored card fields, without the reader's current gate. Called by the cards
/// consumer in its transaction, and once for rows predating the index when its schema is made.
/// `None` restores every current row after a rewind or on that first backfill.
pub(crate) fn cards(k: &Connection, rowid: Option<i64>) -> Result<()> {
    let mut st = k.prepare(
        "SELECT rowid, title, subtitle, narrative, facts, concepts, files_read, files_modified
         FROM cards WHERE replaced_by IS NULL AND (?1 IS NULL OR rowid = ?1)",
    )?;
    let rows = st.query_map([rowid], |r| {
        let mut fields = Vec::new();
        for i in 1..=7 {
            let s: String = r.get(i)?;
            fields.push(if i >= 4 {
                serde_json::from_str::<Vec<String>>(&s)
                    .unwrap_or_default()
                    .join("\n")
            } else {
                s
            });
        }
        Ok((r.get::<_, i64>(0)?, fields.join("\n")))
    })?;
    for row in rows {
        let (id, text) = row?;
        k.execute(
            "INSERT OR REPLACE INTO cards_fts(rowid, text) VALUES(?1, ?2)",
            params![id, text],
        )?;
    }
    Ok(())
}

/// Q4: every displayed summary field, `notes` among them since #403, never a skipped turn.
pub(crate) fn turns(k: &Connection, rowid: Option<i64>) -> Result<()> {
    let mut st = k.prepare(
        "SELECT rowid, fields FROM turns WHERE skipped = 0 AND (?1 IS NULL OR rowid = ?1)",
    )?;
    let rows = st.query_map([rowid], |r| {
        let fields: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
        let text = crate::turns::FIELDS
            .iter()
            .filter_map(|f| fields.get(*f).map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        Ok((r.get::<_, i64>(0)?, text))
    })?;
    for row in rows {
        let (id, text) = row?;
        k.execute(
            "INSERT OR REPLACE INTO turns_fts(rowid, text) VALUES(?1, ?2)",
            params![id, text],
        )?;
    }
    Ok(())
}

/// One indexed record's text replaced by what `Raw::after` returns for it now; its row removed
/// when that is no event. Nothing for a record this index never held.
fn reindex(raw: &Raw, k: &Connection, device: &str, seq: i64) -> Result<()> {
    let Some(rowid) = k
        .query_row(
            "SELECT rowid FROM raw_docs WHERE device = ?1 AND seq = ?2",
            params![device, seq],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    k.execute("DELETE FROM raw_fts WHERE rowid = ?1", [rowid])?;
    match raw
        .after(device, seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == seq)
    {
        Some(Record {
            item: Item::Event(e),
            ..
        }) => {
            k.execute(
                "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
                params![rowid, text(&e.body)],
            )?;
        }
        _ => {
            k.execute("DELETE FROM raw_docs WHERE rowid = ?1", [rowid])?;
        }
    }
    crate::embed_phase::touched(k, "r", &format!("{device}:{seq}"))
}

impl Consumer for Fts {
    fn name(&self) -> &'static str {
        "fts"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let recs = raw.after(raw.device(), after, BATCH)?;
        for r in &recs {
            let e = match &r.item {
                Item::Event(e) => e,
                // D8: the target indexed again as raw returns it now (masked, or gone).
                Item::Tombstone(
                    Target::Record { device, seq } | Target::Range { device, seq, .. },
                ) => {
                    reindex(raw, k, device, *seq)?;
                    continue;
                }
                Item::Removed => continue,
            };
            k.execute(
                "INSERT INTO raw_docs(device, seq, kind, ts, repo, session) VALUES(?1,?2,?3,?4,?5,?6)",
                params![r.device, r.seq, e.kind, e.ts, e.repo, e.session],
            )?;
            k.execute(
                "INSERT INTO raw_fts(rowid, text) VALUES(last_insert_rowid(), ?1)",
                [text(&e.body)],
            )?;
        }
        Ok(recs.last().map_or(after, |r| r.seq))
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        let seqs: Vec<i64> = k
            .prepare("SELECT seq FROM raw_docs WHERE device = ?1 AND seq > ?2")?
            .query_map(params![device, to], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        k.execute(
            "DELETE FROM raw_fts WHERE rowid IN (SELECT rowid FROM raw_docs WHERE device = ?1 AND seq > ?2)",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM raw_docs WHERE device = ?1 AND seq > ?2",
            params![device, to],
        )?;
        for seq in seqs {
            crate::embed_phase::touched(k, "r", &format!("{device}:{seq}"))?;
        }
        Ok(())
    }
}
