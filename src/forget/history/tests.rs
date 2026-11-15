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
    /// It reads them for no reader: why.
    Without(&'static str),
    /// A reader not yet made to take the proof: which surface. Registration (5b) waits for none
    /// to be left.
    Later(&'static str),
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
        Reach::Later("the embedding phase, which sends the text"),
    ),
    (
        "src/embed_phase.rs",
        "read_page",
        Reach::Later("the embedding phase"),
    ),
    (
        "src/embed_phase.rs",
        "current",
        Reach::Later("the embedding phase"),
    ),
    (
        "src/forget.rs",
        "uid_preview",
        Reach::Later("forget --uid's preview"),
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
    (
        "src/view.rs",
        "notes_page",
        Reach::Later("the viewer's feed"),
    ),
    (
        "src/view.rs",
        "REPOS",
        Reach::Later("the viewer's repositories"),
    ),
    (
        "src/view.rs",
        "REPO_NAMES",
        Reach::Later("the viewer's privacy page"),
    ),
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
/// says takes the proof does, so no new reader comes in unguarded and none loses its guard. A
/// reader listed for later that takes the proof is moved to `Proof`.
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
            Reach::Without(why) | Reach::Later(why) => {
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
