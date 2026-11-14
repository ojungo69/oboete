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
    /// Rows with nothing left to store (empty, a harness notification, or a prompt while
    /// `[capture] store_prompts` is off).
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
    /// Rows left out because the gate changes a label they would be stored under, the
    /// repository's or the session's (I10): each run counts them again.
    pub masked: u64,
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

/// The newest claude-mem schema the import reads all of: 15.15.0's (docs/claude-mem-import.md I2).
/// v65 (13.36.0) only recomputes `observations.title_norm_key`; v66 adds cloud sync columns to
/// `work_state_entries` and v67 lets the sync ledgers name that kind (13.40.0); v68 (15.0.2) adds
/// `observation_files`, an index of each observation's `files_read` and `files_modified`, which
/// the import reads from the observation itself. v69 to v71 (15.7.0) and v72 (15.12.0) add the
/// tables of kept images and their links to observations; v73 and v74 (15.12.0) add the tool
/// uses already stored (against a transcript read twice), the records of deleted sessions and
/// the triggers that keep a deleted session's earlier content out. The import reads none of
/// them.
const NEWEST_SCHEMA: i64 = 74;

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
        settings,
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
            count(
                &put,
                &mut stats.observations,
                (&mut stats.seen, &mut stats.masked),
            );
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
            count(
                &sink.put(row)?,
                &mut stats.summaries,
                (&mut stats.seen, &mut stats.masked),
            );
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
            // What was typed is not stored while `[capture] store_prompts` is off, as a live
            // prompt's is not. The row is left out, so a run with it on again brings it in.
            if !settings.store_prompts {
                stats.empty += 1;
                return Ok(());
            }
            // Blocks out, secrets masked, notifications dropped, and 8,000 characters kept, where
            // a live prompt keeps 64 KiB. `clip` masks with the bundled rules alone, so that its
            // check for a key block it cuts sees every header.
            let kept = hook::clip(&hook::strip_blocks(&text, true), &settings.rules);
            // A notification is told here, before a rule of the home's can mask its tag.
            if kept.is_empty() || hook::is_envelope(&kept) {
                stats.empty += 1;
                return Ok(());
            }
            // Then the home's rules gate what `clip` kept, as they do a title and a body. A
            // value of theirs the cut would fall inside, `clip` kept none of.
            let body = redact::outbound_with(&kept, &settings.rules);
            if body.is_empty() {
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
            count(
                &sink.put(row)?,
                &mut stats.prompts,
                (&mut stats.seen, &mut stats.masked),
            );
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
            // I10: the op holds the name, which the gate must leave as it is.
            if labelled != named {
                stats.masked += 1;
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
    /// The gate its labels pass (I10).
    settings: &'a crate::capture::Settings,
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
/// it, how many of its paths its files op left out (F10), and whether what it had to write was
/// left out for a label the gate changes (I10).
struct Put {
    new: bool,
    before: bool,
    cut: u64,
    masked: bool,
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
        // Where the files op this row still owes goes, if it owes one.
        let files_under = under.clone().filter(|under| {
            !(r.files[0].is_empty() && r.files[1].is_empty())
                && !self
                    .known_files
                    .get(&r.key)
                    .is_some_and(|had| had.contains(under))
        });
        // I10: the labels as they would be written, `under` also when it is one an earlier run
        // stored, which no gate read then. A label the gate changes can neither be stored as it
        // is nor name what it named once masked or cut: what the row would write is left out.
        let kept = |field: &str, label: &str| {
            crate::capture::exclusion_label(field, label, self.settings) == label
        };
        if (new || files_under.is_some())
            && !(kept("session", &session) && under.as_deref().is_none_or(|at| kept("repo", at)))
        {
            return Ok(Put {
                new: false,
                before,
                cut: 0,
                masked: true,
            });
        }
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
        if let Some(under) = files_under {
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
        Ok(Put {
            new,
            before,
            cut,
            masked: false,
        })
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

/// A row in the import's line: left out for a label (I10), added, imported by an earlier run, or
/// none of them (one claude-mem recorded since, which `--files` leaves to the plain import, F11).
fn count(put: &Put, added: &mut u64, stats: (&mut u64, &mut u64)) {
    let (seen, masked) = stats;
    if put.masked {
        *masked += 1;
    } else if put.new {
        *added += 1;
    } else if put.before {
        *seen += 1;
    }
}

#[cfg(test)]
mod tests;
