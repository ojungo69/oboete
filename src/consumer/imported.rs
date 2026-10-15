//! Milestone 4 Task 3 (D5): the imported documents consumer. It reads the op log of every device
//! that has ops and keeps each `import` op's document in knowledge.db, keyed by the op that brought
//! it, with a trigram index over its title and body. Curation reads records, never these ops, and
//! only the per-prompt block injects them (#506, `prompt_line`).

use crate::raw::{ImportDoc, OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Imported;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;

/// A document per op: a restore's rewind removes exactly the rows of the ops it lost, and a
/// document two devices imported is kept once per op (readers show it once per uid). The index
/// keeps no copy of the text, which `imported` holds (a quarter of the store on claude-mem's
/// history): a hit is read back through its rowid.
pub fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS imported(
           rowid INTEGER PRIMARY KEY, op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
           uid TEXT NOT NULL, source TEXT NOT NULL, source_id TEXT NOT NULL, kind TEXT NOT NULL,
           repo TEXT NOT NULL, session TEXT NOT NULL, ts INTEGER NOT NULL, title TEXT NOT NULL,
           body TEXT NOT NULL, UNIQUE (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS imported_uid ON imported(uid);
         -- #506: a repository's prompts read the sessions that started in it and the projects
         -- their documents are filed under, prompts left out, from the index alone.
         CREATE INDEX IF NOT EXISTS imported_session ON imported(session, repo, kind);
         CREATE VIRTUAL TABLE IF NOT EXISTS imported_fts
           USING fts5(text, tokenize='trigram', content='', contentless_delete=1);",
    )?;
    Ok(())
}

impl Consumer for Imported {
    fn name(&self) -> &'static str {
        "imported"
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
        for op in ops.into_iter().filter(|o| o.kind == OpKind::Import) {
            // An op this version cannot read stays in raw, unread.
            let Ok(d) = serde_json::from_value::<ImportDoc>(op.body) else {
                continue;
            };
            k.execute(
                "INSERT INTO imported(op_device, op_seq, uid, source, source_id, kind, repo,
                   session, ts, title, body)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    device,
                    op.op_seq,
                    d.uid,
                    d.source,
                    d.source_id,
                    d.kind,
                    d.repo,
                    d.session,
                    d.ts,
                    d.title,
                    d.body
                ],
            )?;
            k.execute(
                "INSERT INTO imported_fts(rowid, text) VALUES(?1, ?2)",
                params![k.last_insert_rowid(), format!("{}\n{}", d.title, d.body)],
            )?;
            // A uid imported again: its text is this row's now.
            crate::embed_phase::touched(None, k, "i", &d.uid)?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        let uids: Vec<String> = k
            .prepare("SELECT DISTINCT uid FROM imported WHERE op_device = ?1 AND op_seq > ?2")?
            .query_map(params![device, to], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        k.execute(
            "DELETE FROM imported_fts WHERE rowid IN
               (SELECT rowid FROM imported WHERE op_device = ?1 AND op_seq > ?2)",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM imported WHERE op_device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        for uid in uids {
            crate::embed_phase::touched(None, k, "i", &uid)?;
        }
        Ok(())
    }
}

/// #506 (docs/prompt-context.md, the second slice): whether imported document `uid` belongs to
/// `repo`'s prompts, by its newest row: a document of a session that started in `repo`
/// (`session_start`, oboete's first record of it), or an agent's note saved in `repo`, which has no
/// session of an agent's. Never by a project's name, which two checkouts may share, nor by the
/// repository a document is filed under (v1's documents are, and map through their sessions).
pub(crate) fn maps(k: &Connection, uid: &str, repo: &str) -> Result<bool> {
    Ok(k.prepare_cached(
        "SELECT 1 FROM imported i
         WHERE i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = ?1)
           AND (i.session = ?3 AND i.repo = ?2 OR EXISTS (
             SELECT 1 FROM session_start s WHERE s.session = i.session AND s.repo = ?2))",
    )?
    .exists(params![uid, repo, crate::raw::NOTE_SESSION])?)
}

/// #506 (spec 4.5, 4.12): imported document `uid` as a prompt's block names it, by its newest
/// row, when it may take a place there: its time, what it is (`note` for an agent's note, else
/// `imported <kind> (status unknown)`) and its words, its title and its body's first sentence (the
/// longer alone when one starts with the other, as a note saved without a title is titled with its
/// text's start, `mcp::untitled`). None for a prompt (a prompt's text is never injected, 4.6), a
/// forgotten document (milestone 5 D5), or one with no words.
pub(crate) fn prompt_line(
    k: &Connection,
    raw: &Raw,
    uid: &str,
) -> Result<Option<(i64, String, String)>> {
    use rusqlite::OptionalExtension;
    let row = k
        .query_row(
            "SELECT source, kind, ts, title, body FROM imported WHERE uid = ?1
             ORDER BY rowid DESC LIMIT 1",
            [uid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((source, kind, ts, title, body)) = row else {
        return Ok(None);
    };
    if kind == "prompt" || raw.forgotten(uid)? {
        return Ok(None);
    }
    let (title, first) = (title.trim(), crate::cards::first_sentence(body.trim()));
    let stem = title.trim_end_matches("...");
    let words = if first.starts_with(stem) {
        first
    } else if stem.starts_with(&first) {
        title.to_owned()
    } else {
        format!("{title}: {first}")
    };
    if words.trim().is_empty() {
        return Ok(None);
    }
    let what = match source == crate::raw::NOTE_SOURCE {
        true => "note".to_owned(),
        false => format!("imported {kind} (status unknown)"),
    };
    Ok(Some((ts, what, words)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source_id: &str, title: &str, body: &str) -> ImportDoc {
        ImportDoc {
            uid: format!("claude-mem:abc:{source_id}"),
            source: "claude-mem:abc".into(),
            source_id: source_id.into(),
            kind: "decision".into(),
            repo: "claude-mem:free-mem".into(),
            session: "s1".into(),
            ts: 1_000,
            title: title.into(),
            body: body.into(),
        }
    }

    fn count(k: &Connection, table: &str) -> i64 {
        k.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn imported_fts_finds_an_imported_title() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![
            doc("o1", "Chose SQLite", "Because it is one file."),
            doc("o2", "Tabs", "Indent with tabs."),
        ])
        .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 2);
        let uid: String = k
            .query_row(
                "SELECT i.uid FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'SQLite'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, "claude-mem:abc:o1");
    }

    /// D5: a restore that lost ops takes their documents out with them, and only theirs. The ops
    /// that take their seqs read in again under the freed rowids, which the contentless index
    /// takes back (OpenCodeReview on #305).
    #[test]
    fn a_rewind_removes_the_rows_of_the_lost_ops_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![doc("o1", "Kept", "One.")]).unwrap();
        raw.append_imports(vec![doc("o2", "Lost", "Two.")]).unwrap();
        raw.append_imports(vec![doc("o3", "Lost", "Three.")])
            .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 3);
        Imported.rewind(&k, &device, 1).unwrap();
        assert_eq!((count(&k, "imported"), count(&k, "imported_fts")), (1, 1));
        let left: String = k
            .query_row(
                "SELECT i.title FROM imported_fts f JOIN imported i ON i.rowid = f.rowid",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, "Kept");
        assert_eq!(Imported.step(&raw, &k, &device, 1).unwrap(), 3);
        let found: Vec<String> = k
            .prepare(
                "SELECT i.source_id FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'Three' ORDER BY i.rowid",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(found, ["o3"]);
        k.execute(
            "INSERT INTO imported_fts(imported_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }

    /// #506: how a prompt's block names an imported document, by its newest row: an observation
    /// by its title and its body's first sentence, labelled imported with its status unknown; a
    /// summary by its body's first sentence (its request); a note saved without a title by its
    /// text's start once. A prompt, and a forgotten document, take no place.
    #[test]
    fn a_prompt_names_an_imported_document_by_its_newest_row() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let kind = |id: &str, kind: &str, title: &str, body: &str| ImportDoc {
            kind: kind.into(),
            ..doc(id, title, body)
        };
        let untitled = "Use tabs in Makefiles. Make needs them.";
        raw.append_imports(vec![
            doc("o1", "Old title", "Old text."),
            doc(
                "o1",
                "Gateway retries",
                "Payments retry on a timeout.\n- three times",
            ),
            kind(
                "s1",
                "summary",
                "",
                "Request: Fix the retry.\nLearned: It was the clock.",
            ),
            kind("p1", "prompt", "", "Why does the payment fail?"),
            ImportDoc {
                source: crate::raw::NOTE_SOURCE.into(),
                ..kind("n1", "note", untitled, untitled)
            },
            doc("o2", "Forgotten", "Gone."),
        ])
        .unwrap();
        raw.append_ops(&[(
            OpKind::Forget,
            serde_json::json!({"uid": "claude-mem:abc:o2"}),
        )])
        .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        Imported.step(&raw, &k, &device, 0).unwrap();
        let line = |uid: &str| prompt_line(&k, &raw, uid).unwrap();
        let at = |what: &str, words: &str| Some((1_000, what.to_owned(), words.to_owned()));
        assert_eq!(
            line("claude-mem:abc:o1"),
            at(
                "imported decision (status unknown)",
                "Gateway retries: Payments retry on a timeout."
            )
        );
        assert_eq!(
            line("claude-mem:abc:s1"),
            at(
                "imported summary (status unknown)",
                "Request: Fix the retry."
            )
        );
        assert_eq!(line("claude-mem:abc:n1"), at("note", untitled));
        assert_eq!(line("claude-mem:abc:p1"), None);
        assert_eq!(line("claude-mem:abc:o2"), None);
        assert_eq!(line("claude-mem:abc:none"), None);
    }
}
