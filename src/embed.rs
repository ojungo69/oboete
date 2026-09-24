//! Document vectors for semantic search (PR-D, docs/pr-d.md): bge-m3 on Workers AI, one normalized
//! fp32 vector per document in `embeddings` (the source), indexed as sign bits in `vec_docs`
//! (sharded by repository and by knowledge / prompt) for a Hamming search rescored in fp32.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{config, db, redact};

pub const DIM: usize = 1024;
/// The model, recorded with every vector: one vector space per store (proposal §2.3). Workers AI
/// and a local copy of the same weights share it (decision 5), so this names the model, not where
/// it ran.
pub const EMBEDDER: &str = "bge-m3";
/// `provider_calls` name, for the daily cap.
const CALLS: &str = "workers-ai-embed";
/// Workers AI takes at most 100 texts per request and counts every text as long as the longest
/// (texts × longest ≤ 60,000 tokens, PR-A2): texts of similar length go together, count × longest
/// ≤ 50,000 characters.
const BATCH: usize = 100;
const BATCH_CHARS: usize = 50_000;
/// The model cuts beyond 8,192 tokens (`truncate_inputs`); sending more is wasted bytes.
const MAX_CHARS: usize = 12_000;
/// A prompt is embedded by its opening (the spike's texts).
const PROMPT_CHARS: usize = 1_000;
/// Documents read per round of a backlog.
const PAGE: i64 = 2_000;
/// A batch of up to 100 texts; a search query waits for its vector (MCP budget p95 1.5 s).
const BATCH_TIMEOUT: Duration = Duration::from_secs(180);
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
/// One answer holds up to 100 × 1,024 floats as JSON (about 2 MB).
const MAX_RESPONSE_BYTES: u64 = 8 << 20;
/// Requests per observe run: observe holds its lock while embedding, and other sessions' summaries
/// wait behind it. A backlog (an import) drains over the following runs and days.
pub const PER_RUN: u32 = 20;

#[derive(Debug, Default, serde::Serialize)]
pub struct Stats {
    pub embedded: usize,
    pub indexed: usize,
    pub requests: u32,
}

/// Embed documents that have no vector yet (newest first) and index vectors that are not in
/// `vec_docs` (new ones, and those a re-key dropped). `max_requests` = None ignores the daily cap
/// (`oboete reindex`).
pub fn backlog(
    conn: &mut Connection,
    cfg: &config::Embedding,
    max_requests: Option<u32>,
) -> Result<Stats> {
    let (url, key) = endpoint(cfg)?;
    let cap = max_requests.map(|n| {
        n.min(
            cfg.daily_requests
                .saturating_sub(db::calls_today(conn, CALLS).unwrap_or(u32::MAX)),
        )
    });
    backlog_at(conn, &url, &key, cap)
}

/// The model's URL and the token.
fn endpoint(cfg: &config::Embedding) -> Result<(String, String)> {
    let account = cfg
        .account_id
        .as_deref()
        .ok_or_else(|| anyhow!("[embedding] account_id is not set"))?;
    let url =
        format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/@cf/baai/bge-m3");
    Ok((url, config::read_key(&cfg.key_file)?))
}

/// A search query's vector, gated like the documents. A search waits for it, so the call gets a
/// short timeout; the caller falls back to full-text search when it fails.
pub fn query(cfg: &config::Embedding, text: &str) -> Result<Vec<f32>> {
    let (url, key) = endpoint(cfg)?;
    let gated: String = redact::outbound(text).chars().take(MAX_CHARS).collect();
    anyhow::ensure!(!gated.trim().is_empty(), "empty query");
    let mut v = run_model(&url, &key, &[&gated], QUERY_TIMEOUT)?;
    Ok(v.remove(0))
}

/// Up to `k` documents of one shard (a repository or all; knowledge or prompts) nearest to `q`:
/// 4k candidates by Hamming distance on the sign bits, rescored by fp32 cosine from
/// `embeddings` (docs/pr-d.md: top-10 agreement 0.987 with the exact ranking). `skip_session`
/// (evaluation only) drops that session's documents before the cut to `k`.
pub fn nearest(
    conn: &Connection,
    q: &[f32],
    repo: Option<&str>,
    prompts: bool,
    skip_session: Option<&str>,
    k: usize,
) -> Result<Vec<String>> {
    let kind = if prompts { "p" } else { "k" };
    // sqlite-vec refuses k above 4,096 (a large `--limit`).
    let candidates = (4 * k).min(4_096) as i64;
    let mut sql = String::from(
        "SELECT doc FROM vec_docs WHERE embedding MATCH vec_bit(?1) AND k = ?2 AND kind = ?3",
    );
    if repo.is_some() {
        sql.push_str(" AND repo = ?4");
    }
    let mut stmt = conn.prepare(&sql)?;
    let docs: Vec<String> = match repo {
        Some(r) => stmt
            .query_map(params![bits(q), candidates, kind, r], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
        None => stmt
            .query_map(params![bits(q), candidates, kind], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
    };
    let mut get = conn.prepare("SELECT vec FROM embeddings WHERE doc = ?1")?;
    let mut scored = Vec::with_capacity(docs.len());
    for doc in docs {
        let Some(bytes) = get
            .query_row(params![doc], |r| r.get::<_, Vec<u8>>(0))
            .optional()?
        else {
            continue;
        };
        let dot: f32 = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .zip(q)
            .map(|(b, x)| f32::from_le_bytes(*b) * x)
            .sum();
        scored.push((dot, doc));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = Vec::with_capacity(k);
    for (_, doc) in scored {
        if out.len() == k {
            break;
        }
        if skip_session.is_none() || db::doc_session(conn, &doc)?.as_deref() != skip_session {
            out.push(doc);
        }
    }
    Ok(out)
}

fn backlog_at(conn: &mut Connection, url: &str, key: &str, cap: Option<u32>) -> Result<Stats> {
    let mut stats = Stats {
        indexed: index_pending(conn)?,
        ..Stats::default()
    };
    loop {
        let left = cap.map_or(u32::MAX, |n| n.saturating_sub(stats.requests));
        // A page of the newest documents at a time keeps memory flat on a large backlog.
        let limit = (i64::from(left) * BATCH as i64).min(PAGE);
        let mut todo = if left == 0 {
            Vec::new()
        } else {
            pending(conn, limit)?
        };
        if todo.is_empty() {
            return Ok(stats);
        }
        todo.sort_by_key(|(_, text)| text.chars().count());
        let before = stats.embedded;
        embed_page(conn, url, key, cap, &todo, &mut stats)?;
        if stats.embedded == before {
            return Ok(stats);
        }
    }
}

fn embed_page(
    conn: &mut Connection,
    url: &str,
    key: &str,
    cap: Option<u32>,
    todo: &[(String, String)],
    stats: &mut Stats,
) -> Result<()> {
    for batch in batches(todo) {
        if cap.is_some_and(|n| stats.requests >= n) {
            break;
        }
        let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
        let started = Instant::now();
        let result = run_model(url, key, &texts, BATCH_TIMEOUT);
        let ms = started.elapsed().as_millis() as i64;
        stats.requests += 1;
        let vecs = match result {
            Ok(v) => {
                db::record_call(conn, CALLS, "ok", ms, Some(&batch.len().to_string()))?;
                v
            }
            Err(e) => {
                db::record_call(conn, CALLS, "error", ms, Some(&format!("{e:#}")))?;
                return Err(e);
            }
        };
        stats.embedded += store(conn, batch, &vecs)?;
    }
    Ok(())
}

/// Documents without a vector from this model, newest first, as the gated text to embed: an
/// observation's kind and title over its body, a summary's body, a prompt's first 1,000
/// characters (the spike's texts). Cut after the gate: a secret across the cut is redacted whole.
fn pending(conn: &Connection, limit: i64) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT d.doc, d.text FROM (
           SELECT 'o' || id AS doc, kind || ': ' || title || char(10) || body AS text, ts
             FROM observations
           UNION ALL SELECT 's' || id, body, ts FROM summaries
           UNION ALL SELECT 'p' || id, body, ts FROM prompts
         ) d LEFT JOIN embeddings e ON e.doc = d.doc AND e.embedder = ?1
         WHERE e.doc IS NULL AND trim(d.text) != '' ORDER BY d.ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![EMBEDDER, limit], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    rows.map(|r| {
        let (doc, text) = r?;
        let keep = if doc.starts_with('p') {
            PROMPT_CHARS
        } else {
            MAX_CHARS
        };
        let gated: String = redact::outbound(&text).chars().take(keep).collect();
        Ok((doc, gated))
    })
    .collect()
}

/// Requests of at most 100 texts whose count × longest stays under `BATCH_CHARS` (`todo` sorted by
/// length, so each batch holds texts of similar length). A text longer than that goes alone.
fn batches(todo: &[(String, String)]) -> Vec<&[(String, String)]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..todo.len() {
        let longest = todo[i].1.chars().count();
        if i > start && (i - start == BATCH || (i - start + 1) * longest > BATCH_CHARS) {
            out.push(&todo[start..i]);
            start = i;
        }
    }
    if start < todo.len() {
        out.push(&todo[start..]);
    }
    out
}

/// One Workers AI call: the texts' vectors, in order.
fn run_model(url: &str, key: &str, texts: &[&str], timeout: Duration) -> Result<Vec<Vec<f32>>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(concat!("oboete/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .post(url)
        .header("Authorization", &format!("Bearer {key}"))
        .send_json(json!({"text": texts, "truncate_inputs": true}))
        .map_err(|e| anyhow!("workers ai: {e}"))?;
    let status = resp.status().as_u16();
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(resp.body_mut().as_reader(), MAX_RESPONSE_BYTES + 1),
        &mut raw,
    )
    .context("workers ai: read body")?;
    anyhow::ensure!(
        raw.len() as u64 <= MAX_RESPONSE_BYTES,
        "workers ai: response larger than {MAX_RESPONSE_BYTES} bytes"
    );
    let text = String::from_utf8_lossy(&raw);
    anyhow::ensure!(
        status == 200,
        "workers ai: http {status}: {}",
        text.chars().take(300).collect::<String>()
    );
    let v: Value = serde_json::from_str(&text).context("workers ai: response is not JSON")?;
    vectors(&v, texts.len())
}

/// `result.data` of a Workers AI answer as `n` unit vectors of finite numbers.
fn vectors(v: &Value, n: usize) -> Result<Vec<Vec<f32>>> {
    let data = v["result"]["data"]
        .as_array()
        .ok_or_else(|| anyhow!("workers ai: no result.data"))?;
    anyhow::ensure!(
        data.len() == n,
        "workers ai: {} vectors for {n} texts",
        data.len()
    );
    data.iter()
        .map(|row| {
            let row = row
                .as_array()
                .filter(|r| r.len() == DIM)
                .ok_or_else(|| anyhow!("workers ai: a vector is not {DIM} numbers"))?;
            let mut vec = row
                .iter()
                .map(|x| {
                    x.as_f64()
                        .map(|x| x as f32)
                        .filter(|x| x.is_finite())
                        .ok_or_else(|| anyhow!("workers ai: a coordinate is not a finite number"))
                })
                .collect::<Result<Vec<f32>>>()?;
            let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            anyhow::ensure!(
                norm.is_finite() && norm > 0.0,
                "workers ai: a zero or overflowing vector"
            );
            vec.iter_mut().for_each(|x| *x /= norm);
            Ok(vec)
        })
        .collect()
}

/// Write one batch's vectors and index them, skipping documents deleted meanwhile. The repository
/// is read here, inside the transaction, so a re-key in between cannot index an old key.
fn store(conn: &mut Connection, batch: &[(String, String)], vecs: &[Vec<f32>]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut n = 0;
    for ((doc, text), vec) in batch.iter().zip(vecs) {
        let Some(repo) = repo_of(&tx, doc)? else {
            continue;
        };
        let bytes: Vec<u8> = vec.iter().flat_map(|x| x.to_le_bytes()).collect();
        let sha = format!("{:x}", Sha256::digest(text.as_bytes()));
        tx.execute(
            "INSERT OR REPLACE INTO embeddings(doc, embedder, text_sha, vec, indexed)
             VALUES(?1, ?2, ?3, ?4, 1)",
            params![doc, EMBEDDER, sha, bytes],
        )?;
        index(&tx, doc, &repo, vec)?;
        n += 1;
    }
    tx.commit()?;
    Ok(n)
}

/// Index the vectors `vec_docs` does not have (a re-key drops a repository's rows; `reindex`
/// drops them all), a page per transaction so a whole store's vectors are never in memory at once.
pub(crate) fn index_pending(conn: &mut Connection) -> Result<usize> {
    let mut n = 0;
    loop {
        let tx = conn.transaction()?;
        let rows: Vec<(String, Vec<u8>)> = tx
            .prepare(
                "SELECT doc, vec FROM embeddings WHERE indexed = 0 AND embedder = ?1 LIMIT ?2",
            )?
            .query_map(params![EMBEDDER, PAGE], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        if rows.is_empty() {
            return Ok(n);
        }
        for (doc, bytes) in rows {
            // A vector whose document is gone is dropped with it (no row stays unindexed forever).
            let Some(repo) = repo_of(&tx, &doc)? else {
                tx.execute("DELETE FROM embeddings WHERE doc = ?1", params![doc])?;
                continue;
            };
            let vec: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            index(&tx, &doc, &repo, &vec)?;
            tx.execute(
                "UPDATE embeddings SET indexed = 1 WHERE doc = ?1",
                params![doc],
            )?;
            n += 1;
        }
        tx.commit()?;
    }
}

fn index(conn: &Connection, doc: &str, repo: &str, vec: &[f32]) -> Result<()> {
    let kind = if doc.starts_with('p') { "p" } else { "k" };
    conn.execute("DELETE FROM vec_docs WHERE doc = ?1", params![doc])?;
    conn.execute(
        "INSERT INTO vec_docs(doc, repo, kind, embedding) VALUES(?1, ?2, ?3, vec_bit(?4))",
        params![doc, repo, kind, bits(vec)],
    )?;
    Ok(())
}

/// Sign bits, most significant bit first in each byte (as the spike's `np.packbits`).
pub fn bits(vec: &[f32]) -> Vec<u8> {
    vec.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, x)| if *x > 0.0 { b | (0x80 >> i) } else { b })
        })
        .collect()
}

/// The repository a document is filed under, or None once it is deleted.
fn repo_of(conn: &Connection, doc: &str) -> Result<Option<String>> {
    let table = match doc.split_at_checked(1) {
        Some(("o", _)) => "observations",
        Some(("s", _)) => "summaries",
        Some(("p", _)) => "prompts",
        _ => return Ok(None),
    };
    let Ok(id) = doc[1..].parse::<i64>() else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            &format!("SELECT repo FROM {table} WHERE id = ?1"),
            params![id],
            |r| r.get(0),
        )
        .optional()?)
}

/// `oboete reindex`: rebuild `vec_docs` from the stored vectors and embed every document that has
/// none, without the daily cap.
pub fn reindex(home: &Path) -> Result<Stats> {
    let cfg = config::load(home)?;
    anyhow::ensure!(
        cfg.embedding.provider == "workers-ai",
        "[embedding] provider is \"{}\"; reindex needs \"workers-ai\"",
        cfg.embedding.provider
    );
    // The account and the token are checked before the index is dropped: a failed reindex must
    // not leave search without one.
    anyhow::ensure!(
        cfg.embedding.account_id.is_some(),
        "[embedding] account_id is not set"
    );
    config::read_key(&cfg.embedding.key_file)?;
    // A detached observe would embed the same documents at the same time: wait for it.
    let lock = std::fs::File::create(home.join("observe.lock"))?;
    lock.lock()?;
    let mut conn = db::open(home)?;
    conn.execute_batch("DELETE FROM vec_docs; UPDATE embeddings SET indexed = 0;")?;
    backlog(&mut conn, &cfg.embedding, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GitHub-token-shaped fake, assembled here so secret scanners do not flag the source.
    fn fake_token() -> String {
        ["gh", "p_q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"].concat()
    }

    #[test]
    fn batches_stay_under_the_request_limits() {
        let doc = |n: usize| (String::new(), "x".repeat(n));
        let mut todo: Vec<_> = (0..250).map(|_| doc(10)).collect();
        todo.extend((0..30).map(|_| doc(2_000)));
        todo.push(doc(60_000));
        let got = batches(&todo);
        for b in &got {
            let longest = b.iter().map(|(_, t)| t.len()).max().unwrap();
            assert!(b.len() <= BATCH);
            assert!(
                b.len() == 1 || b.len() * longest <= BATCH_CHARS,
                "{}",
                b.len()
            );
        }
        assert_eq!(got.iter().map(|b| b.len()).sum::<usize>(), todo.len());
        assert_eq!(got.last().unwrap().len(), 1);
    }

    #[test]
    fn answers_with_bad_coordinates_are_refused() {
        let row = |x: Value| {
            let mut r: Vec<Value> = vec![json!(0.5); DIM];
            r[3] = x;
            json!({"result": {"data": [r]}})
        };
        assert!(vectors(&row(json!(0.1)), 1).is_ok());
        assert!(vectors(&row(json!("x")), 1).is_err());
        assert!(vectors(&row(json!(1e300)), 1).is_err());
        assert!(vectors(&json!({"result": {"data": [vec![0.0; DIM]]}}), 1).is_err());
        assert!(vectors(&row(json!(0.1)), 2).is_err());
    }

    #[test]
    fn bits_are_signs_msb_first() {
        let mut v = vec![-1.0f32; 16];
        v[0] = 0.5;
        v[9] = 0.1;
        assert_eq!(bits(&v), [0x80, 0x40]);
    }

    /// Localhost server answering each request with one vector per text (`[i, 1, 0, …]` for the
    /// text's order in the request), counting requests.
    fn model_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/run", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let mut conn = conn.unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 65536];
                let body = loop {
                    let n = conn.read(&mut buf).unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text
                            .to_lowercase()
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:").map(str::to_string))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= end + 4 + len {
                            break req[end + 4..end + 4 + len].to_vec();
                        }
                    }
                    if n == 0 {
                        break Vec::new();
                    }
                };
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let texts = serde_json::from_slice::<Value>(&body).unwrap()["text"]
                    .as_array()
                    .unwrap()
                    .len();
                let data: Vec<Vec<f32>> = (0..texts)
                    .map(|i| {
                        let mut v = vec![0.0f32; DIM];
                        v[0] = i as f32 + 1.0;
                        v[1] = 1.0;
                        v
                    })
                    .collect();
                let out = json!({"result": {"shape": [texts, DIM], "data": data}, "success": true})
                    .to_string();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    out.len()
                );
                conn.write_all(head.as_bytes()).unwrap();
                conn.write_all(out.as_bytes()).unwrap();
            }
        });
        (url, hits)
    }

    #[test]
    fn backlog_embeds_once_indexes_by_repo_and_follows_deletes_and_rekeys() {
        let dir = std::env::temp_dir().join(format!("oboete-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo_dir = dir.join("r");
        std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
        let path_key = crate::repo::key(&repo_dir);
        let mut conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "s", "claude", &path_key, &path_key, 1).unwrap();
        db::insert_prompt(&conn, "s", 1, &format!("token {} here", fake_token())).unwrap();
        db::insert_prompt(&conn, "s", 2, "a second prompt").unwrap();
        let (url, hits) = model_server();

        let stats = backlog_at(&mut conn, &url, "k", Some(5)).unwrap();
        assert_eq!((stats.embedded, stats.requests), (2, 1));
        let count = |conn: &Connection, sql: &str| -> i64 {
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        };
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM embeddings WHERE indexed = 1"),
            2
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM vec_docs"), 2);
        // Stored vectors are unit length.
        let first: Vec<u8> = conn
            .query_row("SELECT vec FROM embeddings WHERE doc = 'p1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let norm: f32 = first
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b).powi(2))
            .sum();
        assert!((norm - 1.0).abs() < 1e-5);
        // A second run has nothing to embed and makes no request.
        let stats = backlog_at(&mut conn, &url, "k", Some(5)).unwrap();
        assert_eq!((stats.embedded, stats.requests), (0, 0));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The index answers within the repository's knowledge / prompt shard.
        let knn = |conn: &Connection, repo: &str| -> Vec<String> {
            let q = bits(&{
                let mut v = vec![0.0f32; DIM];
                v[0] = 1.0;
                v[1] = 1.0;
                v
            });
            conn.prepare(
                "SELECT doc FROM vec_docs WHERE embedding MATCH vec_bit(?1) AND k = 5
                 AND repo = ?2 AND kind = 'p'",
            )
            .unwrap()
            .query_map(params![q, repo], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        assert_eq!(knn(&conn, &path_key).len(), 2);

        // The repository gets an origin: the re-key drops its index rows, the next run indexes
        // them under the new key without calling the model.
        std::fs::write(
            repo_dir.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/o/r\n",
        )
        .unwrap();
        assert_eq!(db::rekey_paths(&mut conn).unwrap(), 1);
        assert!(knn(&conn, &path_key).is_empty());
        let stats = backlog_at(&mut conn, &url, "k", Some(5)).unwrap();
        assert_eq!((stats.indexed, stats.requests), (2, 0));
        assert_eq!(knn(&conn, "github.com/o/r").len(), 2);

        // Deleting a document deletes its vector and its index row.
        assert!(db::delete_doc(&mut conn, "p1").unwrap());
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM embeddings"), 1);
        assert_eq!(knn(&conn, "github.com/o/r"), ["p2"]);

        // The daily cap: no request when none is left.
        db::insert_prompt(&conn, "s", 3, "a third prompt").unwrap();
        let stats = backlog_at(&mut conn, &url, "k", Some(0)).unwrap();
        assert_eq!((stats.embedded, stats.requests), (0, 0));
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reindex_keeps_the_index_when_the_token_is_missing_and_rebuilds_it_in_pages() {
        let dir = std::env::temp_dir().join(format!("oboete-embed-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "s", "claude", "/r", "/r", 1).unwrap();
        // More vectors than one page, stored as if embedded earlier and not yet indexed.
        let n = PAGE as usize + 500;
        let tx = conn.transaction().unwrap();
        let mut v = vec![0.0f32; DIM];
        v[0] = 1.0;
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        for i in 1..=n {
            db::insert_prompt(&tx, "s", i as i64, "p").unwrap();
            tx.execute(
                "INSERT INTO embeddings(doc, embedder, text_sha, vec) VALUES(?1, ?2, '', ?3)",
                params![format!("p{i}"), EMBEDDER, bytes],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(index_pending(&mut conn).unwrap(), n);
        let indexed: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(indexed, n as i64);
        drop(conn);

        std::fs::write(
            dir.join("config.toml"),
            format!(
                "[embedding]\nprovider = \"workers-ai\"\naccount_id = \"a\"\nkey_file = \"{}\"\n",
                dir.join("missing-key.md").display()
            ),
        )
        .unwrap();
        assert!(reindex(&dir).is_err());
        let conn = db::open(&dir).unwrap();
        let still: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still, n as i64);
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pending_texts_are_gated() {
        let dir = std::env::temp_dir().join(format!("oboete-embed-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "s", "claude", "/r", "/r", 1).unwrap();
        db::insert_prompt(&conn, "s", 1, &format!("token {} here", fake_token())).unwrap();
        // A token across the 1,000-character cut is redacted whole, not cut first.
        let long = format!("{} {} tail", "x".repeat(975), fake_token());
        db::insert_prompt(&conn, "s", 2, &long).unwrap();
        let todo = pending(&conn, 10).unwrap();
        assert_eq!(todo.len(), 2);
        for (_, text) in &todo {
            assert!(!text.contains(&fake_token()[..20]), "{text}");
            assert!(text.chars().count() <= 1_000);
        }
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }
}
