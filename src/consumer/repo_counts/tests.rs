use super::*;
use crate::search::b::fixture::Store;

/// What the tables count, leg by leg, as `view::REPOS` counts them: what `repo_counts` holds.
const COUNTED: &str = "SELECT repo, 'c', COUNT(DISTINCT uid), MAX(valid_from) FROM derivations
       WHERE repo IS NOT NULL GROUP BY repo
     UNION ALL SELECT repo, 'i', COUNT(DISTINCT uid), MAX(ts) FROM imported GROUP BY repo
     UNION ALL SELECT repo, 'r', COUNT(*), MAX(ts) FROM raw_docs WHERE repo IS NOT NULL GROUP BY repo
     ORDER BY 1, 2";

const KEPT: &str = "SELECT repo, leg, n, last FROM repo_counts ORDER BY repo, leg";

fn rows(k: &Connection, sql: &str) -> Vec<(String, String, i64, i64)> {
    let mut st = k.prepare(sql).unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// `k`'s counts are this build's, and what the tables count.
fn agrees(k: &Connection, why: &str) {
    assert!(kept(k).unwrap(), "not kept: {why}");
    assert_eq!(rows(k, KEPT), rows(k, COUNTED), "{why}");
}

/// A store the worker made: records, a claim and an imported document, in three repositories.
fn store() -> Store {
    let mut s = Store::new();
    s.said("s1", "github.com/o/a", 1_000, "First words.");
    s.decided("github.com/o/b", 2_000, "We ship on Fridays.", &[]);
    s.imported("i1", "p", 3_000, "A note", "A document.");
    s.run();
    s
}

/// The Fts consumer's first step makes the counts from the tables; with any of this build's table
/// and triggers gone, or another build's in the store (another body, a trigger more, a table of
/// another shape), the store is not counted from them, and the next step makes this build's again,
/// with every other trigger of theirs gone.
#[test]
fn the_counts_are_made_again_unless_the_store_holds_this_builds_own() {
    let s = store();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    agrees(&k, "the first pass");
    let before = rows(&k, KEPT);
    assert_eq!(before.len(), 4, "{before:?}");
    for change in [
        "DROP TRIGGER repo_counts_c_drop",
        "DROP TRIGGER repo_counts_r_add;
         CREATE TRIGGER repo_counts_r_add AFTER INSERT ON raw_docs BEGIN SELECT 1; END",
        "CREATE TRIGGER repo_counts_r_more AFTER INSERT ON raw_docs BEGIN SELECT 1; END",
        "DROP TABLE repo_counts;
         CREATE TABLE repo_counts (repo TEXT, leg TEXT, n INTEGER, last INTEGER)",
        // The local Codex review of #1068: a trigger of the namespace in another case, one more
        // that would count a record twice, and one of these whose name another build cased.
        "CREATE TRIGGER REPO_COUNTS_R_MORE AFTER INSERT ON raw_docs WHEN NEW.repo IS NOT NULL
         BEGIN UPDATE repo_counts SET n = n + 1 WHERE repo = NEW.repo AND leg = 'r'; END",
        "DROP TRIGGER repo_counts_r_add;
         CREATE TRIGGER Repo_Counts_R_Add AFTER INSERT ON raw_docs BEGIN SELECT 1; END",
    ] {
        k.execute_batch(change).unwrap();
        assert!(!kept(&k).unwrap(), "{change}");
        keep(&k).unwrap();
        agrees(&k, change);
        assert_eq!(rows(&k, KEPT), before, "{change}");
        let more: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE lower(name) = 'repo_counts_r_more' OR name = 'Repo_Counts_R_Add'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(more, 0, "{change}");
    }
}

/// The counts stay what the tables count through every kind of row written: inserts, a uid twice
/// in one repository and in two, rows of no repository, and deletes of one row, of a repository's
/// rows and of the rows past a point (a rewind).
#[test]
fn the_counts_stay_the_tables_through_inserts_and_deletes() {
    let s = store();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let repos = [Some("ra"), Some("rb"), Some("rc"), None];
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let tables = ["raw_docs", "derivations", "imported"];
    let mut legs = std::collections::BTreeSet::new();
    for step in 0..400_i64 {
        let op = next(10);
        let repo = repos[next(4) as usize];
        let uid = format!("u{}", next(6));
        let ts = next(10_000) as i64;
        let seq = 10_000 + step;
        let done = match op {
            0..=2 => k.execute(
                "INSERT INTO raw_docs(device, seq, kind, ts, repo, session)
                 VALUES ('d', ?1, 'prompt', ?2, ?3, 's')",
                rusqlite::params![seq, ts, repo],
            ),
            3 | 4 => k.execute(
                "INSERT INTO derivations(op_device, op_seq, uid, ts, tier, recipe, kind, status,
                   speaker, scope, repo, body, valid_from, anchor_device, anchor_seq)
                 VALUES ('d', ?1, ?2, ?3, 1, 't', 'decision', 'decided', 'user', 'repo', ?4, 'b',
                   ?3, 'd', 1)",
                rusqlite::params![seq, uid, ts, repo],
            ),
            5 | 6 => k.execute(
                "INSERT INTO imported(op_device, op_seq, uid, source, source_id, kind, repo,
                   session, ts, title, body)
                 VALUES ('d', ?1, ?2, 's', ?2, 'decision', ?3, 's', ?4, '', '')",
                rusqlite::params![seq, uid, repo.unwrap_or("ri"), ts],
            ),
            7 => {
                let table = tables[next(3) as usize];
                let n: i64 = k
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                k.execute(
                    &format!(
                        "DELETE FROM {table} WHERE rowid =
                           (SELECT rowid FROM {table} ORDER BY rowid LIMIT 1 OFFSET ?1)"
                    ),
                    [next(n.max(1) as u64) as i64],
                )
            }
            8 => {
                let table = tables[next(3) as usize];
                let repo = if table == "imported" {
                    Some(repo.unwrap_or("ri"))
                } else {
                    repo
                };
                k.execute(
                    &format!("DELETE FROM {table} WHERE repo IS ?1"),
                    rusqlite::params![repo],
                )
            }
            _ => {
                let (table, column) = [
                    ("raw_docs", "seq"),
                    ("derivations", "op_seq"),
                    ("imported", "op_seq"),
                ][next(3) as usize];
                k.execute(
                    &format!("DELETE FROM {table} WHERE {column} > ?1"),
                    [10_000 + step - next(40) as i64],
                )
            }
        };
        done.unwrap();
        agrees(&k, &format!("step {step}, op {op}"));
        legs.extend(rows(&k, KEPT).into_iter().map(|r| r.1));
    }
    assert_eq!(legs.len(), 3, "a leg never kept anything: {legs:?}");
}

/// Each lookup a trigger makes reads an index: a uid its table's uid index (`+repo` keeps it there),
/// a newest time its repository's index; none reads a table.
#[test]
fn every_lookup_of_the_triggers_reads_an_index() {
    let s = store();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let inner = regex::Regex::new(r"\((SELECT (?:[^()]|\([^()]*\))*)\)").unwrap();
    let row = regex::Regex::new(r"\b(?:NEW|OLD)\.\w+").unwrap();
    let mut seen = Vec::new();
    for sql in TRIGGERS {
        for c in inner.captures_iter(sql) {
            let lookup = row.replace_all(&c[1], "'x'").into_owned();
            let plan: Vec<String> = (k.prepare(&format!("EXPLAIN QUERY PLAN {lookup}")).unwrap())
                .query_map([], |r| r.get(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let index = [
                ("FROM derivations WHERE uid", "derivations_uid"),
                ("FROM imported WHERE uid", "imported_uid"),
                ("MAX(ts) FROM raw_docs", "raw_docs_repo"),
                ("MAX(valid_from) FROM derivations", "derivations_repo"),
                ("MAX(ts) FROM imported", "imported_time"),
            ]
            .into_iter()
            .find(|(what, _)| lookup.contains(what))
            .map(|(_, index)| index)
            .unwrap_or_else(|| panic!("a lookup this test does not know: {lookup}"));
            assert!(
                plan.iter().any(|p| p.contains(&format!("INDEX {index} "))),
                "{lookup}: {plan:?}"
            );
            assert!(
                plan.iter().all(|p| !p.starts_with("SCAN")),
                "{lookup}: {plan:?}"
            );
            seen.push(lookup);
        }
    }
    assert_eq!(seen.len(), 7, "{seen:?}");
}

/// A column counted is never updated in place: the update aborts, so a writer that would make the
/// counts stale fails, and the counts stay right. Another column's update goes through.
#[test]
fn an_update_of_a_column_counted_aborts() {
    let s = store();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    assert_eq!(
        k.execute("UPDATE derivations SET status = 'proposed'", [])
            .unwrap(),
        1
    );
    for (table, column) in [
        ("raw_docs", "repo"),
        ("raw_docs", "ts"),
        ("derivations", "repo"),
        ("derivations", "uid"),
        ("derivations", "valid_from"),
        ("imported", "repo"),
        ("imported", "uid"),
        ("imported", "ts"),
    ] {
        let e = k
            .execute(&format!("UPDATE {table} SET {column} = {column}"), [])
            .unwrap_err();
        assert!(
            e.to_string().contains("repo_counts counts"),
            "{table}.{column}: {e}"
        );
    }
    agrees(&k, "after the updates");
}

/// No writer of `src/` replaces a row of a table counted: `INSERT OR REPLACE` deletes the row it
/// replaces with no trigger fired (recursive triggers are off), and counts it again. A file is read
/// whole, so a statement over lines is seen, and so are a schema's and a quoted table's names.
#[test]
fn no_writer_replaces_a_row_counted() {
    let replace = regex::Regex::new(
        r#"(?i)\bREPLACE\s+INTO\s+(?:(?:main|temp)\s*\.\s*)?["`\[]?(?:(?:raw_docs|derivations|imported)\b|\{)"#,
    )
    .unwrap();
    for (text, found) in [
        (
            "INSERT OR REPLACE INTO\n           raw_docs(device) VALUES (?1)",
            true,
        ),
        ("REPLACE INTO main.derivations VALUES (1)", true),
        ("INSERT OR REPLACE INTO \"imported\" VALUES (1)", true),
        ("REPLACE INTO `raw_docs` VALUES (1)", true),
        ("replace into [derivations] values (1)", true),
        ("INSERT OR REPLACE INTO {} VALUES (1)", true),
        ("INSERT INTO raw_docs VALUES (1)", false),
        ("INSERT OR REPLACE INTO meta VALUES (1)", false),
    ] {
        assert_eq!(replace.is_match(text), found, "{text}");
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dirs = vec![root.join("src")];
    let mut found = Vec::new();
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
                let text = std::fs::read_to_string(&path).unwrap();
                for m in replace.find_iter(&text) {
                    let line = text[..m.start()].matches('\n').count() + 1;
                    found.push(format!("{}:{line}", path.display()));
                }
            }
        }
    }
    assert!(found.is_empty(), "{found:?}");
}
