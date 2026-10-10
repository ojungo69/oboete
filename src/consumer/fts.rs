//! The none tier's search (docs/milestone-2-plan.md Task 6): every event's text in an FTS5 trigram
//! index in knowledge.db, `raw_fts`, with its (device, seq) and labels in `raw_docs`. The index
//! keeps no copy of the text (#317): a reader reads it back from raw.db (`texts`). Tool output
//! stays out of it (`indexed`).

use crate::raw::{Item, Raw, Target};
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
         -- Contentless (#317): the text is raw.db's. A home made before keeps its copy until
         -- `oboete rebuild` makes knowledge.db again; nothing reads it.
         CREATE VIRTUAL TABLE IF NOT EXISTS raw_fts
           USING fts5(text, tokenize='trigram', content='', contentless_delete=1);
         -- The repositories each session touched: derived, so a rewind cannot leave one behind.
         CREATE VIEW IF NOT EXISTS session_repos AS
           SELECT DISTINCT session, repo FROM raw_docs WHERE repo IS NOT NULL;",
    )?;
    Ok(())
}

/// Whether an event of `kind` is searched: tool output (nine records in ten, nearly all the text)
/// stays in raw.db, out of `raw_fts` and the vectors, as claude-mem searches none (#317, decision
/// 42). Its `raw_docs` row stays, for its labels and the counts. SQL that picks records says
/// `kind <> 'tool'` for the same rule.
pub(crate) fn indexed(kind: &str) -> bool {
    kind != "tool"
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

/// The text `raw_fts` indexed for `device`'s records at `seqs`, read back from raw.db as
/// `Raw::after` returns them now (masked, D8), in seq order; one that is no event now is left out.
pub(crate) fn texts(raw: &Raw, device: &str, seqs: &[i64]) -> Result<Vec<(i64, String)>> {
    Ok(raw
        .at(device, seqs)?
        .into_iter()
        .filter_map(|r| match r.item {
            Item::Event(e) => Some((r.seq, text(&e.body))),
            _ => None,
        })
        .collect())
}

/// Of `device`'s records at `seqs`, in seq order, those whose text holds every word as SQLite's
/// `LIKE '%word%'` finds it (ASCII letters in either case). A body that cannot hold one is not
/// parsed: with no `\u` escape in it, a character JSON writes as itself (none of `"`, `\`, `/` or
/// a control character) is in the body as itself.
pub(crate) fn holding(raw: &Raw, device: &str, seqs: &[i64], words: &[&str]) -> Result<Vec<i64>> {
    let words: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    // A word with no ASCII letter is found in a text as it is: no lowercase copy of each body.
    let fold = words
        .iter()
        .any(|w| w.bytes().any(|b| b.is_ascii_alphabetic()));
    let plain = |w: &str| {
        !w.chars()
            .any(|c| matches!(c, '"' | '\\' | '/') || c.is_control())
    };
    Ok(raw
        .at(device, seqs)?
        .into_iter()
        .filter_map(|r| {
            let Item::Event(e) = r.item else {
                return None;
            };
            let lower;
            let body = if fold {
                lower = e.body.to_ascii_lowercase();
                &lower
            } else {
                &e.body
            };
            if !body.contains("\\u") && words.iter().any(|w| plain(w) && !body.contains(w.as_str()))
            {
                return None;
            }
            let mut text = text(&e.body);
            text.make_ascii_lowercase();
            words
                .iter()
                .all(|w| text.contains(w.as_str()))
                .then_some(r.seq)
        })
        .collect())
}

/// The card columns its full-text row and its vector are made of (Q4, docs/tools.md V1), in order.
pub(crate) const CARD_COLUMNS: &str =
    "title, subtitle, narrative, facts, concepts, files_read, files_modified";

/// Values one a line, with the byte range of each in the text: a reader gates each value alone
/// (K6), and so does the embedding phase (docs/tools.md V1).
#[derive(Default)]
pub(crate) struct Joined {
    pub text: String,
    pub parts: Vec<std::ops::Range<usize>>,
}

impl Joined {
    /// `value` on a line of its own after the text so far, unless it is the `first`.
    fn push(&mut self, first: bool, value: &str) {
        if !first {
            self.text.push('\n');
        }
        if !value.is_empty() {
            self.parts
                .push(self.text.len()..self.text.len() + value.len());
        }
        self.text.push_str(value);
    }
}

/// A card's searchable text from `CARD_COLUMNS` read from `r` at `at`: the lists one item a line,
/// an empty list an empty line.
pub(crate) fn card_text(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Joined> {
    let mut out = Joined::default();
    for i in 0..7 {
        let s: String = r.get(at + i)?;
        if i < 3 {
            out.push(i == 0, &s);
            continue;
        }
        let items = serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
        if items.is_empty() {
            out.push(false, "");
        }
        for item in &items {
            out.push(false, item);
        }
    }
    Ok(out)
}

/// A summary's searchable text from its `fields` JSON: every displayed field, `notes` among them.
pub(crate) fn turn_text(fields: &str) -> Joined {
    let fields: std::collections::BTreeMap<String, String> =
        serde_json::from_str(fields).unwrap_or_default();
    let mut out = Joined::default();
    let present = crate::turns::FIELDS.iter().filter_map(|f| fields.get(*f));
    for (n, value) in present.enumerate() {
        out.push(n == 0, value);
    }
    out
}

/// Q4: index the stored card fields, without the reader's current gate. Called by the cards
/// consumer in its transaction, and once for rows predating the index when its schema is made.
/// `None` restores every current row after a rewind or on that first backfill.
pub(crate) fn cards(k: &Connection, rowid: Option<i64>) -> Result<()> {
    let mut st = k.prepare(&format!(
        "SELECT rowid, {CARD_COLUMNS} FROM cards
         WHERE replaced_by IS NULL AND (?1 IS NULL OR rowid = ?1)"
    ))?;
    let rows = st.query_map([rowid], |r| {
        Ok((r.get::<_, i64>(0)?, card_text(r, 1)?.text))
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
        Ok((r.get::<_, i64>(0)?, turn_text(&r.get::<_, String>(1)?).text))
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
    let Some((rowid, kind)) = k
        .query_row(
            "SELECT rowid, kind FROM raw_docs WHERE device = ?1 AND seq = ?2",
            params![device, seq],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    // A home made before tool output stayed out still holds its row: it goes too.
    k.execute("DELETE FROM raw_fts WHERE rowid = ?1", [rowid])?;
    match texts(raw, device, &[seq])?.pop() {
        Some((_, text)) => {
            if indexed(&kind) {
                k.execute(
                    "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
                    params![rowid, text],
                )?;
            }
        }
        None => {
            k.execute("DELETE FROM raw_docs WHERE rowid = ?1", [rowid])?;
        }
    }
    crate::embed_phase::touched(Some(raw), k, "r", &format!("{device}:{seq}"))
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
            if indexed(&e.kind) {
                k.execute(
                    "INSERT INTO raw_fts(rowid, text) VALUES(last_insert_rowid(), ?1)",
                    [text(&e.body)],
                )?;
            }
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
            // Its row is gone: no text is read.
            crate::embed_phase::touched(None, k, "r", &format!("{device}:{seq}"))?;
        }
        Ok(())
    }
}
