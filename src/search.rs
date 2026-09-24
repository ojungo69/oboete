//! Search over what is stored. One FTS5 trigram table (`fts`) keeps a copy of every
//! observation, summary and prompt, so a query is one SQL statement and CJK text is indexed by
//! character. Terms of three or more characters go through MATCH (bm25 order, Unicode case
//! folding); a shorter term cannot hit a trigram index and is ANDed on as a literal LIKE
//! (ASCII case folding only), which is a scan the small tables can afford.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

/// `doc` is `o<id>` for an observation, `s<id>` for a summary, `p<id>` for a prompt; `when` is
/// local time.
pub struct Hit {
    pub doc: String,
    pub kind: String,
    pub repo: String,
    pub when: String,
    pub title: String,
    pub body: String,
}

const COLUMNS: &str =
    "doc, kind, repo, strftime('%Y-%m-%d %H:%M', ts / 1000, 'unixepoch', 'localtime'), title, body";

fn hit(r: &rusqlite::Row) -> rusqlite::Result<Hit> {
    Ok(Hit {
        doc: r.get(0)?,
        kind: r.get(1)?,
        repo: r.get(2)?,
        when: r.get(3)?,
        title: r.get(4)?,
        body: r.get(5)?,
    })
}

/// Whitespace-separated terms, all required. `repo = None` searches every repository. Prompts
/// come after observations and summaries.
pub fn search(
    conn: &Connection,
    query: &str,
    repo: Option<&str>,
    limit: usize,
) -> Result<Vec<Hit>> {
    let (long, short): (Vec<&str>, Vec<&str>) = query
        .split_whitespace()
        .partition(|t| t.chars().count() >= 3);
    if long.is_empty() && short.is_empty() {
        return Ok(Vec::new());
    }
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if !long.is_empty() {
        // Each term as an FTS5 string (quotes doubled), implicit AND between them.
        let q = long
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        clauses.push("fts MATCH ?".into());
        args.push(Value::Text(q));
    }
    for t in &short {
        let pattern = format!(
            "%{}%",
            t.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        clauses.push("(title LIKE ? ESCAPE '\\' OR body LIKE ? ESCAPE '\\')".into());
        args.push(Value::Text(pattern.clone()));
        args.push(Value::Text(pattern));
    }
    if let Some(r) = repo {
        clauses.push("repo = ?".into());
        args.push(Value::Text(r.to_string()));
    }
    // Knowledge before prompts: bm25 favours short documents, and a prompt is usually a short
    // question where an observation is the answer.
    let order = if long.is_empty() {
        "kind = 'prompt', ts DESC"
    } else {
        "kind = 'prompt', rank, ts DESC"
    };
    let mut sql = format!("SELECT {COLUMNS} FROM fts WHERE {}", clauses.join(" AND "));
    sql.push_str(&format!(" ORDER BY {order} LIMIT ?"));
    args.push(Value::Integer(sql_limit(limit)));
    let mut stmt = conn.prepare(&sql)?;
    let hits = stmt.query_map(params_from_iter(args), hit)?;
    Ok(hits.collect::<Result<_, _>>()?)
}

/// `oboete eval`: run each `{"qid","text"}` line through `search` over every repository and
/// print the hits as a TREC run (`qid Q0 doc rank score method`), ranks from 1. The score only
/// restates the order; the evaluator ranks by it.
pub fn trec_run(conn: &Connection, queries: &str, depth: usize) -> Result<String> {
    let mut out = String::new();
    for line in queries.lines().filter(|l| !l.trim().is_empty()) {
        let q: serde_json::Value = serde_json::from_str(line)?;
        let (Some(qid), Some(text)) = (q["qid"].as_str(), q["text"].as_str()) else {
            anyhow::bail!("each line needs string qid and text: {line}");
        };
        for (i, h) in search(conn, text, None, depth)?.iter().enumerate() {
            out.push_str(&format!("{qid} Q0 {} {} {} fts\n", h.doc, i + 1, depth - i));
        }
    }
    Ok(out)
}

/// `as i64` would wrap a huge `--limit` negative, which SQLite reads as "no limit".
fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

pub fn get(conn: &Connection, doc: &str) -> Result<Option<Hit>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM fts WHERE doc = ?1"),
            params![doc],
            hit,
        )
        .optional()?)
}

pub struct SessionRow {
    pub id: String,
    pub agent: String,
    pub repo: String,
    pub when: String,
    pub summary: String,
}

/// Sessions newest first with their latest summary (empty until observe has run).
pub fn timeline(conn: &Connection, repo: Option<&str>, limit: usize) -> Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent, s.repo,
                strftime('%Y-%m-%d %H:%M', s.started_at / 1000, 'unixepoch', 'localtime'),
                COALESCE((SELECT body FROM summaries WHERE session_id = s.id ORDER BY ts DESC LIMIT 1), '')
         FROM sessions s WHERE ?1 IS NULL OR s.repo = ?1
         ORDER BY s.started_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![repo, sql_limit(limit)], |r| {
        Ok(SessionRow {
            id: r.get(0)?,
            agent: r.get(1)?,
            repo: r.get(2)?,
            when: r.get(3)?,
            summary: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub struct RepoRow {
    pub repo: String,
    pub sessions: i64,
    pub last: String,
}

/// Every repository with sessions, most recently active first.
pub fn repos(conn: &Connection) -> Result<Vec<RepoRow>> {
    let mut stmt = conn.prepare(
        "SELECT repo, COUNT(*),
                strftime('%Y-%m-%d %H:%M', MAX(started_at) / 1000, 'unixepoch', 'localtime')
         FROM sessions GROUP BY repo ORDER BY MAX(started_at) DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RepoRow {
            repo: r.get(0)?,
            sessions: r.get(1)?,
            last: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What one session left: its summaries, its prompts, then its observations, each in the order
/// stored.
pub fn session_docs(conn: &Connection, session_id: &str) -> Result<Vec<Hit>> {
    let mut stmt = conn.prepare(
        "SELECT doc, kind, repo, strftime('%Y-%m-%d %H:%M', ts / 1000, 'unixepoch', 'localtime'),
                title, body
         FROM (SELECT 0 AS g, id, 's' || id AS doc, 'summary' AS kind, repo, ts, '' AS title, body
                 FROM summaries WHERE session_id = ?1
               UNION ALL
               SELECT 1, id, 'p' || id, 'prompt', repo, ts, '', body
                 FROM prompts WHERE session_id = ?1
               UNION ALL
               SELECT 2, id, 'o' || id, kind, repo, ts, title, body
                 FROM observations WHERE session_id = ?1)
         ORDER BY g, id",
    )?;
    let hits = stmt.query_map(params![session_id], hit)?;
    Ok(hits.collect::<Result<_, _>>()?)
}

pub struct FeedRow {
    pub hit: Hit,
    pub session: String,
    pub agent: String,
}

/// Every summary, observation and prompt, newest first. Summaries and observations carry their
/// session's last event and come as a block (the summary, then the observations in stored
/// order); a prompt carries the moment it was typed, so it shows up while its session runs and
/// sits below what the session left. A session deleted while observe was writing leaves rows
/// with no session row, so its agent comes back empty rather than the rows vanishing.
pub fn feed(conn: &Connection, repo: Option<&str>, limit: usize) -> Result<Vec<FeedRow>> {
    let mut stmt = conn.prepare(
        "SELECT d.doc, d.kind, d.repo, strftime('%Y-%m-%d %H:%M', d.ts / 1000, 'unixepoch', 'localtime'),
                d.title, d.body, d.session_id, COALESCE(s.agent, '')
         FROM (SELECT 0 AS g, id, 's' || id AS doc, 'summary' AS kind, repo, ts, '' AS title, body, session_id
                 FROM summaries
               UNION ALL
               SELECT 1, id, 'o' || id, kind, repo, ts, title, body, session_id FROM observations
               UNION ALL
               SELECT 2, id, 'p' || id, 'prompt', repo, ts, '', body, session_id FROM prompts) d
         LEFT JOIN sessions s ON s.id = d.session_id
         WHERE ?1 IS NULL OR d.repo = ?1
         ORDER BY d.ts DESC, d.session_id, d.g, d.id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![repo, sql_limit(limit)], |r| {
        Ok(FeedRow {
            hit: hit(r)?,
            session: r.get(6)?,
            agent: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One line of `body`, `width` characters around the first term found (case-insensitive).
pub fn snippet(body: &str, terms: &[&str], width: usize) -> String {
    let flat = body.replace('\n', " ");
    let lower = flat.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(&t.to_lowercase()))
        .min()
        .unwrap_or(0);
    let chars: Vec<char> = flat.chars().collect();
    let at = lower[..at].chars().count().min(chars.len());
    let start = at.saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn home(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("oboete-search-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn seed(conn: &mut Connection) {
        db::upsert_session(conn, "s1", "claude", "/r", "/r", 1_700_000_000_000).unwrap();
        db::apply_batch(
            conn,
            &db::PendingSession {
                id: "s1".into(),
                agent: "claude".into(),
                repo: "/r".into(),
                last_event_at: 1_700_000_000_000,
            },
            "test",
            "セッションの要約: 検索を実装した",
            &[
                db::Observation {
                    kind: "change".into(),
                    title: "src/db.rs を更新".into(),
                    body: "DB 接続とクエリを直した\n二行目".into(),
                },
                db::Observation {
                    kind: "decision".into(),
                    title: "use the trigram tokenizer".into(),
                    body: "FTS5 trigram indexes CJK text by character".into(),
                },
                db::Observation {
                    kind: "change".into(),
                    title: "ÉCOLE coverage".into(),
                    body: "coverage 50% done, ÄÖÜ".into(),
                },
            ],
            i64::MAX,
        )
        .unwrap();
        db::insert_prompt(conn, "s1", 1_699_999_990_000, "trigram 検索を足して").unwrap();
    }

    #[test]
    fn eval_prints_a_trec_run_ranked_from_one() {
        let dir = home("trec");
        let mut conn = db::open(&dir).unwrap();
        seed(&mut conn);
        let queries = "{\"qid\":\"q1\",\"text\":\"Trigram\"}\n\n{\"qid\":\"q2\",\"text\":\"nothing matches this\"}\n";
        assert_eq!(
            trec_run(&conn, queries, 50).unwrap(),
            "q1 Q0 o2 1 50 fts\nq1 Q0 p1 2 49 fts\n"
        );
        assert_eq!(trec_run(&conn, queries, 1).unwrap(), "q1 Q0 o2 1 1 fts\n");
        assert!(trec_run(&conn, "{\"qid\":1,\"text\":\"x\"}", 5).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn match_like_get_and_timeline() {
        let dir = home("basic");
        let mut conn = db::open(&dir).unwrap();
        seed(&mut conn);
        let docs = |q: &str, repo: Option<&str>| -> Vec<String> {
            search(&conn, q, repo, 10)
                .unwrap()
                .into_iter()
                .map(|h| h.doc)
                .collect()
        };
        // 3+ characters: trigram MATCH, English and Japanese alike, case-insensitive.
        assert_eq!(docs("Trigram", None), vec!["o2", "p1"]);
        assert_eq!(docs("足して", None), vec!["p1"]);
        assert_eq!(docs("クエリ", None), vec!["o1"]);
        assert_eq!(docs("検索 要約", None), vec!["s1"]);
        assert_eq!(docs("trigram \"quoted\"", None), Vec::<String>::new());
        // A term under 3 characters is a literal LIKE (ASCII case folding), ANDed with the
        // MATCH of the longer ones; `%` and `_` are not wildcards.
        assert_eq!(docs("接続", None), vec!["o1"]);
        assert_eq!(docs("db", None), vec!["o1"]);
        assert_eq!(docs("接続 クエリ", None), vec!["o1"]);
        assert_eq!(docs("接続 trigram", None), Vec::<String>::new());
        assert_eq!(docs("use trigram", None), vec!["o2"]);
        assert_eq!(docs("éco ÄÖ", None), vec!["o3"]);
        assert_eq!(docs("50%", None), vec!["o3"]);
        assert_eq!(docs("0%", None), vec!["o3"]);
        assert_eq!(docs("0_", None), Vec::<String>::new());
        assert_eq!(docs("", None), Vec::<String>::new());
        // Repository filter.
        assert_eq!(docs("tokenizer", Some("/r")), vec!["o2"]);
        assert_eq!(docs("trigram", Some("/other")), Vec::<String>::new());
        assert_eq!(search(&conn, "trigram", None, 0).unwrap().len(), 0);

        let h = get(&conn, "o1").unwrap().unwrap();
        assert_eq!(
            (h.kind.as_str(), h.title.as_str()),
            ("change", "src/db.rs を更新")
        );
        assert!(h.body.contains("二行目") && h.when.starts_with("2023-11-1"));
        let s = get(&conn, "s1").unwrap().unwrap();
        assert_eq!((s.kind.as_str(), s.title.as_str()), ("summary", ""));
        assert!(get(&conn, "o99").unwrap().is_none());

        let rows = timeline(&conn, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].agent.as_str(), rows[0].repo.as_str()),
            ("claude", "/r")
        );
        assert!(rows[0].summary.starts_with("セッションの要約"));
        assert!(timeline(&conn, Some("/other"), 10).unwrap().is_empty());

        let r = repos(&conn).unwrap();
        assert_eq!((r.len(), r[0].repo.as_str(), r[0].sessions), (1, "/r", 1));
        let docs: Vec<String> = session_docs(&conn, "s1")
            .unwrap()
            .into_iter()
            .map(|h| h.doc)
            .collect();
        assert_eq!(docs, ["s1", "p1", "o1", "o2", "o3"]);
        let p = get(&conn, "p1").unwrap().unwrap();
        assert_eq!((p.kind.as_str(), p.title.as_str()), ("prompt", ""));
        assert!(session_docs(&conn, "nope").unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn older_database_without_fts_is_backfilled_once() {
        let dir = home("backfill");
        let mut conn = db::open(&dir).unwrap();
        seed(&mut conn);
        conn.execute_batch("DROP TABLE fts").unwrap();
        drop(conn);
        // Several agents' hooks can open the upgraded database at the same moment.
        let openers: Vec<_> = (0..4)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || db::open(&d).map(drop))
            })
            .collect();
        for h in openers {
            h.join().unwrap().unwrap();
        }
        let conn = db::open(&dir).unwrap();
        assert_eq!(search(&conn, "trigram", None, 10).unwrap().len(), 2);
        assert_eq!(search(&conn, "要約", None, 10).unwrap().len(), 1);
        assert_eq!(search(&conn, "足して", None, 10).unwrap()[0].doc, "p1");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snippet_centres_on_the_first_term() {
        let body = "aaaaaaaaaa bbbbbbbbbb cccccccccc TARGET dddddddddd eeeeeeeeee";
        let s = snippet(body, &["zzz", "target"], 24);
        assert!(
            s.contains("TARGET") && s.starts_with('…') && s.ends_with('…'),
            "{s}"
        );
        assert_eq!(snippet("short\nline", &["nothing"], 40), "short line");
        assert_eq!(snippet("日本語の本文です", &["本文"], 4), "…の本文で…");
    }
}
