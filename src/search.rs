//! Search over what is stored. One FTS5 trigram table (`fts`) keeps a copy of every
//! observation, summary and prompt, so a query is one SQL statement and CJK text is indexed by
//! character. A query is cut into character trigrams, ORed and ranked by bm25 (Unicode case
//! folding), so a Japanese sentence or a question in the developer's own words still finds the
//! documents that share the most of its rarer pieces (PR-E0, measured in `docs/pr-e0.md`). A
//! query too short for any trigram falls back to literal LIKE terms, all required (ASCII case
//! folding only), which is a scan the small tables can afford.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

pub mod b;

/// The query's trigrams: each run between whitespace and punctuation gives its overlapping
/// three-character pieces, except all-hiragana ones (particles and verb endings match almost
/// every Japanese document). Deduplicated, at most 64 so a pasted page stays one quick query.
// ponytail: every trigram is ORed, so a common one scans a long posting list (180k documents:
// p50 0.3 s, p95 0.8 s). Fine for MCP and the viewer; the injection hook (PR-F, 300 ms) should
// keep only the rarest trigrams (measured in docs/pr-e0.md).
pub(crate) fn trigrams(query: &str) -> Vec<String> {
    trigrams_upto(query, 64)
}

/// `trigrams` up to `cap` of them.
pub(crate) fn trigrams_upto(query: &str, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for run in runs(query) {
        let chars: Vec<char> = run.chars().collect();
        for w in chars.windows(3) {
            // The index folds case, so `HTTP` and `http` are one piece (else bm25 counts it twice).
            // The query keeps its spelling: SQLite folds it as it folded the index, and a char
            // whose lowercase is longer (`İ`) would no longer be one trigram.
            let folded: String = w.iter().map(|&c| fold(c)).collect();
            if out.len() < cap && !w.iter().all(hiragana) && seen.insert(folded) {
                out.push(w.iter().collect());
            }
        }
    }
    out
}

/// A query's runs: what lies between whitespace and punctuation. A control character ends one
/// too: FTS5 reads a query as a C string and stops at a NUL.
fn runs(query: &str) -> impl Iterator<Item = &str> {
    const SEPARATORS: &str = "、。，．,.!?！？「」『』()（）[]{}:;：；\"'`<>";
    query.split(|c: char| c.is_whitespace() || c.is_control() || SEPARATORS.contains(c))
}

fn hiragana(c: &char) -> bool {
    ('\u{3040}'..='\u{309f}').contains(c)
}

/// The short words a mixed query's order counts, at most this many: SQLite refuses an expression
/// 1,000 deep, and a pasted page is still one quick query.
const SHORT_WORDS: usize = 8;

/// A mixed query's short words: its runs of two characters, each once (ASCII case folded, as
/// `LIKE` folds it), the first `SHORT_WORDS`. Not one of ASCII letters only or of hiragana only:
/// as a substring it says nothing ("is" in "this", こと).
fn short_words(query: &str) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    runs(query)
        .filter(|t| t.chars().count() == 2)
        .filter(|t| !t.chars().all(|c| c.is_ascii_alphabetic()) && !t.chars().all(|c| hiragana(&c)))
        .filter(|t| seen.insert(t.to_ascii_lowercase()))
        .take(SHORT_WORDS)
        .collect()
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

type QueryClauses = (Vec<String>, Vec<Value>, String, Vec<Value>);

/// How many of a mixed query's hits its short words reorder: the best by bm25. A `LIKE` reads a
/// hit's text, and a trigram query matches widely: over every hit it took 24 s where the rank
/// alone took 0.2 s (325,000 records, 151,000 of them matched). A hit ranked below these keeps
/// its place.
pub(crate) const POOL: usize = 500;

/// What a query matches on: its trigrams ORed against the FTS5 table `fts`, or, for a query too
/// short for a trigram, each word as a `LIKE` on any of the `like` columns. Short words in a
/// mixed query (同期, M5) boost only MATCH hits: the hits that hold more of them come first. The
/// WHERE clauses and arguments, then the ORDER BY prefix and its arguments; `None` when nothing is
/// left.
fn query_clauses(query: &str, fts: &str, like: &[&str]) -> Option<QueryClauses> {
    let grams = trigrams(query);
    let short: Vec<&str> = if grams.is_empty() {
        query.split_whitespace().collect()
    } else {
        short_words(query)
    };
    if grams.is_empty() && short.is_empty() {
        return None;
    }
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if !grams.is_empty() {
        clauses.push(format!("{fts} MATCH ?"));
        args.push(Value::Text(matching(&grams)));
    }
    let mut short_clauses = Vec::new();
    let mut short_args = Vec::new();
    for t in &short {
        let pattern = format!(
            "%{}%",
            t.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let any: Vec<String> = like
            .iter()
            .map(|c| format!("{c} LIKE ? ESCAPE '\\'"))
            .collect();
        short_clauses.push(format!("({})", any.join(" OR ")));
        short_args.extend(like.iter().map(|_| Value::Text(pattern.clone())));
    }
    let (order, order_args) = if grams.is_empty() {
        clauses.extend(short_clauses);
        args.extend(short_args);
        (String::new(), Vec::new())
    } else if short_clauses.is_empty() {
        ("rank, ".into(), Vec::new())
    } else {
        (
            format!("{} DESC, rank, ", short_clauses.join(" + ")),
            short_args,
        )
    };
    Some((clauses, args, order, order_args))
}

/// Each trigram as an FTS5 string (quotes doubled), ORed.
fn matching(grams: &[String]) -> String {
    grams
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// `as i64` would wrap a huge `--limit` negative, which SQLite reads as "no limit".
fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// A hit in Design B's raw index: the record it came from, and a line of its text.
#[derive(Debug)]
pub struct RawHit {
    pub device: String,
    pub seq: i64,
    pub kind: String,
    /// Unix ms.
    pub ts: i64,
    pub repo: Option<String>,
    pub snippet: String,
}

/// The raw records `query` finds on `home`'s stores, as B's search reads them.
#[cfg(test)]
pub fn raw(
    home: &std::path::Path,
    query: &str,
    repo: Option<&str>,
    limit: usize,
) -> Result<Vec<RawHit>> {
    // raw.db first: its shared hold on raw.lock keeps a restore from swapping raw.db and moving
    // knowledge.db aside while this search reads them (Task 8).
    let raw = if crate::raw::exists(home) {
        Some(crate::raw::open(home)?)
    } else {
        None
    };
    let k = crate::knowledge::open(home)?;
    let keys = raw_order(
        raw.as_ref(),
        &k,
        query,
        repo,
        (None, None),
        None,
        None,
        limit,
    )?;
    raw_rows(raw.as_ref(), &k, &keys, query)
}

/// MUST-M12: `since` and `until` (unix ms, `until` exclusive) on `column`, each leg's documents by
/// their own time.
fn within(
    clauses: &mut Vec<String>,
    args: &mut Vec<Value>,
    column: &str,
    (since, until): (Option<i64>, Option<i64>),
) {
    if let Some(t) = since {
        clauses.push(format!("{column} >= ?"));
        args.push(Value::Integer(t));
    }
    if let Some(t) = until {
        clauses.push(format!("{column} < ?"));
        args.push(Value::Integer(t));
    }
}

/// Search the none tier's index (`raw_fts` in knowledge.db, milestone 2 Task 6) the way
/// [`search`] searches v1's: trigrams ORed and ranked by bm25, or literal terms (all required)
/// for a query too short for a trigram. `repo = None` searches every repository; `span` is
/// `within`'s; `skip_session`, an evaluation's, leaves that session's records out (one with no
/// session stays); `kind`, a clause on `d.kind` with its arguments, keeps those kinds alone (Q5,
/// before the limit). `raw` is `None` for a home with no raw.db. The records come as keys
/// (`device:seq`), the best first, through the tombstone filter: `raw_rows` reads them and gates
/// their snippets, after a search's query call is back (Task 5), with the rules of that moment.
/// The index keeps no copy of the text (#317): the words too short for a trigram are looked for
/// in each record's text as raw.db holds it now (`fts::texts`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn raw_order(
    raw: Option<&crate::raw::Raw>,
    k: &Connection,
    query: &str,
    repo: Option<&str>,
    span: (Option<i64>, Option<i64>),
    skip_session: Option<&str>,
    kind: Option<(String, Vec<Value>)>,
    limit: usize,
) -> Result<Vec<String>> {
    crate::consumer::fts::schema(k)?;
    let grams = trigrams(query);
    let short: Vec<&str> = if grams.is_empty() {
        query.split_whitespace().collect()
    } else {
        short_words(query)
    };
    if grams.is_empty() && short.is_empty() {
        return Ok(Vec::new());
    }
    // Tool output is not searched (`fts::indexed`): a home indexed before keeps its rows until
    // `oboete rebuild`, and the scan of `holding_every` reads none of it.
    let (mut clauses, mut args) = (vec!["d.kind <> 'tool'".to_owned()], Vec::new());
    if let Some(r) = repo {
        clauses.push("d.repo = ?".into());
        args.push(Value::Text(r.to_string()));
    }
    within(&mut clauses, &mut args, "d.ts", span);
    if let Some(s) = skip_session {
        clauses.push("COALESCE(d.session, '') <> ?".into());
        args.push(Value::Text(s.to_owned()));
    }
    if let Some((clause, values)) = kind {
        clauses.push(clause);
        args.extend(values);
    }
    let raw_db = fts_seen(raw, k)?;
    let before = hidden(&raw_db)?.len();
    // Enough rows that the hidden ones cannot take the place of visible ones.
    let rows = limit.saturating_add(before);
    let rows = match raw {
        _ if !grams.is_empty() => {
            clauses.push("raw_fts MATCH ?".into());
            args.push(Value::Text(matching(&grams)));
            ranked(raw, k, &short, &clauses.join(" AND "), args, rows)?
        }
        Some(raw) => holding_every(raw, k, &short, &clauses.join(" AND "), args, rows)?,
        None => Vec::new(),
    };
    let pending = hidden(&raw_db)?;
    Ok(rows
        .into_iter()
        .filter(|(device, seq)| !pending.contains(&(device.clone(), *seq)))
        .take(limit)
        .map(|(device, seq)| format!("{device}:{seq}"))
        .collect())
}

/// The trigram hits `filter` keeps, at most `rows`, by rank. Short words in the query (同期, M5)
/// reorder the best `POOL`: the ones whose text holds more of them first. A hit below them keeps
/// its place, however many are asked for (Codex on #360).
fn ranked(
    raw: Option<&crate::raw::Raw>,
    k: &Connection,
    short: &[&str],
    filter: &str,
    mut args: Vec<Value>,
    rows: usize,
) -> Result<Vec<(String, i64)>> {
    let query = |sql: &str, args: Vec<Value>| -> Result<Vec<(String, i64)>> {
        Ok(k.prepare(sql)?
            .query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    };
    let sql = format!(
        "SELECT d.device, d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid
         WHERE {filter} ORDER BY rank, d.ts DESC, d.rowid LIMIT ? OFFSET ?"
    );
    let (Some(raw), false) = (raw, short.is_empty()) else {
        args.extend([Value::Integer(sql_limit(rows)), Value::Integer(0)]);
        return query(&sql, args);
    };
    let mut pool = args.clone();
    pool.extend([Value::Integer(sql_limit(POOL)), Value::Integer(0)]);
    let mut found = query(&sql, pool)?;
    // As SQLite's `LIKE '%word%'` finds a word: ASCII letters in either case.
    let short: Vec<String> = short.iter().map(|w| w.to_ascii_lowercase()).collect();
    let mut held = std::collections::HashMap::new();
    for (device, seqs) in by_device(&found) {
        for (seq, mut text) in crate::consumer::fts::texts(raw, device, &seqs)? {
            text.make_ascii_lowercase();
            let n = short.iter().filter(|w| text.contains(w.as_str())).count();
            held.insert((device.to_owned(), seq), n);
        }
    }
    // Stable: within a count, the pool's order (rank, then the newest).
    found.sort_by_key(|key| std::cmp::Reverse(held.get(key).copied().unwrap_or(0)));
    found.truncate(rows.min(POOL));
    if rows > POOL {
        args.extend([
            Value::Integer(sql_limit(rows - POOL)),
            Value::Integer(sql_limit(POOL)),
        ]);
        found.extend(query(&sql, args)?);
    }
    Ok(found)
}

/// A query with no trigram: the newest records `filter` keeps whose text holds every word, at
/// most `rows`, each page of them read back from raw.db until enough are found.
fn holding_every(
    raw: &crate::raw::Raw,
    k: &Connection,
    words: &[&str],
    filter: &str,
    args: Vec<Value>,
    rows: usize,
) -> Result<Vec<(String, i64)>> {
    const PAGE: usize = 256;
    let mut st = k.prepare(&format!(
        "SELECT d.device, d.seq FROM raw_docs d WHERE {filter} ORDER BY d.ts DESC, d.rowid"
    ))?;
    let mut keys = st.query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut found = Vec::new();
    while found.len() < rows {
        let page = keys
            .by_ref()
            .take(PAGE)
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;
        if page.is_empty() {
            break;
        }
        let mut held = std::collections::HashSet::new();
        for (device, seqs) in by_device(&page) {
            for seq in crate::consumer::fts::holding(raw, device, &seqs, words)? {
                held.insert((device.to_owned(), seq));
            }
        }
        found.extend(page.into_iter().filter(|key| held.contains(key)));
    }
    found.truncate(rows);
    Ok(found)
}

/// `keys`' seqs by device, each device once, in the order it first comes.
fn by_device(keys: &[(String, i64)]) -> Vec<(&str, Vec<i64>)> {
    let mut out: Vec<(&str, Vec<i64>)> = Vec::new();
    for (device, seq) in keys {
        match out.iter_mut().find(|(d, _)| d == device) {
            Some((_, seqs)) => seqs.push(*seq),
            None => out.push((device, vec![*seq])),
        }
    }
    out
}

/// D8: a tombstone the index has not reached yet hides its target, so no search shows what raw
/// already hides. The fts consumer's checkpoint on each device partition, read before the index,
/// and the tombstones past it read after: one that commits while the query runs is still seen
/// (one the worker applies in between only hides more). Every partition counts: a copied home
/// keeps its records, and their tombstones, under the old id (#83).
type Seen<'a> = Option<(&'a crate::raw::Raw, Vec<(i64, String)>)>;

fn fts_seen<'a>(raw: Option<&'a crate::raw::Raw>, k: &Connection) -> Result<Seen<'a>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut devices = raw.devices()?;
    // This device's own partition too while it is still empty (a home copied a moment ago).
    if !devices.iter().any(|d| d == raw.device()) {
        devices.push(raw.device().to_owned());
    }
    let mut ats = Vec::new();
    for d in devices {
        ats.push((crate::knowledge::checkpoint::get(k, "fts", &d)?, d));
    }
    Ok(Some((raw, ats)))
}

/// The records the tombstones past `seen`'s checkpoints hide.
fn hidden(seen: &Seen) -> Result<std::collections::HashSet<(String, i64)>> {
    let mut out = std::collections::HashSet::new();
    if let Some((raw, ats)) = seen {
        for (at, d) in ats {
            out.extend(raw.tombstones_after(d, *at)?);
        }
    }
    Ok(out)
}

/// The raw hits `keys` (`device:seq`) name, in their order, their text read from raw.db as it is
/// now (#317) and any hidden by a tombstone past the index left out, each snippet gated (the
/// egress rules of now) before it is cut.
pub(crate) fn raw_rows(
    raw: Option<&crate::raw::Raw>,
    k: &Connection,
    keys: &[String],
    query: &str,
) -> Result<Vec<RawHit>> {
    let seen = fts_seen(raw, k)?;
    let terms = terms(query);
    let mut out = Vec::new();
    for key in keys {
        let Some((device, seq)) = key.rsplit_once(':') else {
            continue;
        };
        let seq: i64 = seq.parse()?;
        let Some(raw) = raw else {
            continue;
        };
        let Some((_, text)) = crate::consumer::fts::texts(raw, device, &[seq])?.pop() else {
            continue;
        };
        let row = k
            .query_row(
                "SELECT kind, ts, repo FROM raw_docs WHERE device = ?1 AND seq = ?2
                   AND kind <> 'tool'",
                params![device, seq],
                |r| {
                    Ok(RawHit {
                        device: device.to_owned(),
                        seq,
                        kind: r.get(0)?,
                        ts: r.get(1)?,
                        repo: r.get(2)?,
                        snippet: snippet(&crate::redact::outbound_lines(&text), &terms, b::WIDTH),
                    })
                },
            )
            .optional()?;
        out.extend(row);
    }
    let hidden = hidden(&seen)?;
    out.retain(|h| !hidden.contains(&(h.device.clone(), h.seq)));
    Ok(out)
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
    fn raw_search_finds_events_by_a_two_character_japanese_query_and_an_english_word() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        let mut ja = crate::raw::test_event(r#"{"prompt":"同期の設計を見直して"}"#);
        ja.repo = Some("github.com/o/a".into());
        raw.append(&ja).unwrap();
        let mut en = crate::raw::test_event(r#"{"assistant":"the lease worker panicked"}"#);
        en.kind = "reply".into();
        en.repo = Some("github.com/o/b".into());
        raw.append(&en).unwrap();
        let mut k = crate::knowledge::open(p).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        let device = raw.device().to_owned();

        let hits = raw_search(p, "設計", None);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].device.as_str(), hits[0].seq), (device.as_str(), 1));
        assert!(hits[0].snippet.contains("設計"), "{}", hits[0].snippet);
        let hits = raw_search(p, "worker panicked", None);
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), [2]);
        assert_eq!(hits[0].kind, "reply");
        // Keys are not text: "prompt" and "assistant" are field names here.
        assert!(raw_search(p, "prompt", None).is_empty());
        assert!(raw_search(p, "assistant", None).is_empty());
        // This repository unless --all.
        assert!(raw_search(p, "設計", Some("github.com/o/b")).is_empty());
        assert_eq!(raw_search(p, "設計", Some("github.com/o/a")).len(), 1);
        let repos: Vec<String> = k
            .prepare("SELECT repo FROM session_repos ORDER BY repo")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(repos, ["github.com/o/a", "github.com/o/b"]);
    }

    #[test]
    fn a_short_japanese_word_boosts_raw_hits_beside_a_long_word() {
        for (both_at, long_at) in [(1_000, 2_000), (2_000, 1_000)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            let mut store = crate::raw::open(p).unwrap();
            for (ts, body) in [
                (both_at, "設計 worker"),
                (long_at, "worker"),
                (3_000, "設計"),
            ] {
                store
                    .append(&crate::raw::Event {
                        ts,
                        repo: Some("github.com/o/r".into()),
                        ..crate::raw::test_event(body)
                    })
                    .unwrap();
            }
            crate::worker::run_once(p).unwrap();
            let seqs = |text| -> Vec<i64> {
                raw_search(p, text, Some("github.com/o/r"))
                    .iter()
                    .map(|h| h.seq)
                    .collect()
            };
            assert_eq!(seqs("worker"), [2, 1]);
            assert_eq!(seqs("worker absent"), [2, 1]);
            assert_eq!(seqs("設 worker"), [2, 1]);
            assert_eq!(seqs("設計"), [3, 1]);
            assert_eq!(seqs("設計 worker"), [1, 2]);
        }
    }

    /// A short word of ASCII letters only says nothing as a substring ("is" in "this"): it
    /// reorders nothing, as before short words counted.
    #[test]
    fn a_short_word_of_ascii_letters_reorders_nothing() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for (ts, body) in [(1_000, "this worker"), (2_000, "worker")] {
            store
                .append(&crate::raw::Event {
                    ts,
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        for text in ["worker", "is worker", "IS worker"] {
            let seqs: Vec<i64> = raw_search(p, text, Some("github.com/o/r"))
                .iter()
                .map(|h| h.seq)
                .collect();
            assert_eq!(seqs, [2, 1], "{text}");
        }
    }

    /// Records with `bodies` in one repository, indexed: the seqs a query finds, the best first.
    fn found(bodies: &[&str], query: &str) -> Vec<i64> {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for (n, body) in bodies.iter().enumerate() {
            store
                .append(&crate::raw::Event {
                    ts: 1_000 * (n as i64 + 1),
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        raw(p, query, Some("github.com/o/r"), 10)
            .unwrap()
            .iter()
            .map(|h| h.seq)
            .collect()
    }

    /// A hit below the best `POOL` keeps its place however many hits are asked for: the short
    /// words reorder only the pool (Codex on #360: a limit over `POOL` reordered every hit).
    #[test]
    fn a_hit_below_the_pool_keeps_its_place_under_a_larger_limit() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        // The first and longest ranks last.
        for n in 0..=POOL {
            let body = if n == 0 { "設計 worker" } else { "worker" };
            store
                .append(&crate::raw::Event {
                    ts: 1_000 * (n as i64 + 1),
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        let seqs: Vec<i64> = raw(p, "設計 worker", Some("github.com/o/r"), 2 * POOL)
            .unwrap()
            .iter()
            .map(|h| h.seq)
            .collect();
        assert_eq!(seqs.len(), POOL + 1);
        assert_eq!((seqs[0], seqs[POOL]), (POOL as i64 + 1, 1));
    }

    /// A word of hiragana only has no trigram at any length (particles and endings), and says as
    /// little as a substring: it reorders nothing (Codex on #360).
    #[test]
    fn a_hiragana_word_reorders_nothing() {
        for (body, query) in [
            ("worker ください", "worker ください"),
            ("worker こと", "worker こと"),
            ("worker について", "について worker"),
        ] {
            assert_eq!(found(&[body, "worker"], query), [2, 1], "{query}");
        }
    }

    /// A short word is a run between whitespace and punctuation, as a trigram's is.
    #[test]
    fn a_short_word_ends_at_punctuation() {
        assert_eq!(found(&["設計 worker", "worker"], "worker"), [2, 1]);
        assert_eq!(found(&["設計 worker", "worker"], "worker、設計。"), [1, 2]);
    }

    /// Tool output stays in raw.db, out of the index (#317, decision 42): neither a word with a
    /// trigram nor a short one finds it, before a tombstone's reindex or after, while the prompt
    /// beside it is found; its `raw_docs` row keeps its labels.
    #[test]
    fn tool_output_is_kept_but_not_searched() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let prompt = store
            .append(&crate::raw::Event {
                ts: 1_000,
                ..crate::raw::test_event(r#"{"prompt":"設計 lantern"}"#)
            })
            .unwrap();
        let tool = store
            .append(&crate::raw::Event {
                kind: "tool".into(),
                ts: 2_000,
                ..crate::raw::test_event(r#"{"tool":"Bash","input":"cat","output":"設計 lantern"}"#)
            })
            .unwrap();
        let indexed = || -> Vec<i64> {
            let k = crate::knowledge::open(p).unwrap();
            k.prepare(
                "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid
                 WHERE raw_fts MATCH 'lantern' ORDER BY d.seq",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        crate::worker::run_once(p).unwrap();
        assert_eq!(indexed(), [prompt]);
        let key = format!("{}:{prompt}", store.device());
        for q in ["lantern", "設計"] {
            let seqs: Vec<i64> = raw_search(p, q, None).iter().map(|h| h.seq).collect();
            assert_eq!(seqs, [prompt], "{q}");
            // The scan of a short word reads no tool output either, not only the answer shows none.
            let k = crate::knowledge::open(p).unwrap();
            let raw = crate::raw::open(p).unwrap();
            let order = raw_order(Some(&raw), &k, q, None, (None, None), None, None, 10).unwrap();
            assert_eq!(order, std::slice::from_ref(&key), "{q}");
        }
        let device = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Range {
                device,
                seq: tool,
                offset: 0,
                length: 2,
            })
            .unwrap();
        crate::worker::run_once(p).unwrap();
        assert_eq!(indexed(), [prompt]);
        let k = crate::knowledge::open(p).unwrap();
        let kind: String = k
            .query_row("SELECT kind FROM raw_docs WHERE seq = ?1", [tool], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "tool");
    }

    /// A query of short words alone looks for them in the text raw.db holds now (#317): written
    /// as a `\u` escape too, ASCII letters in either case, never in a key, never under a mask.
    #[test]
    fn a_short_word_is_found_in_the_text_raw_db_holds_now() {
        let none = Vec::<i64>::new();
        assert_eq!(
            found(&[r#"{"prompt":"\u8a2d\u8a08を見直す"}"#, "worker"], "設計"),
            [1]
        );
        assert_eq!(found(&["the M5 plan", "worker"], "m5"), [1]);
        assert_eq!(found(&["the m5 plan", "worker"], "M5"), [1]);
        assert_eq!(found(&[r#"{"設計":"worker"}"#], "設計"), none);
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let seq = store.append(&crate::raw::test_event("M5 plan")).unwrap();
        crate::worker::run_once(p).unwrap();
        assert_eq!(raw_search(p, "M5", None).len(), 1);
        let device = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Range {
                device,
                seq,
                offset: 0,
                length: 2,
            })
            .unwrap();
        assert!(raw_search(p, "M5", None).is_empty());
        crate::worker::run_once(p).unwrap();
        assert!(raw_search(p, "M5", None).is_empty());
        assert_eq!(raw_search(p, "plan", None).len(), 1);
    }

    /// A query of short words alone reads the records a page at a time, the newest first, until
    /// it has enough: one past the first page is found.
    #[test]
    fn a_short_word_is_found_past_the_first_page() {
        let mut bodies = vec!["worker"; 300];
        bodies[0] = "設計 old";
        bodies[299] = "設計 new";
        assert_eq!(found(&bodies, "設計"), [300, 1]);
    }

    /// The short words that count are few and counted once: a pasted page is still one query
    /// that SQLite can prepare (Codex on #360: 1,000 of them made an expression too deep).
    #[test]
    fn a_query_of_a_thousand_short_words_still_runs() {
        let repeated = format!("worker {}", "M5 ".repeat(1_000));
        assert_eq!(found(&["M5 worker", "worker"], &repeated), [1, 2]);
        let distinct: String = (0..1_100)
            .map(|i| format!("{}1 ", char::from_u32(0x4e00 + i).unwrap()))
            .collect();
        assert_eq!(
            found(&["M5 worker", "worker"], &format!("worker M5 {distinct}")),
            [1, 2]
        );
    }

    #[test]
    fn a_rewound_record_leaves_the_raw_index() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("kept lease note"))
            .unwrap();
        raw.append(&crate::raw::test_event("lost lease note"))
            .unwrap();
        let mut k = crate::knowledge::open(p).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        drop(raw);
        let c = Connection::open(p.join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > 1", []).unwrap();
        drop(c);
        let raw = crate::raw::open(p).unwrap();
        crate::knowledge::checkpoint::rewind(&raw, &k, &mut consumers).unwrap();
        let hits = raw_search(p, "lease note", None);
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_record_tombstone_hides_it_and_a_range_tombstone_masks_only_its_range() {
        use crate::raw::{Item, Target};
        // Tombstones that arrive before the index reads the records, and after it has.
        for index_first in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            let mut raw = crate::raw::open(p).unwrap();
            let a = raw
                .append(&crate::raw::test_event("alpha zqx-private-words tail"))
                .unwrap();
            let b = raw
                .append(&crate::raw::test_event("bravo visible"))
                .unwrap();
            let dev = raw.device().to_owned();
            if index_first {
                crate::worker::run_once(p).unwrap();
                assert_eq!(raw_search(p, "bravo", None).len(), 1);
            }
            raw.append_tombstone(Target::Record {
                device: dev.clone(),
                seq: b,
            })
            .unwrap();
            raw.append_tombstone(Target::Range {
                device: dev.clone(),
                seq: a,
                offset: 6,
                length: 17,
            })
            .unwrap();
            // Hidden before the index reaches the tombstones too.
            assert!(raw_search(p, "bravo", None).is_empty());
            assert!(raw_search(p, "zqx-private", None).is_empty());
            crate::worker::run_once(p).unwrap();
            assert!(raw_search(p, "bravo", None).is_empty());
            assert!(raw_search(p, "zqx-private", None).is_empty());
            let hits = raw_search(p, "alpha", None);
            assert_eq!(hits.len(), 1);
            assert!(hits[0].snippet.contains("tail") && !hits[0].snippet.contains("zqx"));
            let recs = raw.after(&dev, 0, 10).unwrap();
            assert!(
                matches!(&recs[0].item, Item::Event(e) if e.body == "alpha ***************** tail")
            );
            assert!(matches!(recs[1].item, Item::Removed));
        }
    }

    #[test]
    fn a_copied_home_still_hides_what_an_old_device_tombstoned() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let seq = store
            .append(&crate::raw::test_event("copied lantern"))
            .unwrap();
        crate::worker::run_once(p).unwrap();
        let old = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Record { device: old, seq })
            .unwrap();
        drop(store);
        // The copy gets a new device id: the tombstone stays under the old one, unindexed.
        let copy = tempfile::tempdir().unwrap();
        for f in ["raw.db", "knowledge.db"] {
            std::fs::copy(p.join(f), copy.path().join(f)).unwrap();
        }
        let moved = crate::raw::open(copy.path()).unwrap();
        assert_eq!(moved.devices().unwrap().len(), 1);
        assert!(raw_search(copy.path(), "lantern", None).is_empty());
    }

    #[test]
    fn a_hidden_hit_never_takes_the_place_of_a_visible_one() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for body in ["shared lantern one", "shared lantern two"] {
            store.append(&crate::raw::test_event(body)).unwrap();
        }
        crate::worker::run_once(p).unwrap();
        let both = raw_search(p, "lantern", None);
        assert_eq!(both.len(), 2);
        store
            .append_tombstone(crate::raw::Target::Record {
                device: both[0].device.clone(),
                seq: both[0].seq,
            })
            .unwrap();
        // The index has not reached the tombstone: the top hit is hidden, the next one shown.
        let one = raw(p, "lantern", None, 1).unwrap();
        assert_eq!(one.iter().map(|h| h.seq).collect::<Vec<_>>(), [both[1].seq]);
    }

    fn raw_search(home: &std::path::Path, q: &str, repo: Option<&str>) -> Vec<RawHit> {
        raw(home, q, repo, 10).unwrap()
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
