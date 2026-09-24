//! `oboete import claude-mem <db>`: copy claude-mem's observations, session summaries and
//! prompts into this store (docs/pr-b.md, decision 2). Text passes the outbound gate on the way
//! in, prompts go through the hook's own cleaning, and `imports` remembers every source row so a
//! second run adds nothing. Repositories become `claude-mem:<project>`: claude-mem names a project,
//! not a path, and mapping the names onto oboete's repository keys waits for PR-H.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::{self, Doc};
use crate::{hook, observe, redact};

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
}

struct Session {
    id: String,
    agent: String,
    project: String,
    started: i64,
    ended: Option<i64>,
}

pub fn claude_mem(conn: &mut Connection, path: &Path) -> Result<Stats> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {} read-only", path.display()))?;
    // One read transaction: a consistent snapshot while claude-mem keeps writing.
    src.execute_batch("BEGIN")?;
    let source = source_name(&src)?;
    let tx = conn.transaction()?;
    let mut stats = Stats::default();

    // Observations and summaries name claude-mem's own session id; prompts name the agent's.
    // oboete keys sessions by the agent's id, so the same session captured by both lines up.
    let mut by_memory: HashMap<String, Session> = HashMap::new();
    let mut by_content: HashMap<String, Session> = HashMap::new();
    let mut stmt = src.prepare(
        "SELECT memory_session_id, content_session_id, COALESCE(project, ''),
                COALESCE(platform_source, 'claude'), COALESCE(started_at_epoch, 0), completed_at_epoch
         FROM sdk_sessions",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let memory: Option<String> = r.get(0)?;
        let content: String = r.get(1)?;
        let session = || -> rusqlite::Result<Session> {
            Ok(Session {
                id: content.clone(),
                agent: r.get(3)?,
                project: r.get(2)?,
                started: r.get(4)?,
                ended: r.get(5)?,
            })
        };
        if let Some(m) = memory {
            by_memory.insert(m, session()?);
        }
        by_content.insert(content.clone(), session()?);
    }
    drop(rows);
    drop(stmt);

    let mut stmt = src.prepare(
        "SELECT id, COALESCE(memory_session_id, ''), COALESCE(project, ''), created_at_epoch, COALESCE(type, ''), COALESCE(title, ''),
                COALESCE(narrative, ''), COALESCE(facts, '') FROM observations ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (id, memory, project, ts): (i64, String, String, i64) =
            (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let title = redact::outbound(&r.get::<_, String>(5)?);
        let body = redact::outbound(&observation_body(
            &r.get::<_, String>(6)?,
            &r.get::<_, String>(7)?,
        ));
        if title.is_empty() && body.is_empty() {
            stats.empty += 1;
            continue;
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
        count(
            put(&tx, &source, row)?,
            &mut stats.observations,
            &mut stats.seen,
        );
    }
    drop(rows);
    drop(stmt);

    let mut stmt = src.prepare(
        "SELECT id, COALESCE(memory_session_id, ''), COALESCE(project, ''), created_at_epoch, COALESCE(request, ''),
                COALESCE(investigated, ''), COALESCE(learned, ''), COALESCE(completed, ''),
                COALESCE(next_steps, '') FROM session_summaries ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (id, memory, project, ts): (i64, String, String, i64) =
            (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let mut parts = Vec::new();
        for (i, label) in [
            "Request",
            "Investigated",
            "Learned",
            "Completed",
            "Next steps",
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
            continue;
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
        count(
            put(&tx, &source, row)?,
            &mut stats.summaries,
            &mut stats.seen,
        );
    }
    drop(rows);
    drop(stmt);

    let mut stmt = src.prepare(
        "SELECT id, COALESCE(content_session_id, ''), created_at_epoch, COALESCE(prompt_text, '')
         FROM user_prompts ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (id, content, ts, text): (i64, String, i64, String) =
            (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        // The same cleaning a live prompt gets: blocks out, secrets masked, notifications dropped.
        let body = hook::clip(&hook::strip_blocks(&text, true));
        if body.is_empty() || hook::is_envelope(&body) {
            stats.empty += 1;
            continue;
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
        count(put(&tx, &source, row)?, &mut stats.prompts, &mut stats.seen);
    }
    drop(rows);
    drop(stmt);
    tx.commit()?;
    Ok(stats)
}

/// claude-mem's ids restart in every database (the Windows copy and the WSL one both have an
/// observation 1), so the import key names the database by its first session, which a backup or a
/// move keeps: importing a snapshot and later the live file adds only what is new.
fn source_name(src: &Connection) -> Result<String> {
    let first: Option<(String, i64)> = src
        .query_row(
            "SELECT content_session_id, COALESCE(started_at_epoch, 0) FROM sdk_sessions ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match first {
        Some((id, ts)) => {
            let hash = Sha256::digest(format!("{id}:{ts}").as_bytes());
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

/// Store a row with its session. A row an earlier run imported is skipped before its session is
/// written, so a session the developer deleted since does not come back empty. False = seen.
fn put(tx: &Connection, source: &str, r: Row) -> Result<bool> {
    if db::imported(tx, source, &r.key)? {
        return Ok(false);
    }
    let (id, agent, project, started, ended) = match r.session {
        Some(s) => (
            s.id.clone(),
            s.agent.as_str(),
            s.project.as_str(),
            s.started,
            s.ended,
        ),
        // No session id at all: a session of its own, so unrelated rows do not merge.
        None if r.session_id.is_empty() => (
            format!("{source}/{}", r.key),
            "claude",
            r.project,
            r.ts,
            None,
        ),
        // A session claude-mem no longer lists keeps its id.
        None => (r.session_id.to_string(), "claude", r.project, r.ts, None),
    };
    db::import_session(tx, &id, agent, &repo(project), started, ended)?;
    let repo = repo(r.project);
    let doc = Doc {
        session_id: &id,
        repo: &repo,
        ts: r.ts,
        kind: r.kind,
        title: r.title,
        body: r.body,
    };
    db::import_doc(tx, source, &r.key, &doc)
}

fn repo(project: &str) -> String {
    format!("{SOURCE}:{project}")
}

/// claude-mem's types are oboete's kinds, plus a few it wrote by mistake (`discovery>`) or keeps
/// for itself (`security_note`); those count as discoveries.
fn kind(t: &str) -> &'static str {
    observe::KINDS
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
    use rusqlite::params;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "oboete-import-{name}-{}-{}",
            std::process::id(),
            db::now_ms()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The columns of claude-mem's tables that the import reads.
    /// `started` changes the database's first session, so two calls stand for two databases.
    fn claude_mem_db(path: &Path, started: i64) {
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
        c.execute(
            "INSERT INTO sdk_sessions VALUES(1, 'agent-1', 'mem-1', 'free-mem', 'codex', ?1, 2000)",
            [started],
        )
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

    #[test]
    fn claude_mem_rows_arrive_gated_mapped_and_once() {
        let dir = tmp("claude-mem");
        let src = dir.join("claude-mem.db");
        claude_mem_db(&src, 1000);
        let mut conn = db::open(&dir).unwrap();
        let stats = claude_mem(&mut conn, &src).unwrap();
        assert_eq!(
            (
                stats.observations,
                stats.summaries,
                stats.prompts,
                stats.seen,
                stats.empty
            ),
            (6, 1, 1, 0, 2)
        );

        let obs: Vec<(String, String, String, String, String)> = conn
            .prepare("SELECT session_id, repo, kind, title, body FROM observations ORDER BY id")
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            obs[0],
            (
                "agent-1".into(),
                "claude-mem:free-mem".into(),
                "decision".into(),
                "Chose SQLite".into(),
                "Because it is one file.\n- No server".into()
            )
        );
        assert_eq!(
            (obs[1].2.as_str(), obs[1].4.as_str()),
            ("discovery", "- Fact only")
        );
        assert!(
            !obs[2].4.contains("ghp_") && !obs[2].4.contains("client name"),
            "{}",
            obs[2].4
        );
        assert!(obs[2].4.contains("[REDACTED]"));
        assert_eq!((obs[3].0.as_str(), obs[3].3.as_str()), ("gone", "Orphan"));

        let summary: String = conn
            .query_row("SELECT body FROM summaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            summary,
            "Request: Fix search\nLearned: FTS5 trigram\nCompleted: Shipped"
        );
        let prompt: String = conn
            .query_row("SELECT body FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert!(!prompt.contains("my name") && prompt.starts_with("How do I add"));

        let sessions: Vec<(String, String, String)> = conn
            .prepare("SELECT id, agent, repo FROM sessions ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sessions.len(), 4);
        assert_eq!(
            sessions[0],
            (
                "agent-1".into(),
                "codex".into(),
                "claude-mem:free-mem".into()
            )
        );
        assert_eq!(
            sessions[3],
            ("gone".into(), "claude".into(), "claude-mem:free-mem".into())
        );
        let source = source_name(&Connection::open(&src).unwrap()).unwrap();
        assert!(source.starts_with("claude-mem:"));
        assert_eq!(sessions[1].0, format!("{source}/o15"));
        assert_eq!(sessions[2].0, format!("{source}/o16"));
        // Every document is searchable and traceable to its claude-mem row.
        let hits = crate::search::search(&conn, "trigram", None, 10).unwrap();
        let mapped: String = conn
            .query_row(
                "SELECT doc FROM imports WHERE source LIKE 'claude-mem:%' AND source_id='s20'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            hits.iter().map(|h| h.doc.as_str()).collect::<Vec<_>>(),
            [mapped.as_str()]
        );
        let (fts, imports): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM fts), (SELECT count(*) FROM imports)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((fts, imports), (8, 8));

        // A second run adds nothing.
        let again = claude_mem(&mut conn, &src).unwrap();
        assert_eq!(
            (
                again.observations,
                again.summaries,
                again.prompts,
                again.seen
            ),
            (0, 0, 0, 8)
        );
        let fts: i64 = conn
            .query_row("SELECT count(*) FROM fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 8);

        // A session the developer deleted stays deleted when the same database comes again.
        assert!(db::delete_session(&mut conn, "gone").unwrap());
        claude_mem(&mut conn, &src).unwrap();
        let gone: i64 = conn
            .query_row("SELECT count(*) FROM sessions WHERE id='gone'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(gone, 0);

        // Another claude-mem database reuses the same row ids; its rows are not "seen".
        let other = dir.join("other.db");
        claude_mem_db(&other, 5000);
        let second = claude_mem(&mut conn, &other).unwrap();
        assert_eq!(
            (
                second.observations,
                second.summaries,
                second.prompts,
                second.seen
            ),
            (6, 1, 1, 0)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
