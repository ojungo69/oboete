//! `oboete import claude-mem <db>`: claude-mem's observations, session summaries and prompts as
//! `import` ops in raw.db (milestone 4 D5; docs/pr-b.md, decision 2). Text passes the outbound gate
//! on the way in, prompts go through the hook's own cleaning, and the source ids already imported
//! are read first, so a second run, or one after a run that stopped, adds nothing twice.
//! Repositories become `claude-mem:<project>`: claude-mem names a project, not a path, and a
//! repository reads the project its key ends in (`imported_repos`). Its work state comes in too
//! (docs/claude-mem-import.md).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::raw::{ImportDoc, ImportFiles, Raw};
use crate::{curate, hook, redact};

const SOURCE: &str = "claude-mem";

#[derive(Default, Serialize)]
pub struct Stats {
    pub observations: u64,
    pub summaries: u64,
    pub prompts: u64,
    /// Rows imported by an earlier run.
    pub seen: u64,
    /// Rows with nothing left to store (empty, or a harness notification).
    pub empty: u64,
    /// Work state rows imported as work state ops (docs/claude-mem-import.md I4).
    pub work_state: u64,
    /// Work state rows the checks a write passes refused.
    pub refused: u64,
    /// Work state rows of a project whose repository is never recorded (slice 4a, I6).
    pub never_recorded: u64,
    /// claude-mem's exclusion patterns added as patterns never recorded (parity row 14, I8).
    pub exclusions: u64,
    /// claude-mem's exclusion patterns oboete does not take (over 256 bytes, or with a control
    /// character): the folders they name are recorded, and the import says so.
    pub exclusions_refused: u64,
    /// Observations whose files came in as a files op (docs/file-note.md F10, F11).
    pub files: u64,
    /// Paths left out of a files op to keep it within `FILES_BYTES` (F10). An observation none of
    /// whose paths fits gets no files op, so each run tries it, and counts them, again.
    pub files_cut: u64,
}

/// The most a files op takes as stored (docs/file-note.md F10): half the op cap.
const FILES_BYTES: usize = crate::raw::MAX_OP_BYTES / 2;

struct Session {
    id: String,
    project: String,
}

/// One import at a time on a home (Codex on #305): an import reads the source ids already
/// imported before it appends, so two at once would both append what neither had seen. Held
/// while the returned file is.
pub fn lock(home: &Path) -> Result<std::fs::File> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("import.lock"))?;
    match crate::worker::try_lock(&f) {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!(
                "another oboete import is running on this home: run it again when it ends"
            )
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The newest claude-mem schema the import reads all of: 13.40.0's (docs/claude-mem-import.md I2).
/// v65 (13.36.0) only recomputes `observations.title_norm_key`; v66 adds cloud sync columns to
/// `work_state_entries` and v67 lets the sync ledgers name that kind (13.40.0). The import reads
/// none of them.
const NEWEST_SCHEMA: i64 = 67;

/// The plain import, as the tests run it.
#[cfg(test)]
pub fn claude_mem(
    raw: &mut Raw,
    path: &Path,
    settings: &crate::capture::Settings,
) -> Result<Stats> {
    claude_mem_with(raw, path, settings, false)
}

/// `settings` gates the work state rows as a write's (docs/work-state.md L5). With `files_only`
/// (`--files`, docs/file-note.md F11), the files ops of the observations imported before and
/// nothing else: no document, work state or exclusion.
pub fn claude_mem_with(
    raw: &mut Raw,
    path: &Path,
    settings: &crate::capture::Settings,
    files_only: bool,
) -> Result<Stats> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {} read-only", path.display()))?;
    // One read transaction: a consistent snapshot while claude-mem keeps writing.
    src.execute_batch("BEGIN")?;
    if let Some(version) = schema_version(&src)?
        && version > NEWEST_SCHEMA
    {
        anyhow::bail!(
            "this claude-mem database is at schema v{version}, newer than v{NEWEST_SCHEMA}, the \
             newest this oboete imports: update oboete first, so nothing claude-mem keeps is left \
             behind"
        );
    }
    let source = source_name(&src)?;
    let (added, refused) = match files_only {
        true => (0, 0),
        false => exclusions(raw, path)?,
    };
    let mut stats = Stats {
        exclusions: added,
        exclusions_refused: refused,
        ..Stats::default()
    };
    // A row claude-mem merged into another project is read under that one (I3).
    let project = |table: &str| -> Result<&'static str> {
        Ok(if has_column(&src, table, "merged_into_project")? {
            "COALESCE(NULLIF(merged_into_project, ''), project, '')"
        } else {
            "COALESCE(project, '')"
        })
    };
    let (observed, summarized) = (project("observations")?, project("session_summaries")?);
    let notes = if has_column(&src, "session_summaries", "notes")? {
        "COALESCE(notes, '')"
    } else {
        "''"
    };
    // F10: the files an observation names, where claude-mem keeps them.
    let file_column = |col: &str| -> Result<String> {
        Ok(match has_column(&src, "observations", col)? {
            true => format!("COALESCE({col}, '')"),
            false => "''".to_owned(),
        })
    };
    let (read, modified) = (file_column("files_read")?, file_column("files_modified")?);
    let mut sink = Sink {
        known: raw.import_repos(&source)?,
        known_files: raw.import_files_repos(&source)?,
        raw,
        source: &source,
        files_only,
        items: Vec::new(),
        files: 0,
    };

    // Observations and summaries name claude-mem's own session id; prompts name the agent's.
    // oboete keys sessions by the agent's id, so the same session captured by both lines up.
    let mut by_memory: HashMap<String, Session> = HashMap::new();
    let mut by_content: HashMap<String, Session> = HashMap::new();
    each_row(
        &src,
        "SELECT memory_session_id, content_session_id, COALESCE(project, '') FROM sdk_sessions",
        |r| {
            let memory: Option<String> = r.get(0)?;
            let content: String = r.get(1)?;
            let session = || -> rusqlite::Result<Session> {
                Ok(Session {
                    id: content.clone(),
                    project: r.get(2)?,
                })
            };
            if let Some(m) = memory {
                by_memory.insert(m, session()?);
            }
            by_content.insert(content.clone(), session()?);
            Ok(())
        },
    )?;

    each_row(
        &src,
        &format!(
            "SELECT id, COALESCE(memory_session_id, ''), {observed}, created_at_epoch,
                    COALESCE(type, ''), COALESCE(title, ''), COALESCE(narrative, ''),
                    COALESCE(facts, ''), {read}, {modified} FROM observations ORDER BY id"
        ),
        |r| {
            let (id, memory, project, ts): (i64, String, String, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let files = [
                file_list(&r.get::<_, String>(8)?, &settings.rules),
                file_list(&r.get::<_, String>(9)?, &settings.rules),
            ];
            // The import's rules throughout, as the writes owe the op text scan by them (owner
            // decision 43; Codex's security review of the switch).
            let title = redact::outbound_with(&r.get::<_, String>(5)?, &settings.rules);
            let body = redact::outbound_with(
                &observation_body(&r.get::<_, String>(6)?, &r.get::<_, String>(7)?),
                &settings.rules,
            );
            if title.is_empty() && body.is_empty() {
                stats.empty += 1;
                return Ok(());
            }
            let row = Row {
                key: format!("o{id}"),
                session: by_memory.get(&memory),
                session_id: &memory,
                project: &project,
                ts,
                kind: kind(&r.get::<_, String>(4)?),
                title: &title,
                body: &body,
                files,
            };
            let put = sink.put(row)?;
            stats.files_cut += put.cut;
            count(&put, &mut stats.observations, &mut stats.seen);
            Ok(())
        },
    )?;
    if files_only {
        sink.flush()?;
        stats.files = sink.files;
        return Ok(stats);
    }

    each_row(
        &src,
        &format!(
            "SELECT id, COALESCE(memory_session_id, ''), {summarized}, created_at_epoch,
                    COALESCE(request, ''), COALESCE(investigated, ''), COALESCE(learned, ''),
                    COALESCE(completed, ''), COALESCE(next_steps, ''), {notes}
             FROM session_summaries ORDER BY id"
        ),
        |r| {
            let (id, memory, project, ts): (i64, String, String, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let mut parts = Vec::new();
            for (i, label) in [
                "Request",
                "Investigated",
                "Learned",
                "Completed",
                "Next steps",
                "Notes",
            ]
            .iter()
            .enumerate()
            {
                let text: String = r.get(4 + i)?;
                if !text.trim().is_empty() {
                    parts.push(format!("{label}: {}", text.trim()));
                }
            }
            let body = redact::outbound_with(&parts.join("\n"), &settings.rules);
            if body.is_empty() {
                stats.empty += 1;
                return Ok(());
            }
            let row = Row {
                key: format!("s{id}"),
                session: by_memory.get(&memory),
                session_id: &memory,
                project: &project,
                ts,
                kind: "summary",
                title: "",
                body: &body,
                files: Default::default(),
            };
            count(&sink.put(row)?, &mut stats.summaries, &mut stats.seen);
            Ok(())
        },
    )?;

    each_row(
        &src,
        "SELECT id, COALESCE(content_session_id, ''), created_at_epoch, COALESCE(prompt_text, '')
         FROM user_prompts ORDER BY id",
        |r| {
            let (id, content, ts, text): (i64, String, i64, String) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            // The same cleaning a live prompt gets: blocks out, secrets masked, notifications dropped.
            let body = hook::clip(&hook::strip_blocks(&text, true), &settings.rules);
            if body.is_empty() || hook::is_envelope(&body) {
                stats.empty += 1;
                return Ok(());
            }
            let known = by_content.get(&content);
            let project = known.map(|s| s.project.clone()).unwrap_or_default();
            let row = Row {
                key: format!("p{id}"),
                session: known,
                session_id: &content,
                project: &project,
                ts,
                kind: "prompt",
                title: "",
                body: &body,
                files: Default::default(),
            };
            count(&sink.put(row)?, &mut stats.prompts, &mut stats.seen);
            Ok(())
        },
    )?;
    sink.flush()?;
    stats.files = sink.files;
    if has_table(&src, "work_state_entries")? {
        work_state(raw, &src, &source, settings, &mut stats)?;
    }
    Ok(stats)
}

/// I8 (parity row 14): claude-mem's exclusion patterns, `CLAUDE_MEM_EXCLUDED_PROJECTS` in the
/// `settings.json` beside its database, each added as a pattern never recorded unless an exclusion
/// op ever named it, so one taken back here is not added again; and how many oboete does not take,
/// which a line names, since the folders they name are recorded (the commit's security review).
/// None when the file or the key is missing; a file that is there but cannot be read as
/// claude-mem's settings stops the import before any write, for the same reason (Codex on #563).
fn exclusions(raw: &mut Raw, db: &Path) -> Result<(u64, u64)> {
    let path = db.with_file_name("settings.json");
    // A failure names the file, its line and column at most: never what it holds.
    let stop = |why: String| {
        anyhow::anyhow!(
            "{} {why}, so the folders claude-mem's exclusion patterns name would be recorded: \
             nothing is imported; fix it, or move it away to import without them",
            path.display()
        )
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(stop(format!("cannot be read ({e})"))),
    };
    // As claude-mem reads it (settings-document.ts): a leading BOM skipped, and the keys under
    // `env` when that holds claude-mem's keys.
    let doc: serde_json::Value =
        serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text))
            .map_err(|e| stop(format!("is not JSON ({e})")))?;
    let Some(root) = doc.as_object() else {
        return Err(stop("is not a JSON object".into()));
    };
    let target = match root.get("env").and_then(|env| env.as_object()) {
        Some(env) if env.keys().any(|k| k.starts_with("CLAUDE_MEM_")) => env,
        _ => root,
    };
    let list = match target.get("CLAUDE_MEM_EXCLUDED_PROJECTS") {
        None | Some(serde_json::Value::Null) => return Ok((0, 0)),
        Some(serde_json::Value::String(list)) => list,
        Some(_) => {
            return Err(stop(
                "holds CLAUDE_MEM_EXCLUDED_PROJECTS as something other than one string".into(),
            ));
        }
    };
    let mut named = raw.patterns_ever_named()?;
    let (mut added, mut refused) = (0, 0);
    // An empty entry is none, as claude-mem reads the list.
    for given in list.split(',').filter(|g| !g.trim().is_empty()) {
        let Ok(pattern) = crate::capture::pattern(given) else {
            refused += 1;
            continue;
        };
        // A pattern the list repeats comes in once (Codex on #563).
        if named.insert(pattern.clone()) {
            raw.exclude_capture(("pattern", &pattern), false)?;
            added += 1;
        }
    }
    if refused > 0 {
        eprintln!(
            "oboete: {refused} of claude-mem's exclusion patterns are not brought over (over 256 \
             bytes, or with a control character), so the folders they name are recorded: add \
             each with `oboete capture exclude --pattern` in a form of at most 256 bytes"
        );
    }
    Ok((added, refused))
}

/// I4: claude-mem's work state rows as work state ops, oldest first, under their project as an
/// import names it and at their own time, each checked and gated as a write is (docs/work-state.md
/// L3, L5): a row the checks refuse is counted, not imported.
fn work_state(
    raw: &mut Raw,
    src: &Connection,
    source: &str,
    settings: &crate::capture::Settings,
    stats: &mut Stats,
) -> Result<()> {
    let known = raw.work_state_keys(source)?;
    // A list excluded after this read is refused by the append, which reads the list again and
    // labels the project's name as this read does.
    let never = raw.capture_exclusions()?;
    let mut ops = Vec::new();
    each_row(
        src,
        "SELECT id, COALESCE(project, ''), COALESCE(list_name, ''), COALESCE(fields, ''),
                created_at_epoch FROM work_state_entries ORDER BY id",
        |r| {
            let key = format!("w{}", r.get::<_, i64>(0)?);
            if known.contains(&key) {
                stats.seen += 1;
                return Ok(());
            }
            let (project, list, fields, at): (String, String, String, i64) =
                (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?);
            // Also as the list's labels hold it: a rule may mask part of the project's name.
            let named = repo(&project);
            let labelled = crate::capture::exclusion_label("repo", &named, settings);
            if never.covers_work_state(&named, true) || never.covers_work_state(&labelled, true) {
                stats.never_recorded += 1;
                return Ok(());
            }
            let checked =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&fields)
                    .ok()
                    .and_then(|sent| {
                        let list = crate::work_state::check(&list, &sent).ok()?;
                        let (list, fields) = crate::capture::work_state(&list, &sent, settings);
                        let list = crate::work_state::stored(&list, &sent, &fields).ok()?;
                        Some((list, fields))
                    });
            let Some((list, fields)) = checked else {
                stats.refused += 1;
                return Ok(());
            };
            ops.push((
                crate::raw::OpKind::WorkState,
                serde_json::json!({"repo": repo(&project), "list": list, "fields": fields,
                    "clock": at, "at": at, "source": source, "source_id": key}),
            ));
            stats.work_state += 1;
            Ok(())
        },
    )?;
    // A run that stops keeps the batches it appended; the next adds the rest. A batch keeps to the
    // count and the size an append takes, as the documents' do: a rule's masks can lengthen
    // fields past the size they were checked at (CodeRabbit on #431).
    let (mut batch, mut bytes) = (Vec::new(), 0);
    for op in ops {
        let size = op.1.to_string().len();
        if batch.len() == crate::raw::IMPORT_BATCH || bytes + size > crate::raw::MAX_BATCH_BYTES {
            append_work_state(raw, &std::mem::take(&mut batch), settings, stats)?;
            bytes = 0;
        }
        bytes += size;
        batch.push(op);
    }
    if !batch.is_empty() {
        append_work_state(raw, &batch, settings, stats)?;
    }
    Ok(())
}

/// One batch of imported work state ops. A repository excluded after this run read the list
/// refuses its rows in the append (`NeverRecorded`): the batch then goes in one op at a time, and
/// each op refused is counted as never recorded rather than imported (Codex on #489).
fn append_work_state(
    raw: &mut Raw,
    batch: &[(crate::raw::OpKind, serde_json::Value)],
    settings: &crate::capture::Settings,
    stats: &mut Stats,
) -> Result<()> {
    let never = |e: &anyhow::Error| e.is::<crate::raw::NeverRecorded>();
    match raw.append_imported_ops(batch, settings) {
        Err(e) if never(&e) => {}
        other => return other.map(drop),
    }
    for op in batch {
        match raw.append_imported_ops(std::slice::from_ref(op), settings) {
            Err(e) if never(&e) => {
                stats.work_state -= 1;
                stats.never_recorded += 1;
            }
            other => other.map(drop)?,
        }
    }
    Ok(())
}

/// The newest version in claude-mem's migration log, if it keeps one.
fn schema_version(src: &Connection) -> Result<Option<i64>> {
    if !has_table(src, "schema_versions")? {
        return Ok(None);
    }
    Ok(src.query_row("SELECT MAX(version) FROM schema_versions", [], |r| r.get(0))?)
}

fn has_table(src: &Connection, table: &str) -> Result<bool> {
    Ok(src.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |r| r.get(0),
    )?)
}

/// Whether claude-mem's `table` has `column`: read from the table, not its version number (I2).
fn has_column(src: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(src.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |r| r.get(0),
    )?)
}

/// Calls `each` with every row `sql` selects from `src`, in order.
fn each_row(
    src: &Connection,
    sql: &str,
    mut each: impl FnMut(&rusqlite::Row) -> Result<()>,
) -> Result<()> {
    let mut stmt = src.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        each(r)?;
    }
    Ok(())
}

/// claude-mem's ids restart in every database (the Windows copy and the WSL one both have an
/// observation 1), so the import key names the database by when it was created: the first row of
/// its migration log, which pruning never touches and a backup or a move keeps. Importing a
/// snapshot and later the live file then adds only what is new.
fn source_name(src: &Connection) -> Result<String> {
    let created: Option<String> = src
        .query_row(
            "SELECT applied_at FROM schema_versions ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .or_else(|e| match e {
            // A database without the log (not one claude-mem wrote) gets the bare name.
            rusqlite::Error::SqliteFailure(_, Some(m)) if m.contains("no such table") => Ok(None),
            e => Err(e),
        })?;
    Ok(match created {
        Some(created) => {
            let hash = Sha256::digest(created.as_bytes());
            let hex: String = hash[..6].iter().map(|b| format!("{b:02x}")).collect();
            format!("{SOURCE}:{hex}")
        }
        None => SOURCE.to_string(),
    })
}

/// One claude-mem row on its way in.
struct Row<'a> {
    key: String,
    /// Its session in `sdk_sessions`, if claude-mem still lists it.
    session: Option<&'a Session>,
    /// The session id the row names (possibly empty).
    session_id: &'a str,
    project: &'a str,
    ts: i64,
    kind: &'a str,
    title: &'a str,
    body: &'a str,
    /// The files it read and modified, gated (docs/file-note.md F10): an observation's only.
    files: [Vec<String>; 2],
}

/// Where rows go: import ops, appended every `raw::IMPORT_BATCH` ops, each append its own
/// transaction, so a run that stops keeps what it appended.
struct Sink<'a> {
    raw: &'a mut Raw,
    source: &'a str,
    /// The source ids imported so far, by earlier runs and this one, each with the repositories
    /// it was filed under.
    known: HashMap<String, HashSet<String>>,
    /// The same for the documents' files ops (F11).
    known_files: HashMap<String, HashSet<String>>,
    /// `--files`: the files ops of the documents imported before, and no document (F11).
    files_only: bool,
    items: Vec<crate::raw::Import>,
    /// Files ops put, none of a uid the append leaves out (`Raw::import_refused`).
    files: u64,
}

/// What `Sink::put` did with a row: whether its document is new, whether an earlier run imported
/// it, and how many of its paths its files op left out (F10).
struct Put {
    new: bool,
    before: bool,
    cut: u64,
}

impl Sink<'_> {
    /// A row as an import op, with its session, and its files as a files op (docs/file-note.md
    /// F10, F11): the files op of a document filed before comes now if it has none under the
    /// repository the document is filed under. A row claude-mem moved since (I3) comes in again
    /// with the same uid: readers take a document's newest row (Codex on #431). `--files` moves
    /// none, so such a row's files op is filed where its document is.
    fn put(&mut self, r: Row) -> Result<Put> {
        let repo = repo(r.project);
        let had = self.known.get(&r.key);
        let filed = match had {
            // One whose project is lost (claude-mem pruned its session) stays where it came in.
            Some(had) if r.project.is_empty() => had.iter().min().cloned(),
            Some(had) if had.contains(&repo) => Some(repo.clone()),
            _ => None,
        };
        let before = had.is_some();
        let session = match r.session {
            Some(s) => s.id.clone(),
            // No session id at all: a session of its own, so unrelated rows do not merge.
            None if r.session_id.is_empty() => format!("{}/{}", self.source, r.key),
            // A session claude-mem no longer lists keeps its id.
            None => r.session_id.to_owned(),
        };
        let uid = format!("{}:{}", self.source, r.key);
        let new = filed.is_none() && !self.files_only;
        let under = match filed {
            Some(at) => Some(at),
            None if new => Some(repo.clone()),
            // Moved since, under `--files`: where it was filed, as a lost project's.
            None => had.and_then(|had| had.iter().min().cloned()),
        };
        if new {
            self.known
                .entry(r.key.clone())
                .or_default()
                .insert(repo.clone());
            self.items.push(crate::raw::Import::Doc(ImportDoc {
                uid: uid.clone(),
                source: self.source.to_owned(),
                source_id: r.key.clone(),
                kind: r.kind.to_owned(),
                repo: repo.clone(),
                session: session.clone(),
                ts: r.ts,
                title: r.title.to_owned(),
                body: r.body.to_owned(),
            }));
        }
        let mut cut = 0;
        let [read, modified] = r.files;
        if let Some(under) = under
            && !(read.is_empty() && modified.is_empty())
            && !self
                .known_files
                .get(&r.key)
                .is_some_and(|had| had.contains(&under))
        {
            let (files, left_out) = fitted(ImportFiles {
                uid,
                source: self.source.to_owned(),
                source_id: r.key.clone(),
                repo: under.clone(),
                session,
                files_read: read,
                files_modified: modified,
            })?;
            // None of a uid the append leaves out (D5), which it asks again: `files` counts what
            // is stored, and a forgotten uid's is not put again at every run.
            let refused = self.raw.import_refused(
                (&files.source, &files.source_id),
                &files.uid,
                &files.text(),
            )?;
            if !refused {
                cut = left_out;
                if !(files.files_read.is_empty() && files.files_modified.is_empty()) {
                    self.known_files.entry(r.key).or_default().insert(under);
                    self.items.push(crate::raw::Import::Files(files));
                    self.files += 1;
                }
            }
        }
        if self.items.len() >= crate::raw::IMPORT_BATCH {
            self.flush()?;
        }
        Ok(Put { new, before, cut })
    }

    fn flush(&mut self) -> Result<()> {
        self.raw
            .append_import_items(std::mem::take(&mut self.items), |_| {})?;
        Ok(())
    }
}

/// docs/file-note.md F10: a list of files as claude-mem's lookup reads one, a JSON array's
/// strings (anything else is none), each trimmed, the empty left out, gated.
fn file_list(text: &str, rules: &redact::Rules) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::trim))
            .filter(|p| !p.is_empty())
            .map(|p| redact::outbound_with(p, rules))
            .collect(),
        _ => Vec::new(),
    }
}

/// F10: `files` with its paths kept in order, read first, while its op as stored stays within
/// `FILES_BYTES` (each path as JSON writes it, escapes included), and how many were left out.
fn fitted(mut files: ImportFiles) -> Result<(ImportFiles, u64)> {
    let lists = [
        std::mem::take(&mut files.files_read),
        std::mem::take(&mut files.files_modified),
    ];
    let mut size = serde_json::to_string(&files)?.len();
    let (mut full, mut cut) = (false, 0);
    for (modified, list) in lists.into_iter().enumerate() {
        for path in list {
            // The path as JSON writes it, and a comma.
            let more = serde_json::to_string(&path)?.len() + 1;
            full |= size + more > FILES_BYTES;
            if full {
                cut += 1;
                continue;
            }
            size += more;
            match modified {
                0 => files.files_read.push(path),
                _ => files.files_modified.push(path),
            }
        }
    }
    Ok((files, cut))
}

/// claude-mem's `project` as its documents' repository.
pub(crate) fn repo(project: &str) -> String {
    format!("{SOURCE}:{project}")
}

/// What claude-mem names a directory nothing names (`projectNameFromSource`).
const UNKNOWN_PROJECT: &str = "unknown-project";

/// The longest claude-mem name a prompt sends the viewer (decision 52): a path's, which a
/// submodule's or a worktree's name joins with its parent's.
pub(crate) const NAME_BYTES: usize = 4_096;

/// claude-mem's project-root markers outside git (`PROJECT_ROOT_MARKERS`).
const PROJECT_MARKERS: [&str; 2] = [".claude-mem-project", ".claude-mem.json"];

/// Spec decision 52 (docs/prompt-context.md, "Decision 52"): claude-mem's names for the checkout
/// at `cwd`, as 14.0.1 reads them by path (`getPathProjectContext`'s `allProjects`,
/// src/utils/project-name.ts), the one it writes under last: the git top level's folder name, a
/// linked worktree's or a submodule's with its parent's, a marker root's, or the folder's own.
pub(crate) fn names_of(cwd: &Path) -> Vec<String> {
    // `path.resolve`: absolute, `.` and `..` folded, links kept (git's top is the real path's).
    let cwd = &lexical(&std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf()));
    let real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let top = crate::repo::top(&real);
    let marker = top.is_none().then(|| marker_root(cwd)).flatten();
    let name = folder_name(top.as_deref().or(marker.as_deref()).unwrap_or(cwd), cwd);
    let root = top.as_deref().unwrap_or(cwd);
    if let Some((parent, submodule)) = git_parent(root)
        && let Some(parent_name) = parent.file_name().map(|n| n.to_string_lossy().into_owned())
    {
        let (primary, alias) = if submodule {
            // Its path under the superproject (`submoduleLeaf`), else its own name.
            let leaf = root
                .strip_prefix(&parent)
                .ok()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| name.clone());
            // Its own name too, which the loop below drops when it is the superproject's.
            (format!("{parent_name}/{leaf}"), Some(name.clone()))
        } else if name == parent_name {
            // `buildWorktreeProjectKey`: a worktree named as its repository writes to it, and
            // the doubled key it wrote to before stays readable.
            (parent_name.clone(), Some(format!("{parent_name}/{name}")))
        } else {
            (format!("{parent_name}/{name}"), None)
        };
        let mut names = Vec::new();
        for n in [Some(parent_name), alias].into_iter().flatten() {
            if n != primary && !names.contains(&n) {
                names.push(n);
            }
        }
        names.push(primary);
        return names;
    }
    // A marker re-keys the directories below its root; the folder's own name stays readable (at
    // the root it is the root's).
    let own = folder_name(cwd, cwd);
    if marker.is_some() && own != name && own != UNKNOWN_PROJECT {
        return vec![own, name];
    }
    vec![name]
}

/// `path.basename` of what names the project, as `projectNameFromSource` takes it: a drive's
/// root is `drive-<letter>` on Windows (by `cwd`), and any other root `unknown-project`.
fn folder_name(source: &Path, cwd: &Path) -> String {
    match source.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => {
            let cwd = cwd.to_string_lossy();
            let drive = cwd.as_bytes();
            match cfg!(windows) && drive.len() >= 3 && drive[1] == b':' && drive[2] == b'\\' {
                true if drive[0].is_ascii_alphabetic() => {
                    format!("drive-{}", (drive[0] as char).to_ascii_uppercase())
                }
                _ => UNKNOWN_PROJECT.to_owned(),
            }
        }
    }
}

/// claude-mem's `detectWorktree` (src/utils/worktree.ts): a `.git` file of one `gitdir:` line, the
/// path resolved against `root` without following links, naming `<parent>/.git/worktrees/<name>`
/// (a linked worktree) or `<parent>/.git/modules/...` (a submodule, `true`): the parent.
fn git_parent(root: &Path) -> Option<(std::path::PathBuf, bool)> {
    let git = root.join(".git");
    if !std::fs::metadata(&git).ok()?.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&git).ok()?;
    let gitdir = text.trim().strip_prefix("gitdir:")?.trim_start();
    if gitdir.is_empty() || gitdir.contains(['\n', '\r']) {
        return None;
    }
    let path = lexical(&root.join(gitdir));
    let parts: Vec<_> = path.components().collect();
    let n = parts.len();
    let is = |i: usize, s: &str| parts[i].as_os_str() == s;
    // `^(.+)/.git/worktrees/<name>$`, then `^(.+)/.git/modules/(.+)$` (its last such `.git`).
    let (at, submodule) = if n >= 4 && is(n - 3, ".git") && is(n - 2, "worktrees") {
        (n - 3, false)
    } else {
        (
            (1..n.saturating_sub(2))
                .rev()
                .find(|&i| is(i, ".git") && is(i + 1, "modules"))?,
            true,
        )
    };
    (at >= 2).then(|| (parts[..at].iter().collect(), submodule))
}

/// claude-mem's `findMarkerProjectRoot`: the nearest ancestor of `cwd` holding a marker, the walk
/// stopping before the home directory, the temporary directory, Claude's config directory and the
/// root, and never starting inside the config directory.
fn marker_root(cwd: &Path) -> Option<std::path::PathBuf> {
    let both = |d: std::path::PathBuf| {
        let d = lexical(&d);
        let real = d.canonicalize().unwrap_or_else(|_| d.clone());
        [d, real]
    };
    let config = both(crate::setup::claude_dir());
    let stops: Vec<_> = [crate::config::home_dir(), std::env::temp_dir()]
        .into_iter()
        .flat_map(both)
        .chain(config.clone())
        .collect();
    marker_walk(cwd, &config, &stops)
}

/// `marker_root` with its stops given: none inside `config`, none at or above a stop.
fn marker_walk(
    cwd: &Path,
    config: &[std::path::PathBuf],
    stops: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    let mut current = lexical(cwd);
    if config.iter().any(|c| current.starts_with(c)) {
        return None;
    }
    for _ in 0..64 {
        if stops.contains(&current) {
            return None;
        }
        let parent = current.parent()?.to_path_buf();
        if PROJECT_MARKERS.iter().any(|m| current.join(m).exists()) {
            return Some(current);
        }
        current = parent;
    }
    None
}

/// `path.resolve` of an absolute path: `.` and `..` folded without the filesystem.
fn lexical(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// Decision 52: the repositories, as imported documents are filed (`repo`), of claude-mem's names
/// `names`: a JSON array for `json_each`, compared with `COLLATE NOCASE` as claude-mem compares.
pub(crate) fn name_labels(names: &[String]) -> String {
    serde_json::Value::from(names.iter().map(|n| repo(n)).collect::<Vec<_>>()).to_string()
}

/// The repositories whose imported documents and work state are `repo`'s: claude-mem names a
/// project, not a repository, and files it as `claude-mem:<project>` (`repo`), so a repository's
/// are those of the project its key ends in, and a claude-mem name's those of its own project (a
/// worktree's `<project>/<worktree>` too, as the index files them). ponytail: by name; a mapping by
/// checkout waits for two repositories that share a last name.
pub(crate) fn imported_repos(key: &str) -> [String; 2] {
    let project = match key.strip_prefix("claude-mem:") {
        Some(name) => name.split('/').next().unwrap_or(name),
        None => key.rsplit('/').next().unwrap_or(key),
    };
    [key.to_owned(), repo(project)]
}

/// `imported_repos(key)`, and the prefix of its claude-mem project's worktrees.
pub(crate) fn imported_scope(key: &str) -> ([String; 2], String) {
    let [own, named] = imported_repos(key);
    let worktrees = format!("{named}/");
    ([own, named], worktrees)
}

/// SQL over `col` for `imported_scope(key)`, with the four values it takes.
pub(crate) fn imported_match(col: &str, key: &str) -> (String, [rusqlite::types::Value; 4]) {
    let ([own, named], worktrees) = imported_scope(key);
    (
        format!("({col} IN (?, ?) OR substr({col}, 1, length(?)) = ?)"),
        [own, named, worktrees.clone(), worktrees].map(rusqlite::types::Value::Text),
    )
}

/// claude-mem's types are oboete's kinds, plus a few it wrote by mistake (`discovery>`) or keeps
/// for itself (`security_note`); those count as discoveries.
fn kind(t: &str) -> &'static str {
    curate::KINDS
        .iter()
        .find(|k| **k == t)
        .copied()
        .unwrap_or("discovery")
}

/// The narrative, then each fact on its own line. `facts` is a JSON array of strings.
fn observation_body(narrative: &str, facts: &str) -> String {
    let facts: Vec<String> = serde_json::from_str(facts).unwrap_or_default();
    let mut lines = vec![narrative.trim().to_string()];
    lines.extend(facts.iter().map(|f| format!("- {}", f.trim())));
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// A row in the import's line: added, imported by an earlier run, or neither (one claude-mem
/// recorded since, which `--files` leaves to the plain import, F11).
fn count(put: &Put, added: &mut u64, seen: &mut u64) {
    if put.new {
        *added += 1;
    } else if put.before {
        *seen += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::OpKind;
    use rusqlite::params;
    use serde_json::json;

    /// The columns of claude-mem's tables that the import reads.
    /// `created` is the database's first migration time, so two values stand for two databases.
    fn claude_mem_db(path: &Path, created: &str) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE sdk_sessions(id INTEGER PRIMARY KEY, content_session_id TEXT NOT NULL,
               memory_session_id TEXT, project TEXT, platform_source TEXT,
               started_at_epoch INTEGER, completed_at_epoch INTEGER);
             CREATE TABLE observations(id INTEGER PRIMARY KEY, memory_session_id TEXT, project TEXT,
               created_at_epoch INTEGER, type TEXT, title TEXT, narrative TEXT, facts TEXT);
             CREATE TABLE session_summaries(id INTEGER PRIMARY KEY, memory_session_id TEXT,
               project TEXT, created_at_epoch INTEGER, request TEXT, investigated TEXT,
               learned TEXT, completed TEXT, next_steps TEXT);
             CREATE TABLE user_prompts(id INTEGER PRIMARY KEY, content_session_id TEXT,
               created_at_epoch INTEGER, prompt_text TEXT);
             ",
        )
        .unwrap();
        c.execute_batch(
            "CREATE TABLE schema_versions(id INTEGER PRIMARY KEY, version INTEGER, applied_at TEXT);
             INSERT INTO sdk_sessions VALUES(1, 'agent-1', 'mem-1', 'free-mem', 'codex', 1000, 2000);",
        )
        .unwrap();
        c.execute("INSERT INTO schema_versions VALUES(1, 4, ?1)", [created])
            .unwrap();
        // Two rows without any session id: unrelated, so they must not share a session.
        for id in [15, 16] {
            c.execute(
                "INSERT INTO observations VALUES(?1, NULL, 'free-mem', 1600, 'change', 'Loose', 'No session.', '')",
                [id],
            )
            .unwrap();
        }
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let obs: [(i64, &str, &str, &str, &str, &str); 5] = [
            (
                10,
                "mem-1",
                "decision",
                "Chose SQLite",
                "Because it is one file.",
                "[\"No server\"]",
            ),
            (
                11,
                "mem-1",
                "discovery>",
                "Broken type",
                "",
                "[\"Fact only\"]",
            ),
            (12, "mem-1", "security_note", "", "", "[]"),
            (
                13,
                "mem-1",
                "change",
                "Token",
                "key <private>client name</private> ok",
                "[]",
            ),
            (
                14,
                "gone",
                "bugfix",
                "Orphan",
                "Its session row was deleted.",
                "",
            ),
        ];
        for (id, session, kind, title, narrative, facts) in obs {
            let narrative = narrative.replace("key", &format!("key {key}"));
            c.execute(
                "INSERT INTO observations VALUES(?1, ?2, 'free-mem', 1500, ?3, ?4, ?5, ?6)",
                params![id, session, kind, title, narrative, facts],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO session_summaries VALUES(20, 'mem-1', 'free-mem', 1900, 'Fix search', '', 'FTS5 trigram', 'Shipped', NULL)",
            [],
        )
        .unwrap();
        for (id, text) in [
            (
                30,
                "How do I add <private>my name</private> semantic search?",
            ),
            (
                31,
                "<task-notification>\n<summary>done</summary>\n</task-notification>",
            ),
        ] {
            c.execute(
                "INSERT INTO user_prompts VALUES(?1, 'agent-1', 1100, ?2)",
                params![id, text],
            )
            .unwrap();
        }
    }

    /// `claude_mem_db` as 13.35.0 keeps it (docs/claude-mem-import.md I2-I4): v64 in its log,
    /// summaries' notes, a worktree's observation merged into its project, and work state rows,
    /// two of which no write would take.
    fn claude_mem_v64(path: &Path, created: &str) {
        claude_mem_db(path, created);
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "ALTER TABLE observations ADD COLUMN merged_into_project TEXT;
             ALTER TABLE session_summaries ADD COLUMN merged_into_project TEXT;
             ALTER TABLE session_summaries ADD COLUMN notes TEXT;
             INSERT INTO schema_versions VALUES(2, 64, '2026-10-09T00:00:00.000Z');
             CREATE TABLE work_state_entries(id INTEGER PRIMARY KEY AUTOINCREMENT,
               project TEXT NOT NULL, list_name TEXT NOT NULL, fields TEXT NOT NULL,
               created_at TEXT NOT NULL, created_at_epoch INTEGER NOT NULL);
             UPDATE session_summaries SET notes = 'Kept the index small.' WHERE id = 20;
             INSERT INTO observations(id, memory_session_id, project, created_at_epoch, type,
               title, narrative, facts, merged_into_project)
               VALUES(40, 'mem-1', 'free-mem/wt', 1700, 'change', 'In a worktree', 'Merged.',
               '[]', 'free-mem');",
        )
        .unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let rows = [
            (
                "free-mem",
                "plan",
                r#"{"task": "Port search", "status": "doing"}"#.to_owned(),
                3_000,
            ),
            (
                "free-mem/wt",
                "plan",
                format!(r#"{{"note": "token {key}"}}"#),
                3_100,
            ),
            ("free-mem", "", r#"{"task": "No list"}"#.to_owned(), 3_200),
            ("free-mem", "plan", "not fields".to_owned(), 3_300),
        ];
        for (project, list, fields, at) in rows {
            c.execute(
                "INSERT INTO work_state_entries(project, list_name, fields, created_at,
                   created_at_epoch) VALUES(?1, ?2, ?3, '', ?4)",
                params![project, list, fields, at],
            )
            .unwrap();
        }
    }

    /// The work state ops raw holds, in op order.
    fn work_ops(raw: &Raw) -> Vec<serde_json::Value> {
        raw.ops_after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::WorkState)
            .map(|o| o.body)
            .collect()
    }

    /// The documents' import ops raw holds, in op order.
    fn docs(raw: &Raw) -> Vec<ImportDoc> {
        raw.ops_after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::Import && !crate::raw::is_import_files(&o.body))
            .map(|o| serde_json::from_value(o.body).unwrap())
            .collect()
    }

    /// The files ops raw holds (docs/file-note.md F10), in op order.
    fn files_ops(raw: &Raw) -> Vec<ImportFiles> {
        raw.ops_after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::Import && crate::raw::is_import_files(&o.body))
            .map(|o| serde_json::from_value(o.body).unwrap())
            .collect()
    }

    /// `claude_mem_db` with claude-mem's lists of files, and `lists` (observation id, read,
    /// modified) in them.
    fn with_files(path: &Path, lists: &[(i64, serde_json::Value, serde_json::Value)]) {
        let c = Connection::open(path).unwrap();
        let has: bool = c
            .query_row(
                "SELECT count(*) > 0 FROM pragma_table_info('observations') WHERE name = 'files_read'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if !has {
            c.execute_batch(
                "ALTER TABLE observations ADD COLUMN files_read TEXT;
                 ALTER TABLE observations ADD COLUMN files_modified TEXT;",
            )
            .unwrap();
        }
        for (id, read, modified) in lists {
            c.execute(
                "UPDATE observations SET files_read = ?2, files_modified = ?3 WHERE id = ?1",
                params![id, read.to_string(), modified.to_string()],
            )
            .unwrap();
        }
    }

    /// docs/file-note.md F10 (test 15): an observation that names files gets a files op after its
    /// document's, its paths gated and kept within `FILES_BYTES` as stored; one whose lists are
    /// none, or whose only path does not fit, gets none.
    #[test]
    fn an_observations_files_come_in_as_a_files_op_beside_its_document() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        // Paths JSON writes longer than they are: a quote, a backslash and a control character.
        let odd: Vec<String> = (0..2_000).map(|i| format!("a\"\\\u{1}{i}.rs")).collect();
        with_files(
            &src,
            &[
                (
                    10,
                    json!(["src/a.rs", " ", 7, format!("{key}/b.rs")]),
                    json!(["src/a.rs"]),
                ),
                (13, json!("src/c.rs"), json!({"x": 1})),
                (14, json!(odd), json!(["src/d.rs"])),
                (15, json!(["x".repeat(FILES_BYTES)]), json!([])),
                // Kept while they fit: a short path after one that did not fit is left out too.
                (
                    16,
                    json!(["y".repeat(FILES_BYTES / 2), "z".repeat(FILES_BYTES)]),
                    json!(["m.rs"]),
                ),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        let files = files_ops(&raw);
        assert_eq!(
            files
                .iter()
                .map(|f| f.source_id.as_str())
                .collect::<Vec<_>>(),
            ["o10", "o14", "o16"]
        );
        let a = &files[0];
        assert_eq!(a.files_read[0], "src/a.rs");
        assert_eq!(
            a.files_read.len(),
            2,
            "the blank and the number are no paths"
        );
        assert!(a.files_read[1].ends_with("/b.rs") && !a.files_read[1].contains(&key));
        assert_eq!(a.files_modified, ["src/a.rs"]);
        let doc = docs(&raw)
            .into_iter()
            .find(|d| d.source_id == "o10")
            .unwrap();
        assert_eq!(
            (&a.uid, &a.repo, &a.session),
            (&doc.uid, &doc.repo, &doc.session)
        );
        // The cut: kept in order, read first, within the bytes as stored.
        let b = &files[1];
        let stored = serde_json::to_string(b).unwrap().len();
        assert!(stored <= FILES_BYTES, "{stored}");
        assert!(b.files_read.len() < odd.len() && b.files_modified.is_empty());
        assert_eq!(b.files_read[..], odd[..b.files_read.len()]);
        let short = &files[2];
        assert_eq!((short.files_read.len(), short.files_modified.len()), (1, 0));
        let cut = (odd.len() - b.files_read.len() + 1) as u64;
        assert_eq!((stats.files, stats.files_cut), (3, cut + 1 + 2));
        // In op order, each after its document, and none reads as a document.
        let ops = raw.ops_after(raw.device(), 0, 10_000).unwrap();
        let at = |id: &str, files: bool| {
            ops.iter()
                .position(|o| {
                    o.body["source_id"] == id && crate::raw::is_import_files(&o.body) == files
                })
                .unwrap()
        };
        assert!(at("o10", false) < at("o10", true) && at("o14", false) < at("o14", true));
        for o in ops.iter().filter(|o| crate::raw::is_import_files(&o.body)) {
            assert!(serde_json::from_value::<ImportDoc>(o.body.clone()).is_err());
            assert!(o.body.get("body").is_none() && o.body.get("ts").is_none());
        }
    }

    /// F11 (test 16): `--files` brings the files of the observations imported before and nothing
    /// else, once, none of a forgotten or denied uid; a plain import brings what an earlier run
    /// left out, and a document claude-mem moved gets its files under its new repository.
    #[test]
    fn the_files_of_observations_imported_before_come_in_once() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let homes = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let mut raws: Vec<Raw> = homes
            .iter()
            .map(|h| crate::raw::open(h.path()).unwrap())
            .collect();
        // Imported before claude-mem's files came in, as this PC's store was.
        for raw in &mut raws {
            claude_mem(raw, &src, &Default::default()).unwrap();
            assert!(files_ops(raw).is_empty());
        }
        with_files(
            &src,
            &[
                (10, json!(["src/a.rs"]), json!([])),
                (11, json!(["src/b.rs"]), json!([])),
                (13, json!(["src/c.rs"]), json!([])),
                (14, json!([]), json!(["src/d.rs"])),
            ],
        );
        let c = Connection::open(&src).unwrap();
        c.execute(
            "INSERT INTO observations VALUES(40, 'mem-1', 'free-mem', 1700, 'bugfix', 'New',
               'Since the last import.', '', '[\"src/e.rs\"]', '[]')",
            [],
        )
        .unwrap();
        // Work state claude-mem kept since, which `--files` leaves to the plain import too.
        c.execute_batch(
            "CREATE TABLE work_state_entries(id INTEGER PRIMARY KEY AUTOINCREMENT,
               project TEXT NOT NULL, list_name TEXT NOT NULL, fields TEXT NOT NULL,
               created_at TEXT NOT NULL, created_at_epoch INTEGER NOT NULL);
             INSERT INTO work_state_entries(project, list_name, fields, created_at,
               created_at_epoch) VALUES('free-mem', 'plan', '{\"task\": \"Port search\"}', '', 3000);",
        )
        .unwrap();
        let ids = |raw: &Raw| -> Vec<String> {
            files_ops(raw).into_iter().map(|f| f.source_id).collect()
        };
        let [plain, only] = &mut raws[..] else {
            unreachable!()
        };
        // A forgotten document gets no files op, nor one whose origin is denied.
        let forgotten = docs(only)
            .into_iter()
            .find(|d| d.source_id == "o13")
            .unwrap();
        only.append_ops(&[(OpKind::Forget, json!({"uid": forgotten.uid, "job": "j"}))])
            .unwrap();
        Connection::open(homes[1].path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO denied_records(origin, device, seq, ts, session, job)
                 VALUES(?1, 'd', 1, 0, 's', 'j')",
                [crate::forget::origin(&forgotten.source, "o11")],
            )
            .unwrap();
        let before = docs(only).len();
        let stats = claude_mem_with(only, &src, &Default::default(), true).unwrap();
        assert_eq!(ids(only), ["o10", "o14"]);
        assert_eq!(docs(only).len(), before, "no document");
        assert_eq!((stats.observations, stats.work_state), (0, 0));
        // Six imported before (o12 is empty); o40, recorded since, is left to the plain import.
        assert_eq!((counts(&stats), stats.files), ((0, 0, 0, 6, 1), 2));
        let again = claude_mem_with(only, &src, &Default::default(), true).unwrap();
        assert_eq!((ids(only).len(), again.files), (2, 0), "once");
        let plain_run = claude_mem(only, &src, &Default::default()).unwrap();
        assert_eq!(
            ids(only),
            ["o10", "o14", "o40"],
            "the new one with its document"
        );
        assert_eq!((plain_run.files, plain_run.work_state), (1, 1));
        // The plain import brings what the first run, before the lists, could not.
        claude_mem(plain, &src, &Default::default()).unwrap();
        assert_eq!(ids(plain), ["o10", "o11", "o13", "o14", "o40"]);
        claude_mem(plain, &src, &Default::default()).unwrap();
        assert_eq!(ids(plain).len(), 5);
        // Moved: its document and its files again, under the new project; `--files`, which
        // moves no document, has its files op already where the document is filed.
        c.execute(
            "UPDATE observations SET project = 'other' WHERE id = 10",
            [],
        )
        .unwrap();
        claude_mem_with(plain, &src, &Default::default(), true).unwrap();
        assert_eq!(ids(plain).len(), 5);
        claude_mem(plain, &src, &Default::default()).unwrap();
        let moved: Vec<String> = files_ops(plain)
            .into_iter()
            .filter(|f| f.source_id == "o10")
            .map(|f| f.repo)
            .collect();
        assert_eq!(moved, [repo("free-mem"), repo("other")]);
    }

    /// F11 (test 16): a document imported before claude-mem's files came in, and moved to another
    /// project since, gets its files op from `--files` where it is filed, once; the plain import
    /// then moves it and files its files again under the new project.
    #[test]
    fn files_only_files_a_moved_documents_files_where_it_is_filed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        with_files(&src, &[(10, json!(["src/a.rs"]), json!([]))]);
        Connection::open(&src)
            .unwrap()
            .execute(
                "UPDATE observations SET project = 'other' WHERE id = 10",
                [],
            )
            .unwrap();
        let repos =
            |raw: &Raw| -> Vec<String> { files_ops(raw).into_iter().map(|f| f.repo).collect() };
        let stats = claude_mem_with(&mut raw, &src, &Default::default(), true).unwrap();
        assert_eq!(repos(&raw), [repo("free-mem")]);
        assert_eq!((stats.files, stats.seen), (1, 6), "imported before: seen");
        claude_mem_with(&mut raw, &src, &Default::default(), true).unwrap();
        assert_eq!(repos(&raw), [repo("free-mem")], "once");
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(repos(&raw), [repo("free-mem"), repo("other")]);
    }

    fn counts(s: &Stats) -> (u64, u64, u64, u64, u64) {
        (s.observations, s.summaries, s.prompts, s.seen, s.empty)
    }

    #[test]
    fn claude_mem_rows_arrive_gated_and_mapped() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(counts(&stats), (6, 1, 1, 0, 2));
        let docs = docs(&raw);
        let at = |id: &str| docs.iter().find(|d| d.source_id == id).unwrap();
        let o10 = at("o10");
        assert_eq!(
            (
                o10.session.as_str(),
                o10.repo.as_str(),
                o10.kind.as_str(),
                o10.title.as_str(),
                o10.body.as_str(),
            ),
            (
                "agent-1",
                "claude-mem:free-mem",
                "decision",
                "Chose SQLite",
                "Because it is one file.\n- No server",
            )
        );
        assert_eq!(
            (at("o11").kind.as_str(), at("o11").body.as_str()),
            ("discovery", "- Fact only")
        );
        let gated = &at("o13").body;
        assert!(
            !gated.contains("ghp_") && !gated.contains("client name"),
            "{gated}"
        );
        assert!(gated.contains("[REDACTED]"));
        assert_eq!(
            (at("o14").session.as_str(), at("o14").title.as_str()),
            ("gone", "Orphan")
        );
        // Rows without any session id are sessions of their own.
        assert_eq!(at("o15").session, "claude-mem:b62f07076e19/o15");
        assert_eq!(at("o16").session, "claude-mem:b62f07076e19/o16");
        assert_eq!(
            at("s20").body,
            "Request: Fix search\nLearned: FTS5 trigram\nCompleted: Shipped"
        );
        let prompt = &at("p30").body;
        assert!(!prompt.contains("my name") && prompt.starts_with("How do I add"));
        // The imported documents are searchable once the worker has read them in.
        drop(raw);
        crate::worker::run_once(dir.path()).unwrap();
        let k = crate::knowledge::open(dir.path()).unwrap();
        let found: Vec<String> = k
            .prepare(
                "SELECT i.uid FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'trigram'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(found, ["claude-mem:b62f07076e19:s20"]);
    }

    /// docs/claude-mem-import.md tests 1, 3 and 4: 13.35.0's database brings its summaries'
    /// notes, a merged worktree's rows under their project, and its work state, gated, oldest
    /// first, at their own times; the rows no write would take are counted; a second run adds
    /// nothing.
    #[test]
    fn a_v64_database_brings_notes_merged_rows_and_work_state() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!((stats.work_state, stats.refused), (2, 2));
        let docs = docs(&raw);
        let at = |id: &str| docs.iter().find(|d| d.source_id == id).unwrap();
        assert!(
            at("s20").body.ends_with("\nNotes: Kept the index small."),
            "{}",
            at("s20").body
        );
        assert_eq!(at("o40").repo, "claude-mem:free-mem");
        let ops = work_ops(&raw);
        assert_eq!(ops.len(), 2);
        assert_eq!(
            (
                &ops[0]["repo"],
                &ops[0]["clock"],
                &ops[0]["at"],
                &ops[0]["list"]
            ),
            (
                &json!("claude-mem:free-mem"),
                &json!(3_000),
                &json!(3_000),
                &json!("plan")
            )
        );
        assert_eq!(ops[1]["repo"], "claude-mem:free-mem/wt");
        let note = ops[1]["fields"]["note"].as_str().unwrap();
        assert!(
            note.starts_with("token ") && !note.contains("q9Zx8"),
            "{note}"
        );
        let again = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!((again.work_state, again.refused), (0, 2));
        assert_eq!(work_ops(&raw).len(), 2);
    }

    /// I6, slice 4a (Codex on #489): a project whose repository is never recorded brings no work
    /// state, its worktree's neither, by the name search reads them under (D13); its rows are
    /// counted, and an import after the undo brings them.
    #[test]
    fn a_repository_never_recorded_keeps_its_imported_work_state_out() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        raw.exclude_capture(("repo", "github.com/o/free-mem"), false)
            .unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(
            (stats.work_state, stats.refused, stats.never_recorded),
            (0, 0, 4)
        );
        assert!(work_ops(&raw).is_empty());
        raw.exclude_capture(("repo", "github.com/o/free-mem"), true)
            .unwrap();
        let after = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!((after.work_state, after.never_recorded), (2, 0));
    }

    /// Owner decision 43 (Codex's security review of the switch): the import gates every field
    /// with its own settings' rules, which its writes owe the op text scan by: a rule they hold
    /// masks a title (egress, read per row, would not here), and with redaction off a work state
    /// row comes in as typed and the scan recorded before is owed again.
    #[test]
    fn the_import_gates_with_its_settings_and_owes_the_scan_when_they_are_off() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        Connection::open(&src)
            .unwrap()
            .execute(
                "INSERT INTO observations(id, memory_session_id, project, created_at_epoch, type,
                   title, narrative, facts)
                 VALUES(41, 'mem-1', 'free-mem', 1800, 'change', 'Deploy acme-482915', 'Done.',
                   '[]')",
                [],
            )
            .unwrap();
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        let ruled_home = tempfile::tempdir().unwrap();
        std::fs::write(ruled_home.path().join("config.toml"), rule).unwrap();
        let ruled = crate::capture::Settings::load(ruled_home.path()).unwrap();
        let mut raw = crate::raw::open(ruled_home.path()).unwrap();
        claude_mem(&mut raw, &src, &ruled).unwrap();
        let titles: Vec<String> = docs(&raw).into_iter().map(|d| d.title).collect();
        assert!(
            titles.iter().any(|t| t == "Deploy [REDACTED]"),
            "{titles:?}"
        );
        let off_home = tempfile::tempdir().unwrap();
        std::fs::write(
            off_home.path().join("config.toml"),
            "[redaction]\nenabled = false\n",
        )
        .unwrap();
        let off = crate::capture::Settings::load(off_home.path()).unwrap();
        let mut raw = crate::raw::open(off_home.path()).unwrap();
        let mark = raw.op_text_scan_begun().unwrap();
        assert!(raw.ops_text_scanned("v", &mark).unwrap());
        raw.gates_with(&off.rules);
        claude_mem(&mut raw, &src, &off).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        assert!(work_ops(&raw).iter().any(|o| o.to_string().contains(&key)));
        assert!(raw.ops_text_owed("v").unwrap());
    }

    /// Codex on #489: a repository excluded after the import read the list refuses its rows in
    /// the append; the batch's other rows still come in, and the refused ones are counted.
    #[test]
    fn an_exclusion_made_during_the_import_leaves_out_only_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let op = |project: &str, id: &str| {
            (
                OpKind::WorkState,
                json!({"repo": repo(project), "list": "plan",
                    "fields": {"task": "Port search", "status": "doing"}, "clock": 1_000,
                    "at": 1_000, "source": "claude-mem:b62f07076e19", "source_id": id}),
            )
        };
        let batch = [op("free-mem", "w1"), op("other", "w2")];
        // After the import's read of the list, before its append.
        raw.exclude_capture(("repo", "github.com/o/free-mem"), false)
            .unwrap();
        let mut stats = Stats {
            work_state: 2,
            ..Default::default()
        };
        append_work_state(&mut raw, &batch, &Default::default(), &mut stats).unwrap();
        assert_eq!((stats.work_state, stats.never_recorded), (1, 1));
        let ops = work_ops(&raw);
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert_eq!(ops[0]["source_id"], "w2");
    }

    /// Parity row 14 (CodeRabbit on #563): a pattern that names a claude-mem project keeps the
    /// project's imported work state out, as a repository never recorded does.
    #[test]
    fn a_pattern_keeps_a_projects_imported_work_state_out() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        raw.exclude_capture(("pattern", "free-*"), false).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!((stats.work_state, stats.never_recorded), (0, 4));
        assert!(work_ops(&raw).is_empty());
    }

    /// Codex on #489: a repository excluded under a rule that masks part of its name keeps its
    /// claude-mem project's work state out too: the project's name is matched as labelled now.
    #[test]
    fn a_masked_repository_name_keeps_its_imported_work_state_out() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let rule = "[[redaction.extra_rules]]\nid = \"fm\"\nregex = 'free-mem'\n";
        std::fs::write(dir.path().join("config.toml"), rule).unwrap();
        let settings = crate::capture::Settings::load(dir.path()).unwrap();
        let label = crate::capture::exclusion_label("repo", "github.com/o/free-mem", &settings);
        assert!(!label.contains("free-mem"), "{label}");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        raw.exclude_capture(("repo", &label), false).unwrap();
        let stats = claude_mem(&mut raw, &src, &settings).unwrap();
        assert_eq!((stats.work_state, stats.never_recorded), (0, 4));
        assert!(work_ops(&raw).is_empty());
    }

    /// Codex on #431: a row claude-mem merges into another project after it came in comes in
    /// again under that project, with the same uid, and the index files it there (readers take a
    /// document's newest row); a run after that adds nothing.
    #[test]
    fn a_row_merged_after_its_import_comes_in_again_under_its_project() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let merge = |into: Option<&str>| {
            Connection::open(&src)
                .unwrap()
                .execute(
                    "UPDATE observations SET merged_into_project = ?1 WHERE id = 40",
                    [into],
                )
                .unwrap();
        };
        merge(None);
        let mut raw = crate::raw::open(dir.path()).unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        let o40 = |raw: &Raw| -> Vec<String> {
            docs(raw)
                .into_iter()
                .filter(|d| d.source_id == "o40")
                .map(|d| d.repo)
                .collect()
        };
        assert_eq!(o40(&raw), ["claude-mem:free-mem/wt"]);
        merge(Some("free-mem"));
        let moved = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(moved.observations, 1);
        assert_eq!(o40(&raw), ["claude-mem:free-mem/wt", "claude-mem:free-mem"]);
        let again = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(again.observations, 0);
        assert_eq!(o40(&raw).len(), 2);
        drop(raw);
        crate::worker::run_once(dir.path()).unwrap();
        let k = crate::knowledge::open(dir.path()).unwrap();
        let uid: String = k
            .query_row(
                "SELECT uid FROM imported WHERE source_id = 'o40' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let line = crate::search::b::get(dir.path(), &uid).unwrap().unwrap();
        assert!(line.contains(" claude-mem:free-mem (imported from"));
        // The row's first import also on another device, at a higher op seq than this device's
        // (a copied store): op seqs order one device's ops only, and the row is filed under the
        // merged project already, so nothing more comes in (Codex on #431).
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let first = raw
            .ops_after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .find(|o| o.kind == OpKind::Import && o.body["source_id"] == "o40")
            .unwrap();
        Connection::open(dir.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES('elsewhere', 1000000, 'import', 0, ?1, 1000000)",
                [first.body.to_string()],
            )
            .unwrap();
        let copied = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(copied.observations, 0);
    }

    /// Test 1: a database at a schema newer than the import knows is refused, nothing written.
    #[test]
    fn a_database_newer_than_v67_is_refused_with_nothing_written() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        Connection::open(&src)
            .unwrap()
            .execute_batch(
                "INSERT INTO schema_versions VALUES(3, 65, '');
                 INSERT INTO schema_versions VALUES(4, 66, '');
                 INSERT INTO schema_versions VALUES(5, 67, '');
                 INSERT INTO schema_versions VALUES(6, 68, '');",
            )
            .unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let said = claude_mem(&mut raw, &src, &Default::default())
            .err()
            .unwrap()
            .to_string();
        assert!(said.contains("v68") && said.contains("v67"), "{said}");
        assert!(docs(&raw).is_empty() && work_ops(&raw).is_empty());
    }

    /// I2: 13.36.0's v65, which only recomputes `observations.title_norm_key`, and 13.40.0's v67,
    /// which adds the work state's sync columns, come in as v64 does.
    #[test]
    fn a_v65_or_v67_database_comes_in_as_v64_does() {
        let dir = tempfile::tempdir().unwrap();
        let (v64, v65, v67) = (
            dir.path().join("v64.db"),
            dir.path().join("v65.db"),
            dir.path().join("v67.db"),
        );
        for db in [&v64, &v65, &v67] {
            claude_mem_v64(db, "2025-12-14T16:09:58.769Z");
        }
        let v65_batch = "ALTER TABLE observations ADD COLUMN title_norm_key TEXT;
             UPDATE observations SET title_norm_key = lower(title);
             INSERT INTO schema_versions VALUES(3, 65, '2026-10-13T00:22:16.544Z');";
        Connection::open(&v65)
            .unwrap()
            .execute_batch(v65_batch)
            .unwrap();
        let v67_conn = Connection::open(&v67).unwrap();
        v67_conn.execute_batch(v65_batch).unwrap();
        v67_conn
            .execute_batch(
                "ALTER TABLE work_state_entries ADD COLUMN synced_at INTEGER;
                 ALTER TABLE work_state_entries ADD COLUMN origin_device_id TEXT;
                 ALTER TABLE work_state_entries ADD COLUMN origin_local_id TEXT;
                 ALTER TABLE work_state_entries ADD COLUMN sync_rev TEXT NOT NULL DEFAULT '1';
                 INSERT INTO schema_versions VALUES(4, 66, '2026-10-16T14:58:42.000Z');
                 INSERT INTO schema_versions VALUES(5, 67, '2026-10-16T14:58:42.000Z');",
            )
            .unwrap();
        drop(v67_conn);
        let import = |src: &Path, home: &Path| {
            std::fs::create_dir(home).unwrap();
            let mut raw = crate::raw::open(home).unwrap();
            let stats = claude_mem(&mut raw, src, &Default::default()).unwrap();
            let docs: Vec<_> = docs(&raw)
                .into_iter()
                .map(|d| (d.source_id, d.repo, d.body))
                .collect();
            (
                stats.observations,
                stats.summaries,
                stats.prompts,
                stats.work_state,
                docs,
            )
        };
        let (a, b, c) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("c"),
        );
        let want = import(&v64, &a);
        assert_eq!(import(&v65, &b), want);
        assert_eq!(import(&v67, &c), want);
    }

    /// Test 2: what is read is found by its columns, whatever version the log names: an older
    /// database with notes, and a v61 row with no work state table (another draft's v61).
    #[test]
    fn columns_are_found_whatever_version_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        Connection::open(&src)
            .unwrap()
            .execute_batch(
                "ALTER TABLE session_summaries ADD COLUMN notes TEXT;
                 UPDATE session_summaries SET notes = 'Found by its column.';
                 INSERT INTO schema_versions VALUES(2, 61, '');",
            )
            .unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(stats.work_state, 0);
        let docs = docs(&raw);
        let summary = docs.iter().find(|d| d.source_id == "s20").unwrap();
        assert!(summary.body.ends_with("\nNotes: Found by its column."));
    }

    /// I5 (test 5): a repository reads its project's imported lists, its worktree project's too,
    /// with its own writes, each at its own time whichever reached the store first: a write made
    /// here before the import, later than every imported entry, still comes last.
    #[test]
    fn a_repository_reads_its_projects_imported_work_state_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let repo = "github.com/o/free-mem";
        let done = json!({"task": "Port search", "status": "done"});
        raw.work_state(repo, "plan", done.as_object().unwrap(), None)
            .unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        raw.append_ops(&[(
            OpKind::WorkState,
            json!({"repo": "claude-mem:free-memory", "list": "plan",
                "fields": {"task": "Not this project's"}, "clock": 1, "at": 1,
                "source": "claude-mem:other", "source_id": "w1"}),
        )])
        .unwrap();
        let entries = raw.work_state_entries(repo).unwrap();
        let fields: Vec<&serde_json::Map<String, serde_json::Value>> =
            entries.iter().map(|e| &e.fields).collect();
        assert_eq!(fields.len(), 3, "{fields:?}");
        assert_eq!(fields[0]["status"], "doing");
        assert_eq!(entries[0].ts, 3_000);
        assert!(fields[1].contains_key("note"));
        assert_eq!(entries[1].ts, 3_100);
        assert_eq!(fields[2]["status"], "done");
    }

    /// CodeRabbit on #431: work state rows go in batches within the size an append takes as well
    /// as the count, so rows a rule makes long (every `x` a mask) still all come in.
    #[test]
    fn work_state_rows_a_rule_lengthens_come_in_within_the_batch_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        let c = Connection::open(&src).unwrap();
        let note = format!(r#"{{"note": "{}"}}"#, vec!["x"; 990].join(" "));
        for i in 0..500 {
            c.execute(
                "INSERT INTO work_state_entries(project, list_name, fields, created_at,
                   created_at_epoch) VALUES('free-mem', 'long', ?1, '', ?2)",
                params![note, 10_000 + i],
            )
            .unwrap();
        }
        drop(c);
        let rule = "[[redaction.extra_rules]]\nid = \"x\"\nregex = 'x'\n";
        std::fs::write(dir.path().join("config.toml"), rule).unwrap();
        let settings = crate::capture::Settings::load(dir.path()).unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let stats = claude_mem(&mut raw, &src, &settings).unwrap();
        assert_eq!((stats.work_state, stats.refused), (502, 2));
        let ops = work_ops(&raw);
        let bytes: usize = ops.iter().map(|o| o.to_string().len()).sum();
        assert!(bytes > crate::raw::MAX_BATCH_BYTES, "{bytes}");
    }

    /// Codex on #431: an imported entry dated ahead of this device's clock lifts no clock of the
    /// writes made here, and takes effect no later than its import, so a write made here after it
    /// still comes last; it shows its own time.
    #[test]
    fn an_imported_entry_ahead_of_this_clock_lifts_no_later_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let ahead = crate::db::now_ms() + 86_400_000;
        raw.append_ops(&[(
            OpKind::WorkState,
            json!({"repo": "claude-mem:free-mem", "list": "plan",
                "fields": {"task": "Port search", "status": "doing"}, "clock": ahead,
                "at": ahead, "source": "claude-mem:b62f07076e19", "source_id": "w1"}),
        )])
        .unwrap();
        let repo = "github.com/o/free-mem";
        let done = json!({"task": "Port search", "status": "done"});
        raw.work_state(repo, "plan", done.as_object().unwrap(), None)
            .unwrap();
        let ops = work_ops(&raw);
        assert!(ops[1]["clock"].as_i64().unwrap() < ahead, "{ops:?}");
        let entries = raw.work_state_entries(repo).unwrap();
        assert_eq!(entries[0].ts, ahead);
        assert_eq!(entries[1].fields["status"], "done", "{entries:?}");
    }

    /// The security review of the N1 commit: the name rule reads imported entries only, as
    /// search's reads imported documents only. A native entry is read by its own key alone, also
    /// when an origin gives that key an imported project's shape (`ssh://claude-mem:<name>/x`
    /// keeps its non-numeric "port" in the host).
    #[test]
    fn a_native_entry_under_a_claude_mem_name_is_read_by_its_own_key_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let crafted = crate::repo::normalize("ssh://claude-mem:free-mem/x").unwrap();
        assert_eq!(crafted, "claude-mem:free-mem/x");
        let task = |t: &str| json!({"task": t, "status": "todo"});
        raw.work_state(
            &crafted,
            "plan",
            task("Written here").as_object().unwrap(),
            None,
        )
        .unwrap();
        raw.append_ops(&[(
            OpKind::WorkState,
            json!({"repo": crafted, "list": "plan", "fields": task("Imported"), "clock": 1,
                "at": 1, "source": "claude-mem:b62f07076e19", "source_id": "w1"}),
        )])
        .unwrap();
        let tasks = |repo: &str| -> Vec<String> {
            let entries = raw.work_state_entries(repo).unwrap();
            entries
                .iter()
                .map(|e| e.fields["task"].to_string())
                .collect()
        };
        assert_eq!(tasks("github.com/o/free-mem"), [r#""Imported""#]);
        assert_eq!(tasks(&crafted), [r#""Imported""#, r#""Written here""#]);
    }

    /// D5: a document keeps the uid v1's import gave it, `<source>:<o|s|p><id>`, the source named
    /// by the database's first migration time, so v1's judgments map to it.
    #[test]
    fn the_source_name_and_uids_match_the_v1_import() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        let mut uids: Vec<String> = docs(&raw).into_iter().map(|d| d.uid).collect();
        uids.sort();
        let want: Vec<String> = ["o10", "o11", "o13", "o14", "o15", "o16", "p30", "s20"]
            .iter()
            .map(|id| format!("claude-mem:b62f07076e19:{id}"))
            .collect();
        assert_eq!(uids, want);
    }

    #[test]
    fn importing_twice_appends_nothing_the_second_time() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        let ops = raw.max_op_seq_of(raw.device()).unwrap();
        assert_eq!(
            counts(&claude_mem(&mut raw, &src, &Default::default()).unwrap()),
            (0, 0, 0, 8, 2)
        );
        // claude-mem pruning its sessions does not rename the database.
        Connection::open(&src)
            .unwrap()
            .execute("DELETE FROM sdk_sessions", [])
            .unwrap();
        assert_eq!(
            counts(&claude_mem(&mut raw, &src, &Default::default()).unwrap()),
            (0, 0, 0, 8, 2)
        );
        assert_eq!(raw.max_op_seq_of(raw.device()).unwrap(), ops);
        // Another claude-mem database reuses the same row ids; its rows are not "seen".
        let other = dir.path().join("other.db");
        claude_mem_db(&other, "2026-06-26T18:19:21.955Z");
        assert_eq!(
            counts(&claude_mem(&mut raw, &other, &Default::default()).unwrap()),
            (6, 1, 1, 0, 2)
        );
    }

    /// I8 (parity row 14): claude-mem's exclusion patterns, read from the settings beside its
    /// database, come in once as patterns never recorded; one taken back is not added again, and
    /// a database with no settings beside it imports all the same.
    #[test]
    fn claude_mems_exclusions_come_in_once_as_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        assert_eq!(
            claude_mem(&mut raw, &src, &Default::default())
                .unwrap()
                .exclusions,
            0
        );
        let long = "x".repeat(257);
        let settings = serde_json::json!({
            "CLAUDE_MEM_EXCLUDED_PROJECTS":
                format!("secret-a, ~/private/** ,, secret-b, secret-a, {long}"),
            "CLAUDE_MEM_MODEL": "unrelated",
        });
        std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        // One oboete does not take is counted, never dropped unsaid (the commit's security review).
        assert_eq!((stats.exclusions, stats.exclusions_refused), (3, 1));
        let patterns = raw.capture_exclusions().unwrap().patterns;
        assert_eq!(
            Vec::from_iter(patterns),
            ["secret-a", "secret-b", "~/private/**"]
        );
        raw.exclude_capture(("pattern", "secret-b"), true).unwrap();
        assert_eq!(
            claude_mem(&mut raw, &src, &Default::default())
                .unwrap()
                .exclusions,
            0
        );
        assert_eq!(raw.capture_exclusions().unwrap().patterns.len(), 2);
    }

    /// I8: claude-mem's settings are read as claude-mem reads them, a BOM skipped and the keys
    /// under `env` when that holds them; a file that is there but cannot be read so stops the
    /// import before any write (Codex on #563).
    #[test]
    fn claude_mems_settings_are_read_as_it_reads_them_or_stop_the_import() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let settings = dir.path().join("settings.json");
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        std::fs::create_dir(&settings).unwrap();
        let unreadable = claude_mem(&mut raw, &src, &Default::default());
        std::fs::remove_dir(&settings).unwrap();
        for (given, why) in [
            ("{\"CLAUDE_MEM_EXCLUDED_PROJECTS\": \"a\"", "is not JSON"),
            ("[\"a\"]", "is not a JSON object"),
            (
                "{\"CLAUDE_MEM_EXCLUDED_PROJECTS\": [\"a\"]}",
                "other than one string",
            ),
        ] {
            std::fs::write(&settings, given).unwrap();
            let Err(err) = claude_mem(&mut raw, &src, &Default::default()) else {
                panic!("{given} imported");
            };
            let err = err.to_string();
            assert!(
                err.contains(why) && err.contains("nothing is imported"),
                "{given}: {err}"
            );
        }
        assert!(
            unreadable
                .err()
                .is_some_and(|err| err.to_string().contains("cannot be read"))
        );
        let nested = serde_json::json!({
            "env": {"CLAUDE_MEM_EXCLUDED_PROJECTS": "nested-a", "CLAUDE_MEM_MODEL": "m"},
            "CLAUDE_MEM_EXCLUDED_PROJECTS": "stale-root",
            "theme": "dark",
        });
        std::fs::write(&settings, format!("\u{feff}{nested}")).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        // The stopped runs wrote nothing: every row comes in now, none seen before.
        assert_eq!((stats.exclusions, stats.seen), (1, 0));
        assert!(stats.observations > 0);
        assert_eq!(
            Vec::from_iter(raw.capture_exclusions().unwrap().patterns),
            ["nested-a"]
        );
        // An `env` without claude-mem's keys leaves them at the top, as claude-mem reads them.
        let flat = r#"{"env": {"PATH": "/bin"}, "CLAUDE_MEM_EXCLUDED_PROJECTS": "flat-a"}"#;
        std::fs::write(&settings, flat).unwrap();
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!(stats.exclusions, 1);
        std::fs::write(&settings, r#"{"CLAUDE_MEM_EXCLUDED_PROJECTS": null}"#).unwrap();
        assert_eq!(
            claude_mem(&mut raw, &src, &Default::default())
                .unwrap()
                .exclusions,
            0
        );
    }

    /// D5: each append of `raw::IMPORT_BATCH` documents commits alone, so an import killed at one
    /// keeps the appends before it, and the next run adds the rest, once.
    #[test]
    fn a_killed_import_resumes_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let c = Connection::open(&src).unwrap();
        for id in 100..1_300 {
            c.execute(
                "INSERT INTO observations VALUES(?1, 'mem-1', 'free-mem', 1500, 'change', 'Row', ?2, '')",
                params![id, format!("Observation {id}.")],
            )
            .unwrap();
        }
        drop(c);
        let mut raw = crate::raw::open(dir.path()).unwrap();
        crate::crash::at(2);
        let killed = claude_mem(&mut raw, &src, &Default::default());
        crate::crash::off();
        assert!(killed.is_err());
        assert_eq!(docs(&raw).len(), crate::raw::IMPORT_BATCH);
        let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
        assert_eq!((stats.observations, stats.seen), (706, 500));
        let mut ids: Vec<String> = docs(&raw).into_iter().map(|d| d.source_id).collect();
        let all = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!((all, ids.len()), (1_208, 1_208));
    }

    /// D5: curation reads records and injection reads what curation made, so a home holding
    /// only imported documents has nothing to curate and shows no memory, only the work state
    /// section every session start has.
    #[test]
    fn imported_documents_are_never_curated_or_injected() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let mut raw = crate::raw::open(dir.path()).unwrap();
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        let rules = crate::redact::Rules::default();
        let window = curate::next_window(&raw, raw.device(), 100_000, &rules).unwrap();
        assert!(window.is_none());
        drop(raw);
        crate::worker::run_once(dir.path()).unwrap();
        let cwd = dir.path().join("free-mem");
        assert_eq!(
            hook::inject_text(dir.path(), &cwd, None),
            crate::work_state::nothing_open()
        );
    }

    /// Codex on #305: a second import on a home is refused while one runs.
    #[test]
    fn a_second_import_on_the_same_home_is_refused_while_one_runs() {
        let home = tempfile::tempdir().unwrap();
        let held = lock(home.path()).unwrap();
        let refused = lock(home.path()).unwrap_err();
        assert!(
            format!("{refused:#}").contains("another oboete import"),
            "{refused:#}"
        );
        drop(held);
        lock(home.path()).unwrap();
    }

    /// Decision 52 (docs/prompt-context.md, "Decision 52"): claude-mem 14.0.1's names by path.
    #[test]
    fn a_checkouts_names_are_claude_mems_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().canonicalize().unwrap();
        let mk = |p: &Path| std::fs::create_dir_all(p).unwrap();
        // A checkout and a folder in it: the top level's name.
        let main = d.join("payments");
        mk(&main.join(".git/worktrees/payments-fix"));
        mk(&main.join("src/deep"));
        assert_eq!(names_of(&main), ["payments"]);
        assert_eq!(names_of(&main.join("src/deep")), ["payments"]);
        // A linked worktree: the main repository's name, then `<main>/<worktree>`.
        let wt = d.join("payments-fix");
        mk(&wt.join("src"));
        let gitdir = |to: &Path| format!("gitdir: {}\n", to.display());
        std::fs::write(
            wt.join(".git"),
            gitdir(&main.join(".git/worktrees/payments-fix")),
        )
        .unwrap();
        assert_eq!(
            names_of(&wt.join("src")),
            ["payments", "payments/payments-fix"]
        );
        // One named as its main repository writes to it, the doubled key kept readable.
        let same = d.join("elsewhere/payments");
        mk(&same);
        std::fs::write(
            same.join(".git"),
            gitdir(&main.join(".git/worktrees/payments")),
        )
        .unwrap();
        assert_eq!(names_of(&same), ["payments/payments", "payments"]);
        // A submodule by a relative gitdir: the superproject's name, its own, `<super>/<path>`.
        let sub = main.join("vendor/lib");
        mk(&sub);
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/vendor/lib\n").unwrap();
        assert_eq!(names_of(&sub), ["payments", "lib", "payments/vendor/lib"]);
        // A `.git` file of more than its line, or naming neither, is a checkout of its own.
        std::fs::write(
            sub.join(".git"),
            "gitdir: ../../.git/modules/vendor/lib\nmore\n",
        )
        .unwrap();
        assert_eq!(names_of(&sub), ["lib"]);
        std::fs::write(wt.join(".git"), gitdir(&d.join("other/.git"))).unwrap();
        assert_eq!(names_of(&wt), ["payments-fix"]);
        // A bare repository's worktree (`<repo>.git/worktrees/<name>`) is not one to claude-mem.
        std::fs::write(
            wt.join(".git"),
            gitdir(&d.join("bare.git/worktrees/payments-fix")),
        )
        .unwrap();
        assert_eq!(names_of(&wt), ["payments-fix"]);
        // Outside git: the nearest marker root's name, the folder's own below it; no marker, the
        // folder's. A path is folded first, as `path.resolve` folds it.
        let site = d.join("site");
        mk(&site.join("pages/blog"));
        std::fs::write(site.join(".claude-mem.json"), "{}").unwrap();
        assert_eq!(names_of(&site), ["site"]);
        assert_eq!(names_of(&site.join("pages/blog")), ["blog", "site"]);
        assert_eq!(names_of(&site.join("pages/blog/..")), ["pages", "site"]);
        let loose = d.join("notes/today");
        mk(&loose);
        assert_eq!(names_of(&loose), ["today"]);
        // The walk stops before a stop (the home directory) and never starts in the config one.
        let home = d.join("home");
        mk(&home.join("work"));
        std::fs::write(home.join(".claude-mem-project"), "").unwrap();
        let config = [d.join("home/.claude")];
        let stops = [home.clone(), config[0].clone()];
        assert_eq!(marker_walk(&home.join("work"), &config, &stops), None);
        assert_eq!(
            marker_walk(&home.join("work"), &config, &[]),
            Some(home.clone())
        );
        assert_eq!(marker_walk(&home.join(".claude/x"), &config, &[]), None);
        #[cfg(unix)]
        assert_eq!(names_of(Path::new("/")), [UNKNOWN_PROJECT]);
    }
}
