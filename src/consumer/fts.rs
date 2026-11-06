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
           SELECT DISTINCT session, repo FROM raw_docs WHERE repo IS NOT NULL;
         CREATE INDEX IF NOT EXISTS raw_docs_session ON raw_docs(session, ts, device, seq);",
    )?;
    if !crate::consumer::manifest::exists(k, "table", "session_start")? {
        // #506: where each session started, which maps an imported document to the repository
        // whose prompts it may reach (docs/prompt-context.md, the second slice). A store that has
        // read records already fills it from them, in the same transaction as the table; another
        // process may have filled it since the check, so a row it has stays.
        let tx = if k.is_autocommit() {
            Some(k.unchecked_transaction()?)
        } else {
            None
        };
        k.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_start(
               session TEXT PRIMARY KEY, repo TEXT NOT NULL, ts INTEGER NOT NULL,
               device TEXT NOT NULL, seq INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS session_start_repo ON session_start(repo);
             INSERT OR IGNORE INTO session_start(session, repo, ts, device, seq)
               SELECT session, repo, ts, device, seq FROM (
                 SELECT session, repo, ts, device, seq, row_number() OVER (
                   PARTITION BY session ORDER BY ts, device, seq
                 ) AS n
                 FROM raw_docs WHERE session IS NOT NULL AND repo IS NOT NULL
               ) WHERE n = 1;",
        )?;
        if let Some(tx) = tx {
            tx.commit()?;
        }
    }
    Ok(())
}

/// #506: `session`'s first row with a repository, by time, then device and seq, kept in
/// `session_start`, after a row of it came or went.
fn started(k: &Connection, session: &str) -> Result<()> {
    k.execute("DELETE FROM session_start WHERE session = ?1", [session])?;
    k.execute(
        "INSERT INTO session_start(session, repo, ts, device, seq)
         SELECT session, repo, ts, device, seq FROM raw_docs
         WHERE session = ?1 AND repo IS NOT NULL ORDER BY ts, device, seq LIMIT 1",
        [session],
    )?;
    Ok(())
}

/// Whether an event of `kind` is searched: tool output (nine records in ten, nearly all the text)
/// stays in raw.db, out of `raw_fts` and the vectors, as claude-mem searches none (#317, decision
/// 42). Its `raw_docs` row stays, for its labels and the counts. SQL that picks records says
/// `kind <> 'tool'` for the same rule.
pub(crate) fn indexed(kind: &str) -> bool {
    // #745: nor the owner's directive, when a row of it from before #743 is indexed again.
    kind != "tool" && kind != "directive"
}

/// The text a person would search for: every string in the event's JSON body (not its keys), or
/// the body itself when it is not JSON. Where a Claude Code prompt was typed or sent from
/// (`entrypoint`, #659) is a mark on the record, as `curate::long_text` reads it (#841): no word of
/// it (#843).
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
        Ok(serde_json::Value::Object(m)) => {
            let mut out = Vec::new();
            m.iter()
                .filter(|(key, _)| *key != "entrypoint")
                .for_each(|(_, v)| walk(v, &mut out));
            out.join("\n")
        }
        Ok(v @ serde_json::Value::Array(_)) => {
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
         WHERE replaced_by IS NULL AND hidden = 0 AND (?1 IS NULL OR rowid = ?1)"
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
        "SELECT rowid, fields FROM turns
         WHERE skipped = 0 AND hidden = 0 AND (?1 IS NULL OR rowid = ?1)",
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
    let Some((rowid, kind, session)) = k
        .query_row(
            "SELECT rowid, kind, session FROM raw_docs WHERE device = ?1 AND seq = ?2",
            params![device, seq],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(());
    };
    // A home made before tool output stayed out still holds its row: it goes too.
    k.execute("DELETE FROM raw_fts WHERE rowid = ?1", [rowid])?;
    let now = raw.at(device, &[seq])?.into_iter().next().map(|r| r.item);
    match now {
        Some(Item::Event(e)) => {
            if indexed(&kind) {
                k.execute(
                    "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
                    params![rowid, text(&e.body)],
                )?;
            }
        }
        // docs/retention.md: an expired record keeps its labels' row; only its text goes.
        Some(Item::Expired(_)) => {}
        _ => {
            k.execute("DELETE FROM raw_docs WHERE rowid = ?1", [rowid])?;
            if let Some(session) = session {
                started(k, &session)?;
            }
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
        // N5 (docs/memory-lookup.md): a timeline's prompts of a repository in time order
        // (`search::b::timeline`), the consumer's to make, never a reader's.
        k.execute_batch(
            "CREATE INDEX IF NOT EXISTS raw_docs_prompts ON raw_docs(repo, ts, device, seq)
               WHERE kind = 'prompt'",
        )?;
        // X10 (docs/my-work.md): the tool calls near a work state write, by time
        // (`my_work::link`), the consumer's to make, as the prompts' is.
        k.execute_batch(
            "CREATE INDEX IF NOT EXISTS raw_docs_tools ON raw_docs(ts, device, seq)
               WHERE kind = 'tool'",
        )?;
        // The repositories' records counted with their newest time (`view::repos_in`) from an
        // index, not by reading each record's text: 4.5 s cold on this PC's store (2026-11-01).
        k.execute_batch("CREATE INDEX IF NOT EXISTS raw_docs_repo ON raw_docs(repo, ts)")?;
        let recs = raw.after(raw.device(), after, BATCH)?;
        for r in &recs {
            let (e, text) = match &r.item {
                Item::Event(e) => (e, true),
                // docs/retention.md: an expired record keeps its labels and where its session
                // started (#506's mapping); it has no text to index.
                Item::Expired(e) => (e, false),
                // D8: the target indexed again as raw returns it now (masked, or gone).
                Item::Tombstone(
                    Target::Record { device, seq } | Target::Range { device, seq, .. },
                ) => {
                    reindex(raw, k, device, *seq)?;
                    continue;
                }
                Item::Removed => continue,
            };
            // #743: the owner's directive (`pref add`) is its claim, searched and delivered as one:
            // no raw search hit of its own, and no session that no agent ran. X17
            // (docs/subagents.md): a subagent's stop is a mark with no words.
            if e.kind == "directive" || e.kind == crate::raw::SUBAGENT_STOP {
                continue;
            }
            k.execute(
                "INSERT INTO raw_docs(device, seq, kind, ts, repo, session) VALUES(?1,?2,?3,?4,?5,?6)",
                params![r.device, r.seq, e.kind, e.ts, e.repo, e.session],
            )?;
            // The text's row is the record's, read before `session_start` makes a row of its own.
            let rowid = k.last_insert_rowid();
            // #506: the session's first row with a repository; an import of older records can
            // come after newer ones.
            if let Some(repo) = &e.repo {
                k.execute(
                    "INSERT INTO session_start(session, repo, ts, device, seq) VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(session) DO UPDATE SET
                       repo = excluded.repo, ts = excluded.ts, device = excluded.device,
                       seq = excluded.seq
                     WHERE (excluded.ts, excluded.device, excluded.seq)
                       < (session_start.ts, session_start.device, session_start.seq)",
                    params![e.session, repo, e.ts, r.device, r.seq],
                )?;
            }
            if text && indexed(&e.kind) {
                k.execute(
                    "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
                    params![rowid, self::text(&e.body)],
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
        let sessions: Vec<String> = k
            .prepare(
                "SELECT DISTINCT session FROM raw_docs
                 WHERE device = ?1 AND seq > ?2 AND session IS NOT NULL",
            )?
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
        for session in sessions {
            started(k, &session)?;
        }
        for seq in seqs {
            // Its row is gone: no text is read.
            crate::embed_phase::touched(None, k, "r", &format!("{device}:{seq}"))?;
        }
        Ok(())
    }
}

/// #843: the rows `raw_fts` indexed before a prompt's entry point left `text`, indexed again once,
/// and so their vectors made again. A consumer of its own, before `Fts` (`worker::consumers`): the
/// rows past its checkpoint are those an older oboete indexed, and each step it takes is progress a
/// drain goes on with.
pub struct FtsText;

/// Rows of `FtsText` a step: one knowledge.db transaction each.
pub(crate) const AGAIN: usize = 200;

impl Consumer for FtsText {
    fn name(&self) -> &'static str {
        "fts-text"
    }

    /// Every device with records in raw.db, and this one, as search reads them: a copied home's
    /// records under its old id were indexed before the copy as this device's were (Sourcery on
    /// #846).
    fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
        let mut devices = raw.devices()?;
        if !devices.iter().any(|d| d == raw.device()) {
            devices.push(raw.device().to_owned());
        }
        Ok(devices)
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        // The checkpoint moves only while this oboete runs: the rows past it are those an older
        // one indexed, before this one ran or since (Sourcery on #846).
        let seqs: Vec<i64> = k
            .prepare_cached(
                "SELECT seq FROM raw_docs WHERE device = ?1 AND seq > ?2
                   AND kind IN ('prompt', 'envelope') ORDER BY seq LIMIT ?3",
            )?
            .query_map(params![device, after, AGAIN as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for r in raw.at(device, &seqs)? {
            if let Item::Event(e) = &r.item
                && marked(&e.body)
            {
                reindex(raw, k, device, r.seq)?;
            }
        }
        // Fewer than a step's rows: done. The records past them are `Fts`'s to index, after this
        // in each pass, as `text` reads them now.
        Ok(match seqs.get(AGAIN - 1) {
            Some(&last) => last,
            None => raw.max_seq_of(device)?,
        })
    }

    fn rewind(&mut self, _k: &Connection, _device: &str, _to: i64) -> Result<()> {
        // Nothing: `Fts` deletes the rows past `to`, and indexes what comes there again with `text`
        // as it is now.
        Ok(())
    }
}

/// Whether a record's body holds a Claude Code prompt's entry point at its root (#659).
fn marked(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body).is_ok_and(|v| v.get("entrypoint").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::checkpoint;
    use crate::search::b::fixture::Store;
    use serde_json::json;

    const R: &str = "github.com/o/r";

    /// The seqs of the records whose `raw_fts` row `word` finds, in order.
    fn finds(k: &Connection, word: &str) -> Vec<i64> {
        k.prepare(
            "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid
             WHERE raw_fts MATCH ?1 ORDER BY d.seq",
        )
        .unwrap()
        .query_map([word], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    /// Record `seq`'s `raw_fts` row made of `text`.
    fn indexed_as(k: &Connection, device: &str, seq: i64, text: &str) {
        let rowid: i64 = k
            .query_row(
                "SELECT rowid FROM raw_docs WHERE device = ?1 AND seq = ?2",
                params![device, seq],
                |r| r.get(0),
            )
            .unwrap();
        k.execute("DELETE FROM raw_fts WHERE rowid = ?1", [rowid])
            .unwrap();
        k.execute(
            "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
            params![rowid, text],
        )
        .unwrap();
    }

    /// #843: the rows an older oboete indexed with a prompt's entry point as a line of its text
    /// are indexed again once, at most `AGAIN` prompts and envelopes a step (tool output between
    /// them read by none), each step a knowledge.db transaction of its own; then the consumer's
    /// checkpoint follows raw's top, as a caught-up consumer's does.
    #[test]
    fn rows_indexed_with_an_entry_point_are_indexed_again_once_in_batches() {
        let mut s = Store::new();
        let device = s.raw.device().to_owned();
        let typed = |words: &str| json!({"prompt": words, "entrypoint": "cli"});
        let mut rows = Vec::new();
        for i in 0..AGAIN {
            let words = format!("Fix parser number {i}.");
            let ts = 1_000 + i as i64;
            rows.push((
                s.event("prompt", "s", (R, "main"), ts, typed(&words)),
                words,
            ));
            s.event("tool", "s", (R, "main"), ts, json!({"tool_name": "Bash"}));
        }
        let words = "<task-notification>Build passed.</task-notification>".to_owned();
        rows.push((
            s.event("envelope", "s", (R, "main"), 3_000, typed(&words)),
            words,
        ));
        let says = s.event(
            "prompt",
            "s",
            (R, "main"),
            4_000,
            typed("Run the cli tests."),
        );
        s.run();
        let mut k = crate::knowledge::open(s.home.path()).unwrap();
        assert_eq!(finds(&k, "cli"), [says]);
        // The index as an older oboete left it: the entry point a line of each prompt's and
        // envelope's text, and no catch-up begun.
        for (seq, words) in &rows {
            indexed_as(&k, &device, *seq, &format!("{words}\ncli"));
        }
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &device, 0).unwrap();
        assert_eq!(finds(&k, "cli").len(), AGAIN + 2);
        // Each row indexed again is found by its words (Sourcery on #846: not only gone).
        let prompts: Vec<i64> = rows[..AGAIN].iter().map(|(seq, _)| *seq).collect();
        let mut again = crate::worker::consumers(s.home.path())
            .into_iter()
            .find(|c| c.name() == "fts-text")
            .expect("the consumer that indexes the rows again");
        let tx = k
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let first = again.step(&s.raw, &tx, &device, 0).unwrap();
        tx.commit().unwrap();
        assert_eq!(first, rows[AGAIN - 1].0);
        assert_eq!(finds(&k, "cli"), [rows[AGAIN].0, says]);
        assert_eq!(finds(&k, "parser"), prompts);
        // A drain goes on from there and finishes it: the checkpoint is raw's top.
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &device, first).unwrap();
        s.run();
        assert_eq!(finds(&k, "cli"), [says]);
        assert_eq!(finds(&k, "parser"), prompts);
        assert_eq!(finds(&k, "passed"), [rows[AGAIN].0]);
        let top = s.raw.max_seq_of(&device).unwrap();
        assert_eq!(checkpoint::get(&k, "fts-text", &device).unwrap(), top);
        // Once: a row made as before after the catch-up stays as it is through the next drain.
        indexed_as(&k, &device, rows[0].0, &format!("{}\ncli", rows[0].1));
        s.run();
        assert_eq!(finds(&k, "cli"), [rows[0].0, says]);
    }

    /// X17 (docs/subagents.md): a subagent's stop is a mark with no words: no `raw_docs` row, so
    /// no search hit, vector or snippet, as the owner's directive has none.
    #[test]
    fn a_subagent_stop_is_no_row_of_the_index() {
        let mut s = Store::new();
        let body = json!({"agent_id": "a1", "agent_type": "Explore"});
        let seq = s.event("subagent_stop", "s", (R, "main"), 1_000, body);
        s.event(
            "prompt",
            "s",
            (R, "main"),
            2_000,
            json!({"prompt": "Explore the parser."}),
        );
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let rows: i64 = k
            .query_row("SELECT count(*) FROM raw_docs WHERE seq = ?1", [seq], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0);
        assert_eq!(finds(&k, "Explore"), [seq + 1]);
    }

    /// Sourcery on #846: an older oboete run after the catch-up indexes new prompts with their entry
    /// point and leaves this consumer's checkpoint where it was, so this one indexes them again
    /// when it runs again.
    #[test]
    fn rows_an_older_oboete_indexes_after_the_catch_up_are_indexed_again() {
        let mut s = Store::new();
        let device = s.raw.device().to_owned();
        let typed = |words: &str| json!({"prompt": words, "entrypoint": "cli"});
        s.event("prompt", "s", (R, "main"), 1_000, typed("Fix the parser."));
        s.run();
        let later = s.event("prompt", "s", (R, "main"), 2_000, typed("Ship the lexer."));
        // The older oboete: its consumers without this one, its text with the entry point.
        let mut k = crate::knowledge::open(s.home.path()).unwrap();
        crate::worker::drain(&s.raw, &mut k, &mut [Box::new(Fts) as Box<dyn Consumer>]).unwrap();
        indexed_as(&k, &device, later, "Ship the lexer.\ncli");
        assert_eq!(finds(&k, "cli"), [later]);
        s.run();
        assert_eq!(finds(&k, "cli"), Vec::<i64>::new());
        assert_eq!(finds(&k, "lexer"), [later]);
    }

    /// Sourcery on #846: a copied home's records left under its old id (`Raw::devices`) were
    /// indexed before the copy as this device's were, and are indexed again too.
    #[test]
    fn a_copied_homes_rows_under_its_old_id_are_indexed_again() {
        let mut s = Store::new();
        let old = s.raw.device().to_owned();
        let body = json!({"prompt": "Fix the parser.", "entrypoint": "cli"});
        let seq = s.event("prompt", "s", (R, "main"), 1_000, body);
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        indexed_as(&k, &old, seq, "Fix the parser.\ncli");
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &old, 0).unwrap();
        // The copy's raw.db under a new id, as a copied file gets one (`db::ensure_device`).
        rusqlite::Connection::open(s.home.path().join("raw.db"))
            .unwrap()
            .execute("UPDATE meta SET value = 'copy' WHERE key = 'device_id'", [])
            .unwrap();
        s.run();
        assert_eq!(finds(&k, "cli"), Vec::<i64>::new());
        assert_eq!(finds(&k, "parser"), [seq]);
    }

    /// #843: where a Claude Code prompt was typed or sent from (`entrypoint`, #659) is a mark on the
    /// record, as `curate::long_text` reads it (#841), not a word of its text: neither indexed nor
    /// embedded. A field the renderer does not know is a word as before, and so is an `entrypoint`
    /// inside a value.
    #[test]
    fn a_prompts_entry_point_is_no_word_of_its_text() {
        for entry in ["cli", "claude-vscode", "sdk-cli", "remote_trigger"] {
            let of = |body: serde_json::Value| text(&body.to_string());
            assert_eq!(
                of(json!({"prompt": "Fix the parser.", "entrypoint": entry})),
                "Fix the parser."
            );
            assert_eq!(
                of(json!({"prompt": "Ship it.", "agent_sent": true, "entrypoint": entry})),
                "Ship it."
            );
            assert_eq!(of(json!({"omitted": true, "entrypoint": entry})), "");
        }
        assert_eq!(
            text(
                &json!({"prompt": "Fix the parser.", "entrypoint": "cli", "note": "kept"})
                    .to_string()
            ),
            "Fix the parser.\nkept"
        );
        assert_eq!(
            text(&json!({"tool_input": {"entrypoint": "main.rs"}}).to_string()),
            "main.rs"
        );
    }
}
