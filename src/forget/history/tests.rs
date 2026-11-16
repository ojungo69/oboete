use super::fixture::{mark_applied, register};
use super::*;

/// A home with both stores made, and a connection of the test's own to its raw.db.
fn stores() -> (tempfile::TempDir, crate::raw::Raw, Connection, Connection) {
    let home = tempfile::tempdir().unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let conn = Connection::open(home.path().join("raw.db")).unwrap();
    (home, raw, k, conn)
}

/// With no history forget registered the readers read: raw.db alone is asked.
#[test]
fn no_history_forget_leaves_the_imported_documents_open() {
    let (_home, raw, k, _conn) = stores();
    assert!(raw.imported_open(&k).unwrap().is_some());
    // A knowledge.db without the table (one made before decision 109) changes nothing then.
    k.execute_batch("DROP TABLE history_applied").unwrap();
    assert!(raw.imported_open(&k).unwrap().is_some());
}

/// From its registration until knowledge.db marks it applied, a history forget closes the
/// imported documents to every reader; the mark opens them.
#[test]
fn a_registered_history_forget_closes_the_imported_documents_until_applied() {
    let (home, raw, k, _conn) = stores();
    register(home.path(), "j1", "t1");
    assert!(raw.imported_open(&k).unwrap().is_none());
    mark_applied(home.path(), "j1", "t1");
    assert!(raw.imported_open(&k).unwrap().is_some());
    // Every one of them: a second closes them again.
    register(home.path(), "j2", "t2");
    assert!(raw.imported_open(&k).unwrap().is_none());
    // A knowledge.db without the table has applied none.
    k.execute_batch("DROP TABLE history_applied").unwrap();
    assert!(raw.imported_open(&k).unwrap().is_none());
}

/// The proof and the count of history forgets come from the one read of them (#1022): none
/// registered is open with no count, one waiting is closed with its count, and the mark opens it.
#[test]
fn the_proof_comes_with_the_count_it_was_read_with() {
    let (home, raw, k, _conn) = stores();
    let counted = |raw: &crate::raw::Raw| {
        let (open, n) = raw.imported_open_counted(&k).unwrap();
        (open.is_some(), n)
    };
    assert_eq!(counted(&raw), (true, 0));
    register(home.path(), "j1", "t1");
    assert_eq!(counted(&raw), (false, 1));
    mark_applied(home.path(), "j1", "t1");
    assert_eq!(counted(&raw), (true, 1));
    register(home.path(), "j2", "t2");
    assert_eq!(counted(&raw), (false, 2));
}

/// A job raw.db lost (a restore of an older copy) and a log applied again gets another token: the
/// mark of the first application does not open the documents, so a document of the repository
/// imported between the restore and the second application is not shown.
#[test]
fn a_history_forget_applied_again_closes_them_until_applied_again() {
    let (home, raw, k, conn) = stores();
    register(home.path(), "j1", "t1");
    mark_applied(home.path(), "j1", "t1");
    assert!(raw.imported_open(&k).unwrap().is_some());
    conn.execute("DELETE FROM forget_history WHERE job = 'j1'", [])
        .unwrap();
    register(home.path(), "j1", "t2");
    assert!(raw.imported_open(&k).unwrap().is_none());
    mark_applied(home.path(), "j1", "t2");
    assert!(raw.imported_open(&k).unwrap().is_some());
}

/// A raw.db this build has not opened to write has no table yet: read-only, it holds none.
#[test]
fn a_raw_db_without_the_table_holds_no_history_forget() {
    let (_home, _raw, k, conn) = stores();
    conn.execute_batch("DROP TABLE forget_history").unwrap();
    assert!(registered(&conn).unwrap().is_empty());
    assert!(imported_open(&conn, &k).unwrap().is_some());
}

/// A registration moves the derived writers' fence and the preview's count, so a window, an
/// embedding batch or a preview read before it does not commit after it.
#[test]
fn a_history_forget_moves_the_fence_and_the_denied_count() {
    let (home, raw, _k, _conn) = stores();
    let (fence, denied) = (raw.fence_count().unwrap(), raw.denied_count().unwrap());
    register(home.path(), "j1", "t1");
    assert_eq!(raw.fence_count().unwrap(), fence + 1);
    assert_eq!(raw.denied_count().unwrap(), denied + 1);
}

/// How a function or constant whose SQL reads knowledge.db's imported documents is kept from them
/// while a history forget is not applied (5c's reach table).
enum Reach {
    /// It takes `&Applied`: no caller reaches it without the proof.
    Proof,
    /// It reads them for no reader, or as the owner's own act: why.
    Without(&'static str),
    /// A constant of SQL that only functions taking the proof read.
    Through,
}

/// 5c's reach table: every item of `src/` whose SQL reads `imported`, `imported_fts` or
/// `imported_files`, by file and name.
const READERS: &[(&str, &str, Reach)] = &[
    ("src/cards.rs", "imported_by_file", Reach::Proof),
    (
        "src/consumer/imported.rs",
        "step",
        Reach::Without("the consumer that writes the rows"),
    ),
    (
        "src/consumer/imported.rs",
        "rewind",
        Reach::Without("drops rows past raw.db's commits"),
    ),
    ("src/consumer/imported.rs", "maps", Reach::Proof),
    ("src/consumer/imported.rs", "prompt_line", Reach::Proof),
    (
        "src/embed_phase.rs",
        "doctor_facts_in",
        Reach::Without("doctor's counts, no text"),
    ),
    (
        "src/embed_phase.rs",
        "QUEUE",
        Reach::Without("the embedding queue: uids and times, no text"),
    ),
    ("src/embed_phase.rs", "read_page", Reach::Proof),
    (
        "src/embed_phase.rs",
        "current",
        Reach::Without(
            "a document as stored: the vectors' write-back and upkeep, which go on removing what \
             is gone; `next_half`, which sends what it reads, takes the proof",
        ),
    ),
    (
        "src/forget.rs",
        "uid_preview",
        Reach::Without(
            "the owner's own forget of a uid named: the viewer gets counts, the CLI the title \
             through the gate, which the op log it falls back on holds too",
        ),
    ),
    (
        "src/forget/history.rs",
        "remove",
        Reach::Without("the apply, which removes the rows a history forget covers"),
    ),
    (
        "src/mcp.rs",
        "save_memory",
        Reach::Without("a note's op, which its append checks again"),
    ),
    ("src/search/b.rs", "IMPORTED_SESSIONS", Reach::Through),
    ("src/search/b.rs", "imported_sessions", Reach::Proof),
    ("src/search/b.rs", "imported_projects", Reach::Proof),
    ("src/search/b.rs", "sidecar", Reach::Proof),
    ("src/search/b.rs", "imported_fts_sql", Reach::Proof),
    ("src/search/b.rs", "imported_hit", Reach::Proof),
    ("src/search/b.rs", "imported_kind", Reach::Proof),
    ("src/search/b.rs", "imported_uid", Reach::Proof),
    ("src/search/b.rs", "imported_text", Reach::Proof),
    ("src/search/b.rs", "imported_side", Reach::Proof),
    ("src/search/b.rs", "imported_known", Reach::Proof),
    ("src/search/b.rs", "imported_time", Reach::Proof),
    ("src/view.rs", "notes_page", Reach::Proof),
    ("src/view.rs", "REPOS_IMPORTED", Reach::Through),
    ("src/view.rs", "REPO_NAMES_IMPORTED", Reach::Through),
];

/// Whether a signature takes the proof: a parameter of type `&Applied`, not an optional one
/// (`Option<&Applied>` lets its body run without it), nor one that only names the type
/// (`impl Fn(&Applied)`, a where clause).
fn takes_proof(signature: &str) -> bool {
    regex::Regex::new(
        r"[(,]\s*(?:mut\s+)?\w+\s*:\s*&(?:'\w+\s+)?(?:crate::forget::history::)?Applied\b",
    )
    .unwrap()
    .is_match(signature)
}

/// The item line regex: a `fn`, or a constant or static of SQL.
fn item_line() -> regex::Regex {
    regex::Regex::new(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*fn\s+(\w+)|^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:",
    )
    .unwrap()
}

/// The signature of the item at line `at`: its lines up to its body's brace.
fn signature_at(lines: &[&str], at: usize) -> String {
    let joined = lines[at..(at + 30).min(lines.len())].join(" ");
    joined.split('{').next().unwrap_or_default().to_owned()
}

type Readers = std::collections::BTreeMap<(String, String), bool>;

/// Each item of `src/` (test files left out) whose SQL reads an imported table, with whether it
/// takes the proof (`takes_proof`; two items of one name in a file take it only if both do): the
/// nearest `fn` or constant above each statement. And each file's text, by its path.
// shortcut: a line scan, not a parse: SQL in an outer function below a nested `fn`, or a
// statement built from pieces, is the review's to catch; parse with `syn` if one slips by.
fn sql_readers() -> (Readers, std::collections::BTreeMap<String, String>) {
    let statement = regex::Regex::new(
        r"\b(?:FROM|JOIN|INTO|UPDATE|EXISTS\s*\(\s*SELECT\s+1\s+FROM)\s+imported(?:_fts|_files)?\b",
    )
    .unwrap();
    let item = item_line();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "testdata" {
                    dirs.push(path);
                }
            } else if name.ends_with(".rs")
                && ![
                    "tests.rs",
                    "fixture.rs",
                    "input_tests.rs",
                    "fixture_scan.rs",
                    "stub.rs",
                ]
                .contains(&name.as_str())
            {
                files.push(path);
            }
        }
    }
    let mut found = Readers::new();
    let mut texts = std::collections::BTreeMap::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let file = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for (i, line) in lines.iter().enumerate() {
            if !statement.is_match(line) {
                continue;
            }
            let Some((at, name)) = (0..=i).rev().find_map(|j| {
                let c = item.captures(lines[j])?;
                Some((j, c.get(1).or(c.get(2))?.as_str().to_owned()))
            }) else {
                panic!("{file}:{}: an imported table read outside any item", i + 1);
            };
            let proof = takes_proof(&signature_at(&lines, at));
            found
                .entry((file.clone(), name))
                .and_modify(|p: &mut bool| *p = *p && proof)
                .or_insert(proof);
        }
        texts.insert(file, text);
    }
    (found, texts)
}

/// The functions of `texts` that read `constant`, by file and name, each with whether it takes
/// the proof: a function runs from its `fn` line to the next one, its comments and the
/// constant's own definition left out.
fn readers_of(
    texts: &std::collections::BTreeMap<String, String>,
    constant: &str,
) -> Vec<(String, bool)> {
    let item = item_line();
    let word = regex::Regex::new(&format!(r"\b{constant}\b")).unwrap();
    let mut out = Vec::new();
    for (file, text) in texts {
        let lines: Vec<&str> = text.lines().collect();
        let fns: Vec<(usize, String)> = (lines.iter().enumerate())
            .filter_map(|(j, line)| Some((j, item.captures(line)?.get(1)?.as_str().to_owned())))
            .collect();
        for (n, (at, name)) in fns.iter().enumerate() {
            let end = fns.get(n + 1).map_or(lines.len(), |(next, _)| *next);
            let reads = lines[*at..end].iter().any(|line| {
                let defines = (item.captures(line))
                    .and_then(|c| c.get(2))
                    .is_some_and(|m| m.as_str() == constant);
                !line.trim_start().starts_with("//") && !defines && word.is_match(line)
            });
            if reads {
                let proof = takes_proof(&signature_at(&lines, *at));
                out.push((format!("{file}:{name}"), proof));
            }
        }
    }
    out
}

/// 5c: every reader of knowledge.db's imported documents is in the reach table, and each one it
/// says takes the proof does, so no new reader comes in unguarded and none loses its guard. Every
/// reader is guarded or says why not since 1a-4; registration (5b) still waits for what this
/// table cannot see (docs/milestone-5-plan.md, 5c).
#[test]
fn every_reader_of_the_imported_documents_is_in_the_reach_table() {
    let (found, texts) = sql_readers();
    let listed: std::collections::BTreeMap<(String, String), &Reach> = READERS
        .iter()
        .map(|(file, name, reach)| (((*file).to_owned(), (*name).to_owned()), reach))
        .collect();
    let unlisted: Vec<_> = found.keys().filter(|k| !listed.contains_key(*k)).collect();
    assert!(
        unlisted.is_empty(),
        "readers missing from READERS: {unlisted:?}"
    );
    let gone: Vec<_> = listed.keys().filter(|k| !found.contains_key(*k)).collect();
    assert!(
        gone.is_empty(),
        "READERS lists what reads no imported table: {gone:?}"
    );
    for (key, reach) in &listed {
        let proof = found[key];
        match reach {
            Reach::Proof => assert!(proof, "{key:?} is listed as taking the proof and does not"),
            Reach::Without(why) => {
                assert!(
                    !proof,
                    "{key:?} takes the proof: list it as Proof, not as {why:?}"
                )
            }
            Reach::Through => {
                let readers = readers_of(&texts, &key.1);
                assert!(
                    !readers.is_empty() && readers.iter().all(|(_, proof)| *proof),
                    "{key:?} is listed as read only by functions taking the proof: {readers:?}"
                )
            }
        }
    }
}

/// The reach test's proof check: a parameter of type `&Applied`, not an optional one, and a
/// constant's readers are every function that names it outside a comment, wherever it is.
#[test]
fn the_reach_test_counts_only_a_proof_its_reader_cannot_run_without() {
    assert!(takes_proof("fn a(k: &Connection, _: &Applied, uid: &str)"));
    assert!(takes_proof(
        "fn a(k: &Connection, _: &crate::forget::history::Applied)"
    ));
    assert!(!takes_proof(
        "fn a(k: &Connection, applied: Option<&Applied>)"
    ));
    assert!(!takes_proof(
        "fn a(k: &Connection, applied: Option<&crate::forget::history::Applied>)"
    ));
    assert!(takes_proof("fn a<'a>(k: &Connection, proof: &'a Applied)"));
    assert!(!takes_proof("fn a(k: &Connection) -> &Applied"));
    assert!(!takes_proof(
        "fn a(k: &Connection, applied: Option< &Applied>)"
    ));
    assert!(!takes_proof(
        "fn a(k: &Connection, f: impl Fn(&Applied) -> bool)"
    ));
    assert!(!takes_proof(
        "fn a<F>(k: &Connection, f: F) where F: Fn(&Applied)"
    ));
    let texts = [
        (
            "a.rs".to_owned(),
            "fn before(k: &Connection) {}\n/// QUERY's text.\nconst QUERY: &str = \"FROM imported\";\n\
             fn reader(k: &Connection, _: &Applied) {\n    k.prepare(QUERY);\n}\n\
             fn guarded(k: &Connection, _: &Applied) {\n    // not QUERY\n}\n"
                .to_owned(),
        ),
        (
            "b.rs".to_owned(),
            "fn other(k: &Connection) {\n    k.prepare(a::QUERY);\n}\n".to_owned(),
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        readers_of(&texts, "QUERY"),
        [
            ("a.rs:reader".to_owned(), true),
            ("b.rs:other".to_owned(), false)
        ]
    );
}

/// Doctor's line names the history forgets knowledge.db has not applied, a job applied again
/// under another token among them, and is none once it has applied them all.
#[test]
fn doctor_names_the_history_forgets_not_yet_applied() {
    let set = |pairs: &[(&str, &str)]| -> std::collections::BTreeSet<super::Pair> {
        pairs
            .iter()
            .map(|(j, t)| ((*j).to_owned(), (*t).to_owned()))
            .collect()
    };
    let registered = set(&[("j1", "t1"), ("j2", "t2")]);
    assert_eq!(
        super::doctor_line(&registered, &set(&[("j1", "t1")])).as_deref(),
        Some(
            "imported history is withheld: 1 forget(s) of a repository's history not yet \
             applied in knowledge.db (`oboete forget --continue` applies them; the worker does \
             too)"
        )
    );
    assert!(super::doctor_line(&registered, &set(&[])).is_some_and(|l| l.contains(": 2 forget")));
    assert_eq!(super::doctor_line(&registered, &registered), None);
    assert!(super::doctor_line(&set(&[("j1", "t2")]), &set(&[("j1", "t1")])).is_some());
    assert_eq!(super::doctor_line(&set(&[]), &set(&[])), None);
}

/// What knowledge.db holds of imported document `uid`: its rows, its files' rows, its vectors'
/// keys, the vec0 index rows `ids` (its keys' ids, read before), and its place in the embedding
/// queue (none before the queue is made).
fn held(k: &Connection, uid: &str, ids: &[i64]) -> [i64; 5] {
    let one = |sql: &str| k.query_row(sql, [uid], |r| r.get::<_, i64>(0)).unwrap();
    let queued = match crate::consumer::manifest::exists(k, "table", "vector_todo").unwrap() {
        true => one("SELECT COUNT(*) FROM vector_todo WHERE family = 'i' AND key = ?1"),
        false => 0,
    };
    let indexed = ids
        .iter()
        .map(|id| {
            k.query_row(
                "SELECT COUNT(*) FROM vec_index WHERE rowid = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        })
        .sum();
    [
        one("SELECT COUNT(*) FROM imported WHERE uid = ?1"),
        one("SELECT COUNT(*) FROM imported_files WHERE uid = ?1"),
        one("SELECT COUNT(*) FROM vector_keys WHERE kind IN ('k', 'p') AND key = ?1"),
        indexed,
        queued,
    ]
}

/// The ids of imported document `uid`'s vector keys.
fn key_ids(k: &Connection, uid: &str) -> Vec<i64> {
    k.prepare("SELECT id FROM vector_keys WHERE kind IN ('k', 'p') AND key = ?1")
        .unwrap()
        .query_map([uid], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// How many imported documents' full-text rows hold `word`.
fn indexed(k: &Connection, word: &str) -> i64 {
    k.query_row(
        "SELECT COUNT(*) FROM imported_fts WHERE imported_fts MATCH ?1",
        [format!("\"{word}\"")],
        |r| r.get(0),
    )
    .unwrap()
}

fn pairs(list: &[(&str, &str)]) -> std::collections::BTreeSet<super::Pair> {
    list.iter()
        .map(|(j, t)| ((*j).to_owned(), (*t).to_owned()))
        .collect()
}

/// 5c's apply: every row knowledge.db made from a document a history forget covers goes, under
/// whichever repository it was filed, with its full-text rows, files, vectors and place in the
/// embedding queue; the forget is recorded and the readers open; what it does not cover stays. A
/// second pass changes nothing.
#[test]
fn the_apply_removes_what_a_history_forget_covers_and_opens_the_readers() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let read = &["src/pay.rs"][..];
    let covered = s.imported_with_files(
        "o1",
        ("alpha", "cm"),
        "decision",
        1_000,
        "Zebrafinch retries",
        read,
        &[],
    );
    // claude-mem moved it to another project: the same uid comes in again, filed under that one.
    s.imported(
        "o1",
        "beta",
        1_100,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    let kept = s.imported_with_files(
        "o2",
        ("alpha", "cm"),
        "decision",
        1_200,
        "Heron backoff",
        read,
        &[],
    );
    s.run();
    crate::embed_phase::fixture::vectors(&s);
    // Imported after the embedding: it waits in the queue.
    let queued = s.imported(
        "o3",
        "alpha",
        1_300,
        "Zebrafinch limits",
        "Zebrafinch limits.",
    );
    s.run();
    let mut k = crate::knowledge::open(&home).unwrap();
    let (covered_ids, kept_ids) = (key_ids(&k, &covered), key_ids(&k, &kept));
    assert_eq!(held(&k, &covered, &covered_ids), [2, 1, 1, 1, 0]);
    assert_eq!(held(&k, &queued, &[]), [1, 0, 0, 0, 1]);
    assert_eq!(indexed(&k, "Zebrafinch"), 3);
    register_covering(&home, "j1", "t1", &[&covered, &queued]);
    assert!(s.raw.imported_open(&k).unwrap().is_none());
    assert_eq!(s.raw.apply_history(&mut k).unwrap(), 1);
    assert_eq!(held(&k, &covered, &covered_ids), [0; 5]);
    assert_eq!(held(&k, &queued, &[]), [0; 5]);
    assert_eq!(indexed(&k, "Zebrafinch"), 0);
    assert_eq!(held(&k, &kept, &kept_ids), [1, 1, 1, 1, 0]);
    assert_eq!(indexed(&k, "Heron"), 1);
    assert_eq!(super::applied(&k).unwrap(), pairs(&[("j1", "t1")]));
    assert!(s.raw.imported_open(&k).unwrap().is_some());
    assert_eq!(s.raw.apply_history(&mut k).unwrap(), 0);
    assert_eq!(held(&k, &kept, &kept_ids), [1, 1, 1, 1, 0]);
}

/// 5c: a knowledge.db made new (a rebuild) records a history forget at its first round, before it
/// projects anything, and then projects none of what the forget covers, documents or files; no
/// vector of them comes over from the old file. What the forget does not cover comes back whole.
#[test]
fn a_rebuild_records_the_forget_first_and_brings_back_nothing_it_covers() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let read = &["src/pay.rs"][..];
    let covered = s.imported_with_files(
        "o1",
        ("alpha", "cm"),
        "decision",
        1_000,
        "Zebrafinch retries",
        read,
        &[],
    );
    let kept = s.imported_with_files(
        "o2",
        ("alpha", "cm"),
        "decision",
        1_200,
        "Heron backoff",
        read,
        &[],
    );
    s.run();
    crate::embed_phase::fixture::vectors(&s);
    let sha = |k: &Connection, uid: &str| -> String {
        k.query_row(
            "SELECT src_sha FROM vector_keys WHERE kind IN ('k', 'p') AND key = ?1",
            [uid],
            |r| r.get(0),
        )
        .unwrap()
    };
    let (covered_sha, kept_sha) = {
        let k = crate::knowledge::open(&home).unwrap();
        (sha(&k, &covered), sha(&k, &kept))
    };
    register_covering(&home, "j1", "t1", &[&covered]);
    // With no store open, as `oboete rebuild` runs.
    let crate::search::b::fixture::Store { home: _dir, raw } = s;
    drop(raw);
    crate::worker::rebuild(&home).unwrap();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(super::applied(&k).unwrap(), pairs(&[("j1", "t1")]));
    let raw = crate::raw::open(&home).unwrap();
    assert!(raw.imported_open(&k).unwrap().is_some());
    assert_eq!(held(&k, &covered, &[])[..2], [0, 0]);
    assert_eq!(held(&k, &kept, &[])[..2], [1, 1]);
    let cached = |sha: &str| -> i64 {
        k.query_row(
            "SELECT COUNT(*) FROM vectors WHERE src_sha = ?1",
            [sha],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((cached(&covered_sha), cached(&kept_sha)), (0, 1));
}

/// 5c's resume: the worker's round applies a history forget knowledge.db has not, before it
/// drains.
#[test]
fn a_worker_round_applies_a_history_forget_waiting() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let covered = s.imported(
        "o1",
        "alpha",
        1_000,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    s.run();
    register_covering(&home, "j1", "t1", &[&covered]);
    s.run();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(held(&k, &covered, &[])[0], 0);
    assert_eq!(super::applied(&k).unwrap(), pairs(&[("j1", "t1")]));
    assert!(s.raw.imported_open(&k).unwrap().is_some());
}

/// 5c's first red test: raw.db restored to before a job loses it, a document it covered comes in
/// again and is projected; the job applied again from its log gets another token, which closes
/// the readers though knowledge.db applied the job before, and the apply that follows removes the
/// document again.
#[test]
fn a_forget_applied_again_with_another_token_removes_what_came_in_meanwhile() {
    use super::fixture::{lose, register_covering};
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let covered = s.imported(
        "o1",
        "alpha",
        1_000,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    s.run();
    register_covering(&home, "j1", "t1", &[&covered]);
    s.run();
    lose(&home, "j1");
    s.imported(
        "o1",
        "alpha",
        1_000,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    s.run();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(held(&k, &covered, &[])[0], 1);
    register_covering(&home, "j1", "t2", &[&covered]);
    assert!(s.raw.imported_open(&k).unwrap().is_none());
    s.run();
    assert_eq!(held(&k, &covered, &[])[0], 0);
    assert_eq!(super::applied(&k).unwrap(), pairs(&[("j1", "t2")]));
    assert!(s.raw.imported_open(&k).unwrap().is_some());
}

/// 5c: `oboete forget --continue` applies a history forget knowledge.db has not, and with none
/// waiting it opens nothing to write.
#[test]
fn forget_continue_applies_a_history_forget_waiting() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let covered = s.imported(
        "o1",
        "alpha",
        1_000,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    s.run();
    assert_eq!(super::apply_due(&home).unwrap(), 0);
    register_covering(&home, "j1", "t1", &[&covered]);
    assert_eq!(super::apply_due(&home).unwrap(), 1);
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(held(&k, &covered, &[])[0], 0);
    assert_eq!(super::apply_due(&home).unwrap(), 0);
}

/// 5c (CodeRabbit on #1023): a drain outside the worker's rounds applies a waiting history forget
/// first too: `worker::drained` (a command's plan, replay) and a maintenance report's drain (a
/// restore, a recuration).
#[test]
fn every_drain_applies_a_history_forget_waiting_first() {
    use super::fixture::register_covering;
    for report in [false, true] {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let covered = s.imported(
            "o1",
            "alpha",
            1_000,
            "Zebrafinch retries",
            "Zebrafinch retries.",
        );
        s.run();
        register_covering(&home, "j1", "t1", &[&covered]);
        // With no store open, as a command opens them.
        let crate::search::b::fixture::Store { home: _dir, raw } = s;
        drop(raw);
        if report {
            // What the plan reads, right after the report's drain.
            let (_, prepared) = crate::worker::prepare_report(
                &home,
                crate::executable::CommandCaller::Worker,
                &mut |_| {},
                |_, k| super::applied(k),
            );
            let (planned, ..) = prepared.unwrap();
            assert_eq!(planned, pairs(&[("j1", "t1")]));
        } else {
            drop(crate::worker::drained(&home, false).unwrap());
        }
        let k = crate::knowledge::open(&home).unwrap();
        assert_eq!(held(&k, &covered, &[])[0], 0, "report: {report}");
        assert_eq!(
            super::applied(&k).unwrap(),
            pairs(&[("j1", "t1")]),
            "report: {report}"
        );
    }
}

/// 5d: an import leaves out a document, its files and a note a history forget covers; what the
/// forget does not cover goes in.
#[test]
fn an_import_leaves_out_what_a_history_forget_covers() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let read = &["src/pay.rs"][..];
    register_covering(&home, "j1", "t1", &["claude-mem:test:o1", "oboete-note:n1"]);
    s.imported_with_files(
        "o1",
        ("alpha", "cm"),
        "decision",
        1_000,
        "Zebrafinch retries",
        read,
        &[],
    );
    let kept = s.imported_with_files(
        "o2",
        ("alpha", "cm"),
        "decision",
        1_200,
        "Heron backoff",
        read,
        &[],
    );
    s.note(
        "n1",
        "github.com/o/alpha",
        1_300,
        "Zebrafinch note",
        "Saved words.",
    );
    s.run();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(held(&k, "claude-mem:test:o1", &[])[..2], [0, 0]);
    assert_eq!(held(&k, "oboete-note:n1", &[])[0], 0);
    assert_eq!(held(&k, &kept, &[])[..2], [1, 1]);
    let ops: i64 = Connection::open(home.join("raw.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM ops WHERE type = 'import' AND json_extract(body, '$.uid') IN
               ('claude-mem:test:o1', 'oboete-note:n1')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ops, 0, "nothing covered is appended to raw.db");
}

/// 5d: a history forget committed after an import checked its batch, before the batch's own
/// transaction: the batch leaves out what it covers and appends the rest, not held up by it.
#[test]
fn a_forget_committed_during_an_import_leaves_out_what_it_covers_and_lets_the_rest_in() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let docs: Vec<_> = ["o1", "o2"]
        .iter()
        .map(|id| {
            crate::raw::Import::Doc(crate::raw::ImportDoc {
                uid: format!("claude-mem:test:{id}"),
                source: "claude-mem:test".into(),
                source_id: (*id).into(),
                kind: "decision".into(),
                repo: crate::import::repo("alpha"),
                session: "cm".into(),
                ts: 1_000,
                title: format!("Title {id}"),
                body: "Body.".into(),
            })
        })
        .collect();
    let at = home.clone();
    crate::raw::BEFORE_IMPORT_APPEND.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            register_covering(&at, "j1", "t1", &["claude-mem:test:o1"]);
        }))
    });
    let appended = s.raw.append_import_items(docs, |_| {}).unwrap();
    assert_eq!(appended, 1);
    let uids: Vec<String> = Connection::open(home.join("raw.db"))
        .unwrap()
        .prepare("SELECT json_extract(body, '$.uid') FROM ops WHERE type = 'import'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(uids, ["claude-mem:test:o2"]);
}

/// 5d: inside an append's own transaction, an op a history forget covers refuses the batch with
/// `HistoryCovered`, nothing written; imported work state is covered by its source and source id,
/// and an import takes a covered list as already imported.
#[test]
fn an_append_refuses_a_covered_op_and_the_work_state_import_knows_a_covered_list() {
    let s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    {
        let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        raw.execute_batch(
            "INSERT INTO forget_history(job, token) VALUES ('j1', 't1');
             INSERT INTO denied_imports(kind, key, job) VALUES
               ('uid', 'claude-mem:test:o1', 'j1'), ('ws', 'claude-mem:test:w7', 'j1');",
        )
        .unwrap();
    }
    let mut raw = crate::raw::open(&home).unwrap();
    let doc = serde_json::json!({"uid": "claude-mem:test:o1", "source": "claude-mem:test",
        "source_id": "o1", "kind": "decision", "repo": "claude-mem:alpha", "session": "cm",
        "ts": 1_000, "title": "T", "body": "B"});
    let err = raw
        .append_ops(&[(crate::raw::OpKind::Import, doc)])
        .unwrap_err();
    assert!(err.is::<crate::raw::HistoryCovered>(), "{err:#}");
    let list = serde_json::json!({"repo": "claude-mem:alpha", "list": "l", "fields": {},
        "clock": 1, "at": 1, "source": "claude-mem:test", "source_id": "w7"});
    let settings = crate::capture::Settings::load(&home).unwrap();
    let err = raw
        .append_imported_ops(&[(crate::raw::OpKind::WorkState, list)], &settings)
        .unwrap_err();
    assert!(err.is::<crate::raw::HistoryCovered>(), "{err:#}");
    assert!(
        raw.work_state_keys("claude-mem:test")
            .unwrap()
            .contains("w7")
    );
    assert!(
        !raw.work_state_keys("claude-mem:other")
            .unwrap()
            .contains("w7")
    );
}

/// 5d: a note a history forget covers is not saved again, though the op log still holds the
/// note's first save (which answers a repeated note as kept): whether the forget was there before
/// the save or came to cover it while the save was on its way (Codex on #1027).
#[test]
fn a_note_a_history_forget_covers_is_not_saved_again() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let repo = "github.com/o/alpha";
    let note = |id: &str| crate::raw::ImportDoc {
        uid: format!("{}:{id}", crate::raw::NOTE_SOURCE),
        source: crate::raw::NOTE_SOURCE.into(),
        source_id: id.into(),
        kind: "note".into(),
        repo: repo.into(),
        session: crate::raw::NOTE_SESSION.into(),
        ts: 1_000,
        title: format!("Zebrafinch note {id}"),
        body: "Saved words.".into(),
    };
    s.raw.append_imports(vec![note("n1"), note("n2")]).unwrap();
    register_covering(&home, "j1", "t1", &["oboete-note:n1"]);
    let settings = crate::capture::Settings::load(&home).unwrap();
    let mut save = |doc: &crate::raw::ImportDoc| {
        s.raw
            .append_note(
                doc,
                (&[], &Default::default()),
                repo,
                &settings,
                ("note", None),
            )
            .unwrap()
    };
    assert_eq!(save(&note("n1")), crate::raw::Noted::HistoryForgotten);
    let at = home.clone();
    crate::raw::BEFORE_IMPORT_APPEND.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            register_covering(&at, "j2", "t2", &["oboete-note:n2"]);
        }))
    });
    assert_eq!(save(&note("n2")), crate::raw::Noted::HistoryForgotten);
    let saves: i64 = Connection::open(home.join("raw.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM ops WHERE type = 'import'
               AND json_extract(body, '$.uid') IN ('oboete-note:n1', 'oboete-note:n2')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saves, 2, "the first saves alone");
}

/// #1025: a vector a set-aside copy of knowledge.db holds for a covered document stays behind
/// when the copy's vectors are carried into the live one.
#[test]
fn a_set_aside_copy_carries_no_vector_of_what_a_history_forget_covers() {
    use super::fixture::register_covering;
    let mut s = crate::search::b::fixture::Store::new();
    let home = s.home.path().to_owned();
    let covered = s.imported(
        "o1",
        "alpha",
        1_000,
        "Zebrafinch retries",
        "Zebrafinch retries.",
    );
    let kept = s.imported("o2", "alpha", 1_200, "Heron backoff", "Heron backoff.");
    s.run();
    crate::embed_phase::fixture::vectors(&s);
    let sha = |k: &Connection, uid: &str| -> String {
        k.query_row(
            "SELECT src_sha FROM vector_keys WHERE kind IN ('k', 'p') AND key = ?1",
            [uid],
            |r| r.get(0),
        )
        .unwrap()
    };
    let (covered_sha, kept_sha) = {
        let k = crate::knowledge::open(&home).unwrap();
        (sha(&k, &covered), sha(&k, &kept))
    };
    // The live knowledge.db set aside, as a quarantine leaves it, and a new one in its place.
    std::fs::rename(
        home.join("knowledge.db"),
        home.join("knowledge.db.quarantined-1"),
    )
    .unwrap();
    for end in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(home.join(format!("knowledge.db{end}")));
    }
    register_covering(&home, "j1", "t1", &[&covered]);
    let k = crate::knowledge::open(&home).unwrap();
    crate::embed_phase::carry_set_aside(&home, &k, &s.raw).unwrap();
    let cached = |sha: &str| -> i64 {
        k.query_row(
            "SELECT COUNT(*) FROM vectors WHERE src_sha = ?1",
            [sha],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((cached(&covered_sha), cached(&kept_sha)), (0, 1));
}

/// 5c: raw.db's readers of work state leave out an imported entry a history forget covers, from
/// its registration on: a repository's writes (its own, interleaved with the imported ones on one
/// device, keep their order) and My Work's imported subagent stops. A raw.db without the refusal
/// table reads every entry.
#[test]
fn the_work_state_readers_leave_out_an_imported_entry_a_history_forget_covers() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let repo = "github.com/o/alpha";
    let settings = crate::capture::Settings::load(home.path()).unwrap();
    let imported = |id: &str, at: i64| {
        (
            crate::raw::OpKind::WorkState,
            serde_json::json!({"repo": "claude-mem:alpha", "list": crate::work_state::AGENT_LIFECYCLE,
                "fields": {"task": id, "content_session_id": "s1"},
                "clock": at, "at": at, "source": "claude-mem:test", "source_id": id}),
        )
    };
    let own = |task: &str| {
        let mut fields = serde_json::Map::new();
        fields.insert("task".into(), task.into());
        fields
    };
    raw.work_state(repo, "plan", &own("own-1"), None).unwrap();
    raw.append_imported_ops(&[imported("w1", 1_000)], &settings)
        .unwrap();
    raw.work_state(repo, "plan", &own("own-2"), None).unwrap();
    raw.append_imported_ops(&[imported("w2", 2_000)], &settings)
        .unwrap();
    Connection::open(home.path().join("raw.db"))
        .unwrap()
        .execute_batch(
            "INSERT INTO forget_history(job, token) VALUES ('j1', 't1');
             INSERT INTO denied_imports(kind, key, job) VALUES ('ws', 'claude-mem:test:w1', 'j1');",
        )
        .unwrap();
    let read = |raw: &crate::raw::Raw| -> (Vec<String>, Vec<String>) {
        let task = |f: &serde_json::Value| f["task"].as_str().unwrap().to_owned();
        let entries = raw.work_state_entries(repo).unwrap();
        let stops = raw
            .imported_list_fields(crate::work_state::AGENT_LIFECYCLE, &["s1"])
            .unwrap();
        (
            (entries.iter())
                .map(|e| task(&serde_json::Value::Object(e.fields.clone())))
                .collect(),
            stops.iter().map(task).collect(),
        )
    };
    assert_eq!(
        read(&raw),
        (
            vec!["w2".into(), "own-1".into(), "own-2".into()],
            vec!["w2".into()]
        )
    );
    Connection::open(home.path().join("raw.db"))
        .unwrap()
        .execute_batch("DROP TABLE denied_imports")
        .unwrap();
    assert_eq!(
        read(&raw),
        (
            vec!["w1".into(), "w2".into(), "own-1".into(), "own-2".into()],
            vec!["w1".into(), "w2".into()]
        )
    );
}
