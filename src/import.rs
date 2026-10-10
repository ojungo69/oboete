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

use crate::raw::{ImportDoc, Raw};
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
}

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

/// The newest claude-mem schema the import reads all of: 13.35.0's (docs/claude-mem-import.md I2).
const NEWEST_SCHEMA: i64 = 64;

/// `settings` gates the work state rows as a write's (docs/work-state.md L5).
pub fn claude_mem(
    raw: &mut Raw,
    path: &Path,
    settings: &crate::capture::Settings,
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
    let mut sink = Sink {
        known: raw.import_keys(&source)?,
        raw,
        source: &source,
        docs: Vec::new(),
    };
    let mut stats = Stats::default();

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
                    COALESCE(facts, '') FROM observations ORDER BY id"
        ),
        |r| {
            let (id, memory, project, ts): (i64, String, String, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let title = redact::outbound(&r.get::<_, String>(5)?);
            let body = redact::outbound(&observation_body(
                &r.get::<_, String>(6)?,
                &r.get::<_, String>(7)?,
            ));
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
            };
            count(sink.put(row)?, &mut stats.observations, &mut stats.seen);
            Ok(())
        },
    )?;

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
            let body = redact::outbound(&parts.join("\n"));
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
            };
            count(sink.put(row)?, &mut stats.summaries, &mut stats.seen);
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
            let body = hook::clip(&hook::strip_blocks(&text, true));
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
            };
            count(sink.put(row)?, &mut stats.prompts, &mut stats.seen);
            Ok(())
        },
    )?;
    sink.flush()?;
    if has_table(&src, "work_state_entries")? {
        work_state(raw, &src, &source, settings, &mut stats)?;
    }
    Ok(stats)
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
    // A run that stops keeps the batches it appended; the next adds the rest.
    for batch in ops.chunks(crate::raw::IMPORT_BATCH) {
        raw.append_ops(batch)?;
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
}

/// Where rows go: import ops, appended every `raw::IMPORT_BATCH` documents, each append its own
/// transaction, so a run that stops keeps what it appended.
struct Sink<'a> {
    raw: &'a mut Raw,
    source: &'a str,
    /// The source ids imported so far, by earlier runs and this one.
    known: HashSet<String>,
    docs: Vec<ImportDoc>,
}

impl Sink<'_> {
    /// A row as an import op, with its session. False: imported before.
    fn put(&mut self, r: Row) -> Result<bool> {
        if !self.known.insert(r.key.clone()) {
            return Ok(false);
        }
        let session = match r.session {
            Some(s) => s.id.clone(),
            // No session id at all: a session of its own, so unrelated rows do not merge.
            None if r.session_id.is_empty() => format!("{}/{}", self.source, r.key),
            // A session claude-mem no longer lists keeps its id.
            None => r.session_id.to_owned(),
        };
        self.docs.push(ImportDoc {
            uid: format!("{}:{}", self.source, r.key),
            source: self.source.to_owned(),
            source_id: r.key,
            kind: r.kind.to_owned(),
            repo: repo(r.project),
            session,
            ts: r.ts,
            title: r.title.to_owned(),
            body: r.body.to_owned(),
        });
        if self.docs.len() >= crate::raw::IMPORT_BATCH {
            self.flush()?;
        }
        Ok(true)
    }

    fn flush(&mut self) -> Result<()> {
        self.raw.append_imports(std::mem::take(&mut self.docs))?;
        Ok(())
    }
}

/// claude-mem's `project` as its documents' repository.
pub(crate) fn repo(project: &str) -> String {
    format!("{SOURCE}:{project}")
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

fn count(new: bool, added: &mut u64, seen: &mut u64) {
    *if new { added } else { seen } += 1;
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

    /// The import ops raw holds, in op order.
    fn docs(raw: &Raw) -> Vec<ImportDoc> {
        raw.ops_after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::Import)
            .map(|o| serde_json::from_value(o.body).unwrap())
            .collect()
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

    /// Test 1: a database at a schema newer than the import knows is refused, nothing written.
    #[test]
    fn a_database_newer_than_v64_is_refused_with_nothing_written() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
        Connection::open(&src)
            .unwrap()
            .execute("INSERT INTO schema_versions VALUES(3, 65, '')", [])
            .unwrap();
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let said = claude_mem(&mut raw, &src, &Default::default())
            .err()
            .unwrap()
            .to_string();
        assert!(said.contains("v65") && said.contains("v64"), "{said}");
        assert!(docs(&raw).is_empty() && work_ops(&raw).is_empty());
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
        raw.work_state(repo, "plan", done.as_object().unwrap())
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
        raw.work_state(repo, "plan", done.as_object().unwrap())
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
        raw.work_state(&crafted, "plan", task("Written here").as_object().unwrap())
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
}
