//! Milestone 4 Task 3 (D5): the imported documents consumer. It reads the op log of every device
//! that has ops and keeps each `import` op's document in knowledge.db, keyed by the op that brought
//! it, with a trigram index over its title and body. Curation reads records, never these ops; the
//! per-prompt block (#506, `prompt_line`) and the note on a file (docs/file-note.md F13, by the
//! files `ImportedFiles` keeps) are what inject them.

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
         -- Decision 52: the sessions and projects of the documents filed under a checkout's
         -- claude-mem names, read without case as claude-mem reads them.
         CREATE INDEX IF NOT EXISTS imported_project ON imported(repo COLLATE NOCASE, session, kind);
         CREATE VIRTUAL TABLE IF NOT EXISTS imported_fts
           USING fts5(text, tokenize='trigram', content='', contentless_delete=1);",
    )?;
    Ok(())
}

/// N5 (docs/memory-lookup.md): every column the trigram side of a search reads of an imported
/// match (`search::b::imported_fts`), so its join reads this narrow index and not the documents.
/// The consumer makes it, never a reader: a hook's search can be ended mid-build. On this PC's
/// store (263,208 documents, 2026-10-20) about 31 MB, made in 0.6 s with the documents cached.
pub(crate) const COVER: &str = "imported_cover";

fn cover(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE INDEX IF NOT EXISTS imported_cover ON imported(rowid, repo, ts, kind, session, uid);
         -- N5: a timeline's documents of a repository in time order (`search::b::imported_side`).
         CREATE INDEX IF NOT EXISTS imported_time ON imported(repo, ts, uid);
         -- Every repository's, newest first, read to the page's end (`search::b::imported_side`).
         CREATE INDEX IF NOT EXISTS imported_newest ON imported(ts, uid);",
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
        cover(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in ops.into_iter().filter(|o| o.kind == OpKind::Import) {
            // A document's files are no document: `ImportedFiles` keeps them (docs/file-note.md
            // F10, F12).
            if crate::raw::is_import_files(&op.body) {
                continue;
            }
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

/// docs/file-note.md F12: the files imported observations name, from their files ops, for the
/// note on a file. Its own checkpoint: a rollback's older binary passes over files ops, and it
/// must not move this one past an op it never read.
pub struct ImportedFiles;

/// One row per path of each list of a files op, with the number of paths the op names in all
/// (F6), so the note's lookup counts nothing; a uid's files are its newest op's (the highest
/// rowid), as its text is its newest document row's.
pub fn files_schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS imported_files(
           rowid INTEGER PRIMARY KEY, op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
           uid TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL, modified INTEGER NOT NULL,
           named INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS imported_files_path ON imported_files(name, path);
         CREATE INDEX IF NOT EXISTS imported_files_uid ON imported_files(uid);",
    )?;
    Ok(())
}

impl Consumer for ImportedFiles {
    fn name(&self) -> &'static str {
        "imported_files"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        files_schema(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        let mut st = k.prepare_cached(
            "INSERT INTO imported_files(op_device, op_seq, uid, name, path, modified, named)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for op in ops.into_iter().filter(|o| o.kind == OpKind::Import) {
            if !crate::raw::is_import_files(&op.body) {
                continue;
            }
            // An op this version cannot read stays in raw, unread.
            let Ok(f) = serde_json::from_value::<crate::raw::ImportFiles>(op.body) else {
                continue;
            };
            let named = (f.files_read.len() + f.files_modified.len()) as i64;
            for (modified, list) in [(false, &f.files_read), (true, &f.files_modified)] {
                for path in list {
                    let path = crate::cards::slashed(path);
                    let name = crate::cards::file_name(&path).to_owned();
                    st.execute(params![
                        device, op.op_seq, f.uid, name, path, modified, named
                    ])?;
                }
            }
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        files_schema(k)?;
        k.execute(
            "DELETE FROM imported_files WHERE op_device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}

/// #506 (docs/prompt-context.md, the second slice): whether imported document `uid` belongs to
/// `repo`'s prompts, by its newest row: a document of a session that started in `repo`
/// (`session_start`, oboete's first record of it), or an agent's note saved in `repo`, which has no
/// session of an agent's; and since decision 52, one filed under one of the checkout's claude-mem
/// names (`import::names_of`), without case. Never by the repository a document is filed under
/// otherwise (v1's documents are, and map through their sessions).
pub(crate) fn maps(k: &Connection, uid: &str, repo: &str, names: &[String]) -> Result<bool> {
    Ok(k.prepare_cached(
        "SELECT 1 FROM imported i
         WHERE i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = ?1)
           AND (i.session = ?3 AND i.repo = ?2
             OR i.repo COLLATE NOCASE IN (SELECT value FROM json_each(?4))
             OR EXISTS (
               SELECT 1 FROM session_start s WHERE s.session = i.session AND s.repo = ?2))",
    )?
    .exists(params![
        uid,
        repo,
        crate::raw::NOTE_SESSION,
        crate::import::name_labels(names)
    ])?)
}

/// #506 (spec 4.5, 4.12): imported document `uid` as a prompt's block names it, by its newest
/// row, when it may take a place there: its time, what it is (`note` for an agent's note, else
/// `imported <kind> (status unknown)`) and its words, its title and its text (the longer alone when
/// one starts with the other, as a note saved without a title is titled with its text's start,
/// `mcp::untitled`; a title that only begins a word of the text is kept). Its text is an
/// observation's whole body, its narrative and then its facts, and a note's whole text, as
/// claude-mem's block shows an observation's narrative (parity row 31); a summary's is its first
/// sentence, its request, as a session summary of oboete's is named. None for a prompt
/// (a prompt's text is never injected, 4.6), a forgotten document (milestone 5 D5), or one with no
/// words.
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
    let note = source == crate::raw::NOTE_SOURCE;
    let body = body.trim();
    // An observation's body is its narrative, then each fact on a line of its own
    // (`import::observation_body`), which no mark tells apart from a list in the narrative: the
    // body is shown whole, the line cut at `CLIP` (CodeRabbit on #571).
    let text = match kind.as_str() {
        "summary" => crate::cards::first_sentence(body),
        _ => body.to_owned(),
    };
    let title = title.trim();
    let words = if crate::cards::begins_with_title(&text, title) {
        text
    } else if title.trim_end_matches("...").starts_with(&text) {
        title.to_owned()
    } else {
        format!("{title}: {text}")
    };
    if words.trim().is_empty() {
        return Ok(None);
    }
    let what = match note {
        true => "note".to_owned(),
        false => format!("imported {kind} (status unknown)"),
    };
    Ok(Some((ts, what, words)))
}

#[cfg(test)]
mod tests;
