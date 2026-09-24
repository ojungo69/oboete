//! Search over what is stored. One FTS5 trigram table (`fts`) keeps a copy of every
//! observation, summary and prompt, so a query is one SQL statement and CJK text is indexed by
//! character. A query is cut into character trigrams, ORed and ranked by bm25 (Unicode case
//! folding), so a Japanese sentence or a question in the developer's own words still finds the
//! documents that share the most of its rarer pieces (PR-E0, measured in `docs/pr-e0.md`). A
//! query too short for any trigram falls back to literal LIKE terms, all required (ASCII case
//! folding only), which is a scan the small tables can afford.

use anyhow::{Context, Result};
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

/// The query's trigrams: each run between whitespace and punctuation gives its overlapping
/// three-character pieces, except all-hiragana ones (particles and verb endings match almost
/// every Japanese document). Deduplicated, at most 64 so a pasted page stays one quick query.
// ponytail: every trigram is ORed, so a common one scans a long posting list (180k documents:
// p50 0.3 s, p95 0.8 s). Fine for MCP and the viewer; the injection hook (PR-F, 300 ms) should
// keep only the rarest trigrams (measured in docs/pr-e0.md).
fn trigrams(query: &str) -> Vec<String> {
    const SEPARATORS: &str = "、。，．,.!?！？「」『』()（）[]{}:;：；\"'`<>";
    let hiragana = |c: &char| ('\u{3040}'..='\u{309f}').contains(c);
    let mut out: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for run in query.split(|c: char| c.is_whitespace() || SEPARATORS.contains(c)) {
        let chars: Vec<char> = run.chars().collect();
        for w in chars.windows(3) {
            // The index folds case, so `HTTP` and `http` are one piece (else bm25 counts it twice).
            // The query keeps its spelling: SQLite folds it as it folded the index, and a char
            // whose lowercase is longer (`İ`) would no longer be one trigram.
            let folded: String = w.iter().map(|&c| fold(c)).collect();
            if !w.iter().all(hiragana) && !seen.contains(&folded) && out.len() < 64 {
                seen.push(folded);
                out.push(w.iter().collect());
            }
        }
    }
    out
}

/// One char's case fold for dedup and the snippet: ASCII only, which SQLite's tokenizer folds too.
/// ponytail: under-folds other scripts (`Σ`/`ς` stay two keys, so a query holding both counts
/// that piece twice in bm25) but never merges what SQLite keeps apart (`ı` and `i`), which would
/// drop a branch; exact parity means porting SQLite's Unicode fold table.
fn fold(c: char) -> char {
    c.to_ascii_lowercase()
}

/// What a hit is matched on, for `snippet`: the query's trigrams, or its terms when it has none.
pub fn terms(query: &str) -> Vec<String> {
    let grams = trigrams(query);
    if grams.is_empty() {
        query.split_whitespace().map(String::from).collect()
    } else {
        grams
    }
}

/// Candidates per side for the hybrid (docs/pr-d.md, #46): the full-text list's top 100, split by
/// kind, and each kind's 100 nearest vectors.
const HYBRID_DEPTH: usize = 100;

/// Ranked search as every surface uses it: the hybrid of docs/pr-d.md (`hybrid-kf`) when semantic
/// search is on, else full-text. The query's vector is fetched while the full-text side runs; when
/// it cannot be had (offline, no token), the result is the full-text one.
pub fn find(
    conn: &Connection,
    embedding: &crate::config::Embedding,
    query: &str,
    repo: Option<&str>,
    limit: usize,
) -> Result<Vec<Hit>> {
    if embedding.provider != "workers-ai" || limit == 0 {
        return search(conn, query, repo, limit);
    }
    let depth = limit.max(HYBRID_DEPTH);
    let (lexical, qvec) = std::thread::scope(|s| {
        let q = s.spawn(|| crate::embed::query(embedding, query));
        (search(conn, query, repo, depth), q.join())
    });
    let qvec = match qvec {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            eprintln!("oboete: full-text search only: {e:#}");
            None
        }
        Err(_) => None,
    };
    fuse(conn, lexical?, qvec.as_deref(), repo, None, limit)
}

/// Knowledge first, then prompts; within each kind the full-text ranking and the vector ranking
/// (when there is a query vector) fused by reciprocal rank, k = 60. `skip_session` (evaluation
/// only) keeps that session's documents out of the vector side; `lexical` comes without them.
fn fuse(
    conn: &Connection,
    lexical: Vec<Hit>,
    qvec: Option<&[f32]>,
    repo: Option<&str>,
    skip_session: Option<&str>,
    limit: usize,
) -> Result<Vec<Hit>> {
    let depth = limit.max(HYBRID_DEPTH);
    let mut out = Vec::new();
    let (knowledge, prompts): (Vec<Hit>, Vec<Hit>) =
        lexical.into_iter().partition(|h| h.kind != "prompt");
    for (hits, prompts) in [(knowledge, false), (prompts, true)] {
        let dense = match qvec {
            Some(q) => crate::embed::nearest(conn, q, repo, prompts, skip_session, depth)?,
            None => Vec::new(),
        };
        let mut order: Vec<String> = Vec::new();
        let mut score: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let ranked = hits.iter().map(|h| &h.doc).chain(dense.iter());
        let ranks = (0..hits.len()).chain(0..dense.len());
        for (doc, rank) in ranked.zip(ranks) {
            let s = score.entry(doc.clone()).or_insert_with(|| {
                order.push(doc.clone());
                0.0
            });
            *s += 1.0 / (61.0 + rank as f64);
        }
        // Stable: equal scores keep the order they were first seen in (full-text first).
        order.sort_by(|a, b| score[b].total_cmp(&score[a]));
        let mut by_doc: std::collections::HashMap<String, Hit> =
            hits.into_iter().map(|h| (h.doc.clone(), h)).collect();
        for doc in order {
            if out.len() == limit {
                return Ok(out);
            }
            let hit = match by_doc.remove(&doc) {
                Some(h) => Some(h),
                None => get(conn, &doc)?,
            };
            out.extend(hit);
        }
    }
    Ok(out)
}

/// Ranked search. `repo = None` searches every repository. Prompts come after observations and
/// summaries.
pub fn search(
    conn: &Connection,
    query: &str,
    repo: Option<&str>,
    limit: usize,
) -> Result<Vec<Hit>> {
    let grams = trigrams(query);
    let short: Vec<&str> = if grams.is_empty() {
        query.split_whitespace().collect()
    } else {
        Vec::new()
    };
    if grams.is_empty() && short.is_empty() {
        return Ok(Vec::new());
    }
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if !grams.is_empty() {
        // Each trigram as an FTS5 string (quotes doubled), ORed.
        let q = grams
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
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
    let order = if grams.is_empty() {
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

/// The first `want` hits outside `session`, asking `hits` for more until there are enough or the
/// search runs out.
fn outside(
    conn: &Connection,
    session: Option<&str>,
    want: usize,
    hits: impl Fn(usize) -> Result<Vec<Hit>>,
) -> Result<Vec<Hit>> {
    let mut n = want;
    loop {
        let all = hits(n)?;
        let got = all.len();
        let mut kept = Vec::new();
        for h in all {
            if kept.len() < want
                && (session.is_none()
                    || crate::db::doc_session(conn, &h.doc)?.as_deref() != session)
            {
                kept.push(h);
            }
        }
        if kept.len() == want || got < n {
            return Ok(kept);
        }
        n *= 2;
    }
}

/// `oboete eval`: run each `{"qid","text"}` line through `search` over every repository and
/// print the hits as a TREC run (`qid Q0 doc rank score method`), ranks from 1. The score only
/// restates the order; the evaluator ranks by it. A line's optional `session` is the conversation
/// the question came from: its documents hold the answer written after it, so they are left out
/// before ranking (proposal §3.1) and the next hits move up.
pub fn trec_run(
    conn: &Connection,
    queries: &str,
    depth: usize,
    embedding: Option<&crate::config::Embedding>,
) -> Result<String> {
    let method = if embedding.is_some() { "hybrid" } else { "fts" };
    let mut out = String::new();
    for line in queries.lines().filter(|l| !l.trim().is_empty()) {
        let q: serde_json::Value = serde_json::from_str(line)?;
        let (Some(qid), Some(text)) = (q["qid"].as_str(), q["text"].as_str()) else {
            anyhow::bail!("each line needs string qid and text: {line}");
        };
        // A TREC run is whitespace-separated columns.
        anyhow::ensure!(
            !qid.is_empty() && !qid.contains(char::is_whitespace),
            "qid must be one token without whitespace: {qid:?}"
        );
        // A malformed session must not quietly turn the same-session exclusion off.
        let session = match &q["session"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.as_str()),
            _ => anyhow::bail!("session must be a string: {line}"),
        };
        // A hybrid run must not quietly become a full-text one: a query that cannot be embedded
        // stops the run.
        let qvec = match embedding {
            Some(e) => Some(crate::embed::query(e, text).with_context(|| format!("embed {qid}"))?),
            None => None,
        };
        let hits = match &qvec {
            // The session leaves both candidate lists before fusion (as in the spike's
            // `runs_kf.py`), so its documents neither take ranks nor crowd others out.
            Some(q) => {
                let lex = outside(conn, session, depth.max(HYBRID_DEPTH), |n| {
                    search(conn, text, None, n)
                })?;
                fuse(conn, lex, Some(q), None, session, depth)?
            }
            None => outside(conn, session, depth, |n| search(conn, text, None, n))?,
        };
        let kept: Vec<String> = hits.into_iter().map(|h| h.doc).collect();
        for (i, doc) in kept.iter().enumerate() {
            out.push_str(&format!(
                "{qid} Q0 {doc} {} {} {method}\n",
                i + 1,
                depth - i
            ));
        }
    }
    Ok(out)
}

/// `as i64` would wrap a huge `--limit` negative, which SQLite reads as "no limit".
fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// A document by its local id (`o123`) or its uid (`7f3a9c21:o123`, `claude-mem:<db>:o5`).
pub fn get(conn: &Connection, doc: &str) -> Result<Option<Hit>> {
    let local: Option<String> = if doc.contains(':') {
        conn.query_row(
            "SELECT 'o' || id FROM observations WHERE uid = ?1
             UNION ALL SELECT 's' || id FROM summaries WHERE uid = ?1
             UNION ALL SELECT 'p' || id FROM prompts WHERE uid = ?1",
            params![doc],
            |r| r.get(0),
        )
        .optional()?
    } else {
        Some(doc.to_string())
    };
    let Some(local) = local else {
        return Ok(None);
    };
    // From the document's own table by id: `fts` keeps `doc` UNINDEXED, so a lookup there scans
    // the whole index (119 ms a document on the 178k-document evaluation store).
    let Some((table, Ok(id))) = local
        .split_at_checked(1)
        .map(|(t, id)| (t, id.parse::<i64>()))
    else {
        return Ok(None);
    };
    let from = match table {
        "o" => "SELECT 'o' || id AS doc, kind, repo, ts, title, body FROM observations",
        "s" => {
            "SELECT 's' || id AS doc, 'summary' AS kind, repo, ts, '' AS title, body FROM summaries"
        }
        "p" => {
            "SELECT 'p' || id AS doc, 'prompt' AS kind, repo, ts, '' AS title, body FROM prompts"
        }
        _ => return Ok(None),
    };
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM ({from} WHERE id = ?1)"),
            params![id],
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

/// One line of `body`, `width` characters around the passage with the most different `terms`
/// (case-insensitive; `terms` from [`terms`]).
pub fn snippet(body: &str, terms: &[String], width: usize) -> String {
    let flat = body.replace('\n', " ");
    let chars: Vec<char> = flat.chars().collect();
    let lower: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    // Every (char position, term) where a term occurs; the passage is the window that holds the
    // most different terms, so a hit found by a few rare trigrams shows them.
    let mut found: Vec<(usize, usize)> = Vec::new();
    for (i, t) in terms.iter().enumerate() {
        let t: Vec<char> = t.chars().map(fold).collect();
        if t.is_empty() || t.len() > lower.len() {
            continue;
        }
        found.extend(
            (0..=lower.len() - t.len())
                .filter(|&p| lower[p..p + t.len()] == t[..])
                .map(|p| (p, i)),
        );
    }
    found.sort_unstable();
    let mut at = found.first().map_or(0, |f| f.0);
    let mut best = 0;
    for (k, &(p, _)) in found.iter().enumerate() {
        let mut seen: Vec<usize> = found[k..]
            .iter()
            .take_while(|f| f.0 < p + width * 2 / 3)
            .map(|f| f.1)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() > best {
            (best, at) = (seen.len(), p);
        }
    }
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

    #[test]
    fn hybrid_fuses_meaning_with_words_knowledge_first() {
        let dir = home("hybrid");
        let mut conn = db::open(&dir).unwrap();
        seed(&mut conn);
        // o1..o3 and s1 are knowledge, p1 a prompt. Only o2 has the word; o1 and p1 point where
        // the query vector points.
        let unit = |pairs: &[(usize, f32)]| {
            let mut v = vec![0.0f32; crate::embed::DIM];
            for &(i, x) in pairs {
                v[i] = x;
            }
            v
        };
        for (doc, v) in [
            ("o1", unit(&[(0, 1.0)])),
            ("o2", unit(&[(0, 0.6), (1, 0.8)])),
            ("o3", unit(&[(2, 1.0)])),
            ("s1", unit(&[(0, -1.0)])),
            ("p1", unit(&[(0, 1.0)])),
        ] {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO embeddings(doc, embedder, text_sha, vec) VALUES(?1, ?2, '', ?3)",
                rusqlite::params![doc, crate::embed::EMBEDDER, bytes],
            )
            .unwrap();
        }
        crate::embed::index_pending(&mut conn).unwrap();
        let q = unit(&[(0, 1.0)]);
        let docs = |qvec: Option<&[f32]>, repo: Option<&str>, limit: usize| -> Vec<String> {
            let lex = search(&conn, "tokenizer", repo, 100).unwrap();
            fuse(&conn, lex, qvec, repo, None, limit)
                .unwrap()
                .into_iter()
                .map(|h| h.doc)
                .collect()
        };
        // o2: word (rank 1) + vector (rank 2) beats o1: vector only (rank 1); prompts come last.
        assert_eq!(docs(Some(&q), None, 10), ["o2", "o1", "o3", "s1", "p1"]);
        assert_eq!(docs(Some(&q), Some("/r"), 2), ["o2", "o1"]);
        assert_eq!(docs(Some(&q), Some("/elsewhere"), 10), Vec::<String>::new());
        assert_eq!(docs(Some(&q), None, 5_000).len(), 5);
        // An evaluation question's own session leaves the vector side before the cut.
        let near = |skip: Option<&str>, k: usize| {
            crate::embed::nearest(&conn, &q, None, false, skip, k).unwrap()
        };
        assert!(near(Some("s1"), 5).is_empty());
        assert_eq!(near(Some("elsewhere"), 1), ["o1"]);
        // Without a query vector the ranking is the full-text one.
        assert_eq!(docs(None, None, 10), ["o2"]);
        // Semantic search switched on but unusable (no account): full-text, not an error.
        let broken = crate::config::Embedding {
            provider: "workers-ai".into(),
            ..Default::default()
        };
        let hits: Vec<String> = find(&conn, &broken, "tokenizer", None, 10)
            .unwrap()
            .into_iter()
            .map(|h| h.doc)
            .collect();
        assert_eq!(hits, ["o2"]);
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

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
                    body: "coverage 50% done, ÄÖÜ İSTANBUL".into(),
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
            trec_run(&conn, queries, 50, None).unwrap(),
            "q1 Q0 o2 1 50 fts\nq1 Q0 p1 2 49 fts\n"
        );
        assert_eq!(
            trec_run(&conn, queries, 1, None).unwrap(),
            "q1 Q0 o2 1 1 fts\n"
        );
        assert!(trec_run(&conn, "{\"qid\":1,\"text\":\"x\"}", 5, None).is_err());
        assert!(trec_run(&conn, "{\"qid\":\"q 1\",\"text\":\"x\"}", 5, None).is_err());
        assert!(
            trec_run(
                &conn,
                "{\"qid\":\"q1\",\"text\":\"x\",\"session\":7}",
                5,
                None
            )
            .is_err()
        );
        // The question's own session is left out and the next hit moves up.
        let own = "{\"qid\":\"q1\",\"text\":\"Trigram\",\"session\":\"s1\"}\n";
        assert_eq!(trec_run(&conn, own, 1, None).unwrap(), "");
        let other = "{\"qid\":\"q1\",\"text\":\"Trigram\",\"session\":\"elsewhere\"}\n";
        assert_eq!(
            trec_run(&conn, other, 1, None).unwrap(),
            "q1 Q0 o2 1 1 fts\n"
        );
        db::upsert_session(&conn, "s2", "claude", "/r", "/r", 1_700_000_000_000).unwrap();
        db::insert_prompt(
            &conn,
            "s2",
            1_700_000_100_000,
            "trigram from another session",
        )
        .unwrap();
        assert_eq!(trec_run(&conn, own, 1, None).unwrap(), "q1 Q0 p2 1 1 fts\n");
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
        // Trigrams, ORed: English and Japanese alike, case-insensitive; a part that matches
        // nothing does not empty the result, and a sentence finds what shares its pieces.
        assert_eq!(docs("Trigram", None), vec!["o2", "p1"]);
        assert_eq!(docs("足して", None), vec!["p1"]);
        assert_eq!(docs("クエリ", None), vec!["o1"]);
        assert_eq!(docs("trigram \"quoted\"", None), vec!["o2", "p1"]);
        assert_eq!(docs("接続 trigram", None), vec!["o2", "p1"]);
        assert_eq!(docs("use trigram", None), vec!["o2", "p1"]);
        assert_eq!(docs("クエリの接続を調べてください", None), vec!["o1"]);
        assert_eq!(docs("éco ÄÖ", None), vec!["o3"]);
        // `İ` lowercases to two code points; the query keeps its spelling for SQLite to fold.
        assert_eq!(docs("İST", None), vec!["o3"]);
        // A query with no trigram falls back to literal LIKE terms, all required (ASCII case
        // folding); `%` and `_` are not wildcards.
        assert_eq!(docs("検索 要約", None), vec!["s1"]);
        assert_eq!(docs("接続", None), vec!["o1"]);
        assert_eq!(docs("db", None), vec!["o1"]);
        assert_eq!(docs("接続 db", None), vec!["o1"]);
        assert_eq!(docs("接続 要約", None), Vec::<String>::new());
        assert_eq!(docs("50%", None), vec!["o3"]);
        assert_eq!(docs("0%", None), vec!["o3"]);
        assert_eq!(docs("0_", None), Vec::<String>::new());
        assert_eq!(docs("", None), Vec::<String>::new());
        // Repository filter.
        assert_eq!(docs("tokenizer", Some("/r")), vec!["o2"]);
        assert_eq!(docs("trigram", Some("/other")), Vec::<String>::new());
        assert_eq!(search(&conn, "trigram", None, 0).unwrap().len(), 0);

        let h = get(&conn, "o1").unwrap().unwrap();
        let uid = format!("{}:o1", db::device_id(&conn).unwrap());
        assert_eq!(get(&conn, &uid).unwrap().unwrap().doc, "o1");
        assert!(get(&conn, "nobody:o1").unwrap().is_none());
        for bad in ["x1", "o", "oabc", ""] {
            assert!(get(&conn, bad).unwrap().is_none(), "{bad}");
        }
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
    fn trigrams_skip_hiragana_split_at_punctuation_and_stop_at_64() {
        assert_eq!(trigrams("検索をしてください。"), ["検索を", "索をし"]);
        assert_eq!(trigrams("abcd, abc"), ["abc", "bcd"]);
        assert_eq!(trigrams("HTTP http"), ["HTT", "TTP"]);
        assert_eq!(trigrams("iii ııı III"), ["iii", "ııı"]);
        assert_eq!(trigrams("db 接続"), Vec::<String>::new());
        let long: String = ('a'..='z').cycle().take(200).collect();
        assert_eq!(trigrams(&long).len(), 26);
        let many: String = (0..100).map(|i| format!("x{i:02} ")).collect();
        assert_eq!(trigrams(&many).len(), 64);
    }

    #[test]
    fn snippet_shows_the_passage_with_the_most_terms() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let body = "aaaaaaaaaa bbbbbbbbbb cccccccccc TARGET dddddddddd eeeeeeeeee";
        let s = snippet(body, &t(&["zzz", "target"]), 24);
        assert!(
            s.contains("TARGET") && s.starts_with('…') && s.ends_with('…'),
            "{s}"
        );
        assert_eq!(snippet("short\nline", &t(&["nothing"]), 40), "short line");
        assert_eq!(snippet("日本語の本文です", &t(&["本文"]), 4), "…の本文で…");
        // A common trigram early on loses to the passage where the rarer ones meet.
        let body = format!(
            "the start {} the trigram tokenizer indexes CJK",
            "x".repeat(200)
        );
        let s = snippet(&body, &terms("the trigram tokenizer"), 40);
        assert!(s.contains("trigram tokenizer"), "{s}");
    }
}
