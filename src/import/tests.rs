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

/// The documents' import ops raw holds, in op order.
fn docs(raw: &Raw) -> Vec<ImportDoc> {
    raw.ops_after(raw.device(), 0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == OpKind::Import && !crate::raw::is_import_files(&o.body))
        .map(|o| serde_json::from_value(o.body).unwrap())
        .collect()
}

/// The files ops raw holds (docs/file-note.md F10), in op order.
fn files_ops(raw: &Raw) -> Vec<ImportFiles> {
    raw.ops_after(raw.device(), 0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == OpKind::Import && crate::raw::is_import_files(&o.body))
        .map(|o| serde_json::from_value(o.body).unwrap())
        .collect()
}

/// `claude_mem_db` with claude-mem's lists of files, and `lists` (observation id, read,
/// modified) in them.
fn with_files(path: &Path, lists: &[(i64, serde_json::Value, serde_json::Value)]) {
    let c = Connection::open(path).unwrap();
    let has: bool = c
        .query_row(
            "SELECT count(*) > 0 FROM pragma_table_info('observations') WHERE name = 'files_read'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !has {
        c.execute_batch(
            "ALTER TABLE observations ADD COLUMN files_read TEXT;
                 ALTER TABLE observations ADD COLUMN files_modified TEXT;",
        )
        .unwrap();
    }
    for (id, read, modified) in lists {
        c.execute(
            "UPDATE observations SET files_read = ?2, files_modified = ?3 WHERE id = ?1",
            params![id, read.to_string(), modified.to_string()],
        )
        .unwrap();
    }
}

/// docs/file-note.md F10 (test 15): an observation that names files gets a files op after its
/// document's, its paths gated and kept within `FILES_BYTES` as stored; one whose lists are
/// none, or whose only path does not fit, gets none.
#[test]
fn an_observations_files_come_in_as_a_files_op_beside_its_document() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    // Paths JSON writes longer than they are: a quote, a backslash and a control character.
    let odd: Vec<String> = (0..2_000).map(|i| format!("a\"\\\u{1}{i}.rs")).collect();
    with_files(
        &src,
        &[
            (
                10,
                json!(["src/a.rs", " ", 7, format!("{key}/b.rs")]),
                json!(["src/a.rs"]),
            ),
            (13, json!("src/c.rs"), json!({"x": 1})),
            (14, json!(odd), json!(["src/d.rs"])),
            (15, json!(["x".repeat(FILES_BYTES)]), json!([])),
            // Kept while they fit: a short path after one that did not fit is left out too.
            (
                16,
                json!(["y".repeat(FILES_BYTES / 2), "z".repeat(FILES_BYTES)]),
                json!(["m.rs"]),
            ),
        ],
    );
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    let files = files_ops(&raw);
    assert_eq!(
        files
            .iter()
            .map(|f| f.source_id.as_str())
            .collect::<Vec<_>>(),
        ["o10", "o14", "o16"]
    );
    let a = &files[0];
    assert_eq!(a.files_read[0], "src/a.rs");
    assert_eq!(
        a.files_read.len(),
        2,
        "the blank and the number are no paths"
    );
    assert!(a.files_read[1].ends_with("/b.rs") && !a.files_read[1].contains(&key));
    assert_eq!(a.files_modified, ["src/a.rs"]);
    let doc = docs(&raw)
        .into_iter()
        .find(|d| d.source_id == "o10")
        .unwrap();
    assert_eq!(
        (&a.uid, &a.repo, &a.session),
        (&doc.uid, &doc.repo, &doc.session)
    );
    // The cut: kept in order, read first, within the bytes as stored.
    let b = &files[1];
    let stored = serde_json::to_string(b).unwrap().len();
    assert!(stored <= FILES_BYTES, "{stored}");
    assert!(b.files_read.len() < odd.len() && b.files_modified.is_empty());
    assert_eq!(b.files_read[..], odd[..b.files_read.len()]);
    let short = &files[2];
    assert_eq!((short.files_read.len(), short.files_modified.len()), (1, 0));
    let cut = (odd.len() - b.files_read.len() + 1) as u64;
    assert_eq!((stats.files, stats.files_cut), (3, cut + 1 + 2));
    // In op order, each after its document, and none reads as a document.
    let ops = raw.ops_after(raw.device(), 0, 10_000).unwrap();
    let at = |id: &str, files: bool| {
        ops.iter()
            .position(|o| {
                o.body["source_id"] == id && crate::raw::is_import_files(&o.body) == files
            })
            .unwrap()
    };
    assert!(at("o10", false) < at("o10", true) && at("o14", false) < at("o14", true));
    for o in ops.iter().filter(|o| crate::raw::is_import_files(&o.body)) {
        assert!(serde_json::from_value::<ImportDoc>(o.body.clone()).is_err());
        assert!(o.body.get("body").is_none() && o.body.get("ts").is_none());
    }
}

/// F11 (test 16): `--files` brings the files of the observations imported before and nothing
/// else, once, none of a forgotten or denied uid; a plain import brings what an earlier run
/// left out, and a document claude-mem moved gets its files under its new repository.
#[test]
fn the_files_of_observations_imported_before_come_in_once() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let homes = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut raws: Vec<Raw> = homes
        .iter()
        .map(|h| crate::raw::open(h.path()).unwrap())
        .collect();
    // Imported before claude-mem's files came in, as this PC's store was.
    for raw in &mut raws {
        claude_mem(raw, &src, &Default::default()).unwrap();
        assert!(files_ops(raw).is_empty());
    }
    with_files(
        &src,
        &[
            (10, json!(["src/a.rs"]), json!([])),
            (11, json!(["src/b.rs"]), json!([])),
            (13, json!(["src/c.rs"]), json!([])),
            (14, json!([]), json!(["src/d.rs"])),
        ],
    );
    let c = Connection::open(&src).unwrap();
    c.execute(
        "INSERT INTO observations VALUES(40, 'mem-1', 'free-mem', 1700, 'bugfix', 'New',
               'Since the last import.', '', '[\"src/e.rs\"]', '[]')",
        [],
    )
    .unwrap();
    // Work state claude-mem kept since, which `--files` leaves to the plain import too.
    c.execute_batch(
            "CREATE TABLE work_state_entries(id INTEGER PRIMARY KEY AUTOINCREMENT,
               project TEXT NOT NULL, list_name TEXT NOT NULL, fields TEXT NOT NULL,
               created_at TEXT NOT NULL, created_at_epoch INTEGER NOT NULL);
             INSERT INTO work_state_entries(project, list_name, fields, created_at,
               created_at_epoch) VALUES('free-mem', 'plan', '{\"task\": \"Port search\"}', '', 3000);",
        )
        .unwrap();
    let ids =
        |raw: &Raw| -> Vec<String> { files_ops(raw).into_iter().map(|f| f.source_id).collect() };
    let [plain, only] = &mut raws[..] else {
        unreachable!()
    };
    // A forgotten document gets no files op, nor one whose origin is denied.
    let forgotten = docs(only)
        .into_iter()
        .find(|d| d.source_id == "o13")
        .unwrap();
    only.append_ops(&[(OpKind::Forget, json!({"uid": forgotten.uid, "job": "j"}))])
        .unwrap();
    Connection::open(homes[1].path().join("raw.db"))
        .unwrap()
        .execute(
            "INSERT INTO denied_records(origin, device, seq, ts, session, job)
                 VALUES(?1, 'd', 1, 0, 's', 'j')",
            [crate::forget::origin(&forgotten.source, "o11")],
        )
        .unwrap();
    let before = docs(only).len();
    let stats = claude_mem_with(only, &src, &Default::default(), true).unwrap();
    assert_eq!(ids(only), ["o10", "o14"]);
    assert_eq!(docs(only).len(), before, "no document");
    assert_eq!((stats.observations, stats.work_state), (0, 0));
    // Six imported before (o12 is empty); o40, recorded since, is left to the plain import.
    assert_eq!((counts(&stats), stats.files), ((0, 0, 0, 6, 1), 2));
    let again = claude_mem_with(only, &src, &Default::default(), true).unwrap();
    assert_eq!((ids(only).len(), again.files), (2, 0), "once");
    let plain_run = claude_mem(only, &src, &Default::default()).unwrap();
    assert_eq!(
        ids(only),
        ["o10", "o14", "o40"],
        "the new one with its document"
    );
    assert_eq!((plain_run.files, plain_run.work_state), (1, 1));
    // The plain import brings what the first run, before the lists, could not.
    claude_mem(plain, &src, &Default::default()).unwrap();
    assert_eq!(ids(plain), ["o10", "o11", "o13", "o14", "o40"]);
    claude_mem(plain, &src, &Default::default()).unwrap();
    assert_eq!(ids(plain).len(), 5);
    // Moved: its document and its files again, under the new project; `--files`, which
    // moves no document, has its files op already where the document is filed.
    c.execute(
        "UPDATE observations SET project = 'other' WHERE id = 10",
        [],
    )
    .unwrap();
    claude_mem_with(plain, &src, &Default::default(), true).unwrap();
    assert_eq!(ids(plain).len(), 5);
    claude_mem(plain, &src, &Default::default()).unwrap();
    let moved: Vec<String> = files_ops(plain)
        .into_iter()
        .filter(|f| f.source_id == "o10")
        .map(|f| f.repo)
        .collect();
    assert_eq!(moved, [repo("free-mem"), repo("other")]);
}

/// F11 (test 16): a document imported before claude-mem's files came in, and moved to another
/// project since, gets its files op from `--files` where it is filed, once; the plain import
/// then moves it and files its files again under the new project.
#[test]
fn files_only_files_a_moved_documents_files_where_it_is_filed() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    claude_mem(&mut raw, &src, &Default::default()).unwrap();
    with_files(&src, &[(10, json!(["src/a.rs"]), json!([]))]);
    Connection::open(&src)
        .unwrap()
        .execute(
            "UPDATE observations SET project = 'other' WHERE id = 10",
            [],
        )
        .unwrap();
    let repos = |raw: &Raw| -> Vec<String> { files_ops(raw).into_iter().map(|f| f.repo).collect() };
    let stats = claude_mem_with(&mut raw, &src, &Default::default(), true).unwrap();
    assert_eq!(repos(&raw), [repo("free-mem")]);
    assert_eq!((stats.files, stats.seen), (1, 6), "imported before: seen");
    claude_mem_with(&mut raw, &src, &Default::default(), true).unwrap();
    assert_eq!(repos(&raw), [repo("free-mem")], "once");
    claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(repos(&raw), [repo("free-mem"), repo("other")]);
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

/// Adds a prompt of session `agent-1` to `claude_mem_db`'s database.
fn prompt(path: &Path, id: i64, text: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO user_prompts VALUES(?1, 'agent-1', 1200, ?2)",
            params![id, text],
        )
        .unwrap();
}

/// A home whose config.toml is `toml`, and its capture settings.
fn home_with(toml: &str) -> (tempfile::TempDir, crate::capture::Settings) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), toml).unwrap();
    let settings = crate::capture::Settings::load(home.path()).unwrap();
    (home, settings)
}

/// A prompt is gated with the home's rules too, as a title and a body are: `clip` masked it
/// with the bundled rules alone, so a value only a rule of the home's finds was stored as
/// typed. What is stored is what `clip` kept, with the rule's values masked in it: under no
/// rule of the home's it is the text stored before, also for the key blocks, cut short and
/// whole about the cut, that a security review of this change tried.
#[test]
fn a_prompt_is_gated_with_the_homes_rules_too() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let lines = format!("{}\n", "Q9zX8mL2vB4nR7tY1wK3pS6dJ0aF5hU2".repeat(2)).repeat(100);
    let before = "a".repeat(7_960);
    let texts = [
        "Deploy acme-482915 to staging".to_owned(),
        // Longer than `clip` keeps, the value before its cut.
        format!("ship acme-482915 {}", "a".repeat(11_000)),
        format!("push with {key}"),
        format!("{before} -----BEGIN RSA PRIVATE KEY-----\n{lines}"),
        format!("{before} -----BEGIN RSA PRIVATE KEY-----\n{lines}-----END RSA PRIVA"),
        format!(
            "{before} -----BEGIN RSA PRIVATE KEY-----\n{lines}-----BEGIN OPENSSH PRIVATE \
                 KEY-----\n{lines}-----END OPENSSH PRIVATE KEY-----\n"
        ),
        format!(
            "{}-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            "a".repeat(7_000),
            "B".repeat(6_000)
        ),
    ];
    for (i, text) in texts.iter().enumerate() {
        prompt(&src, 32 + i as i64, text);
    }
    let bodies = |settings: &crate::capture::Settings, home: &Path| -> Vec<String> {
        let mut raw = crate::raw::open(home).unwrap();
        raw.gates_with(&settings.rules);
        claude_mem(&mut raw, &src, settings).unwrap();
        let stored = docs(&raw);
        (0..texts.len())
            .map(|i| {
                let id = format!("p{}", 32 + i);
                stored
                    .iter()
                    .find(|d| d.source_id == id)
                    .unwrap()
                    .body
                    .clone()
            })
            .collect()
    };
    // The text stored before this change: `clip`'s.
    let (home, plain) = home_with("");
    let was: Vec<String> = texts
        .iter()
        .map(|t| hook::clip(&hook::strip_blocks(t, true), &plain.rules))
        .collect();
    assert_eq!(was[2], "push with [REDACTED]");
    assert!(was[1].ends_with("\n…[clipped, 11017 chars in full]"));
    assert!(
        was.iter()
            .all(|w| !w.contains("Q9zX8mL2") && !w.contains("BBBB"))
    );
    assert_eq!(bodies(&plain, home.path()), was);
    // Under a rule of the home's: the same, with its value masked. It is masked before the cut
    // (#994), which keeps 8,000 characters of the masked text: one more `a` in the long prompt,
    // whose mask is a character shorter than its value.
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    let (home, ruled) = home_with(rule);
    let mut masked: Vec<String> = was
        .iter()
        .map(|w| w.replace("acme-482915", "[REDACTED]"))
        .collect();
    assert_ne!(masked[..2], was[..2]);
    masked[1] = format!(
        "ship [REDACTED] {}\n…[clipped, 11017 chars in full]",
        "a".repeat(7_984)
    );
    assert_eq!(bodies(&ruled, home.path()), masked);
    // Owner decision 43: while redaction is off, a prompt is kept as typed.
    let (home, off) = home_with("[redaction]\nenabled = false\n");
    let typed = bodies(&off, home.path());
    assert_eq!(typed[0], texts[0]);
    assert_eq!(typed[2], texts[2]);
}

/// A value only a rule of the home's finds, with the cut at 8,000 characters inside it:
/// nothing of it is stored, and one mask stands for it, whole. `clip` kept its start, and the
/// home's gate, which runs on what was kept, no longer took those characters for the value.
/// Under no rule of the home's the cut is where it was.
#[test]
fn a_prompt_cut_inside_a_value_of_the_homes_rule_keeps_none_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let text = format!("{}acme-482915 {}", "a".repeat(7_992), "b".repeat(3_000));
    prompt(&src, 32, &text);
    let body = |settings: &crate::capture::Settings, home: &Path| -> String {
        let mut raw = crate::raw::open(home).unwrap();
        raw.gates_with(&settings.rules);
        claude_mem(&mut raw, &src, settings).unwrap();
        let stored = docs(&raw);
        stored
            .iter()
            .find(|d| d.source_id == "p32")
            .unwrap()
            .body
            .clone()
    };
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    let (home, ruled) = home_with(rule);
    let stored = body(&ruled, home.path());
    assert!(!stored.contains("acme"), "{}", &stored[7_980..]);
    assert_eq!(
        stored,
        format!(
            "{}[REDACTED]\n…[clipped, 11004 chars in full]",
            "a".repeat(7_992)
        )
    );
    let (home, plain) = home_with("");
    assert_eq!(
        body(&plain, home.path()),
        format!(
            "{}acme-482\n…[clipped, 11004 chars in full]",
            "a".repeat(7_992)
        )
    );
}

/// The home's rules read all of an imported prompt as it came, before the bundled rules mask it
/// and before the cut, as capture's scan reads a live prompt (#994). So nothing stays of a value
/// that ends far past the cut, of one whose rule needs the text after it, of one that holds a
/// value the bundled rules mask (cut or not), or of one a second rule finds behind the first
/// rule's mask. Each was stored in part: the rules read what was kept, masked by the bundled
/// rules.
#[test]
fn the_homes_rules_read_all_of_an_imported_prompt_as_it_came() {
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let vault = "[[redaction.extra_rules]]\nid = \"vault\"\nregex = 'vault\\{[^}]*\\}'\n";
    let ended = "[[redaction.extra_rules]]\nid = \"vault\"\nregex = 'vault\\{([^}]+)\\} end'\n";
    let holds = "[[redaction.extra_rules]]\nid = \"vault\"\n\
                 regex = 'vault\\{ghp_[a-zA-Z0-9]{36}:[^}]*\\}'\n";
    let chain = "[[redaction.extra_rules]]\nid = \"foo\"\nregex = 'foo'\n\
                 [[redaction.extra_rules]]\nid = \"tok\"\n\
                 regex = '\\[REDACTED\\]token-([0-9]{6})'\n";
    let a = |n: usize| "a".repeat(n);
    // The rule, the prompt, and what is stored of it.
    let cases: Vec<(&str, String, String)> = vec![
        (
            vault,
            format!("{}vault{{{}}} done", a(7_990), "s".repeat(5_000)),
            format!("{}[REDACTED]\n…[clipped, 13002 chars in full]", a(7_990)),
        ),
        (
            ended,
            format!("{}vault{{123456}} end {}", a(7_984), "b".repeat(3_000)),
            format!(
                "{}vault{{[REDACTED]\n…[clipped, 11002 chars in full]",
                a(7_984)
            ),
        ),
        (
            holds,
            format!("{}vault{{{token}:{}}} done", a(7_900), "s".repeat(5_000)),
            format!(
                "{}[REDACTED] done\n…[clipped, 12953 chars in full]",
                a(7_900)
            ),
        ),
        (
            holds,
            format!("x vault{{{token}:shortsecret}} y"),
            "x [REDACTED] y".to_owned(),
        ),
        (
            chain,
            format!("{}footoken-123456 {}", a(7_983), "b".repeat(3_000)),
            String::new(),
        ),
    ];
    for (n, (rule, text, stored)) in cases.into_iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        prompt(&src, 41, &text);
        let (home, ruled) = home_with(rule);
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.gates_with(&ruled.rules);
        claude_mem(&mut raw, &src, &ruled).unwrap();
        let docs = docs(&raw);
        let body = &docs.iter().find(|d| d.source_id == "p41").unwrap().body;
        let end = &body[body.len().saturating_sub(60)..];
        if stored.is_empty() {
            // The second rule's value: no digit of it, whatever masks stand for the rest.
            let head = body.strip_suffix("\n…[clipped, 10999 chars in full]");
            assert!(
                head.is_some_and(|head| !head.contains(|c: char| c.is_ascii_digit())),
                "{n}: {end}"
            );
        } else {
            assert_eq!(body, &stored, "{n}: {end}");
        }
    }
}

/// Two edges of the read of all of a prompt. A prompt with more matches of one rule than a
/// scan holds is one mask, as capture stores a live prompt with that many: `redact::ranges`
/// then gives one span for all of the text, which is not told from a value that is the whole
/// prompt. Only a rule of the home's is counted so: a bundled rule's matches are a span each,
/// however many, and a prompt with that many of them past the window keeps its start under a
/// rule of the home's as it does under none. A key block the cut leaves open still goes when a
/// rule of the home's masks its header: the span that holds a header is left to the bundled
/// rules and to the check for it.
#[test]
fn a_prompt_read_whole_masks_too_many_matches_and_drops_an_open_key_block() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let tail = format!("{}{}", "a".repeat(12_000), " acme-123456".repeat(1_001));
    prompt(&src, 41, &tail);
    let key = format!(
        "{}-----BEGIN PRIVATE KEY-----\n{}",
        "a".repeat(7_000),
        "B".repeat(6_000)
    );
    prompt(&src, 42, &key);
    // Too many matches in a prompt that also holds a header: one mask all the same.
    prompt(&src, 43, &format!("-----BEGIN PRIVATE KEY-----\n{tail}"));
    // Too many of a value only the bundled rules find, all of them past the window.
    let token = format!(" ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    prompt(
        &src,
        44,
        &format!("{}{}", "a".repeat(12_000), token.repeat(1_001)),
    );
    let rule = "[[redaction.extra_rules]]\nid = \"acme\"\nregex = 'acme-[0-9]{6}'\n\
                [[redaction.extra_rules]]\nid = \"header\"\nregex = '-----BEGIN [A-Z ]+-----'\n";
    let (home, ruled) = home_with(rule);
    let mut raw = crate::raw::open(home.path()).unwrap();
    raw.gates_with(&ruled.rules);
    claude_mem(&mut raw, &src, &ruled).unwrap();
    let docs = docs(&raw);
    let body = |id: &str| {
        docs.iter()
            .find(|d| d.source_id == id)
            .unwrap()
            .body
            .clone()
    };
    assert_eq!(body("p41"), "[REDACTED]\n…[clipped, 24012 chars in full]");
    assert_eq!(body("p43"), "[REDACTED]\n…[clipped, 24040 chars in full]");
    let start = format!("{}\n…[clipped, 53041 chars in full]", "a".repeat(8_000));
    assert_eq!(body("p44"), start);
    assert_eq!(
        body("p42"),
        format!("{}\n…[clipped, 13028 chars in full]", "a".repeat(7_000))
    );
    let (home, plain) = home_with("");
    let mut raw = crate::raw::open(home.path()).unwrap();
    claude_mem(&mut raw, &src, &plain).unwrap();
    let kept = self::docs(&raw)
        .into_iter()
        .find(|d| d.source_id == "p44")
        .unwrap()
        .body;
    assert_eq!(kept, start);
}

/// A harness notification is told before the home's rules run: a rule that masks its opening
/// tag does not make it a prompt (a security review of this change, 2026-11-12). A prompt
/// the home's rules leave nothing of is none.
#[test]
fn a_notification_stays_out_under_a_rule_that_masks_its_tag() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    prompt(
        &src,
        32,
        "<task-notification>\nAccess PIN: 7391\n</task-notification>",
    );
    prompt(&src, 33, "<private>all of it</private>");
    // Tags that close into a private block only at the home's gate, the third pass.
    prompt(
        &src,
        34,
        "<pri<pri<private>a</private>vate>b</private>vate>all of it</private>",
    );
    let rule = "[[redaction.extra_rules]]\nid = \"tag\"\nregex = 'task-notification'\n";
    let (home, ruled) = home_with(rule);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &ruled).unwrap();
    // The fixture's notification, this one, the two prompts that are all private, and o12.
    assert_eq!(counts(&stats), (6, 1, 1, 0, 5));
    let stored = op_bodies(&raw);
    assert!(
        stored
            .iter()
            .all(|op| !op.contains("7391") && !op.contains("all of it"))
    );
    let prompts: Vec<String> = docs(&raw)
        .into_iter()
        .filter(|d| d.kind == "prompt")
        .map(|d| d.source_id)
        .collect();
    assert_eq!(prompts, ["p30"]);
}

/// While `[capture] store_prompts` is off, the import stores no prompt's text either: the
/// rows are left out, and come in once it is on again.
#[test]
fn prompts_stay_out_while_store_prompts_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let (home, off) = home_with("[capture]\nstore_prompts = false\n");
    let mut raw = crate::raw::open(home.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &off).unwrap();
    assert_eq!(counts(&stats), (6, 1, 0, 0, 3));
    assert!(docs(&raw).iter().all(|d| d.kind != "prompt"));
    let on = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(counts(&on), (0, 0, 1, 7, 2));
    assert!(docs(&raw).iter().any(|d| d.source_id == "p30"));
}

/// Every op raw holds, as stored.
fn op_bodies(raw: &Raw) -> Vec<String> {
    raw.ops_after(raw.device(), 0, 10_000)
        .unwrap()
        .into_iter()
        .map(|o| o.body.to_string())
        .collect()
}

/// I10: a label is stored text too, and it names something. A row whose repository or session
/// the gate changes is left out and counted, document, files and work state alike: the label
/// was stored as claude-mem holds it, whatever the home's rules masked in it.
#[test]
fn a_row_under_a_label_the_gate_changes_is_left_out_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    Connection::open(&src)
        .unwrap()
        .execute(
            "INSERT INTO observations(id, memory_session_id, project, created_at_epoch, type,
                   title, narrative, facts)
                 VALUES(50, 'mem-9', 'other', 1800, 'change', 'Elsewhere', 'Kept.', '[]')",
            [],
        )
        .unwrap();
    with_files(
        &src,
        &[
            (10, json!(["src/a.rs"]), json!([])),
            (50, json!(["src/b.rs"]), json!([])),
        ],
    );
    // The repository's name.
    let (home, ruled) = home_with("[[redaction.extra_rules]]\nid = \"fm\"\nregex = 'free-mem'\n");
    let mut raw = crate::raw::open(home.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &ruled).unwrap();
    // Out: seven observations, the summary, the prompt and the four work state rows.
    assert_eq!((counts(&stats), stats.masked), ((1, 0, 0, 0, 2), 13));
    assert_eq!((stats.work_state, stats.refused, stats.files), (0, 0, 1));
    let stored = op_bodies(&raw);
    assert!(
        stored.iter().all(|op| !op.contains("free-mem")),
        "{stored:?}"
    );
    assert_eq!(docs(&raw).len(), 1);
    assert_eq!(files_ops(&raw)[0].repo, repo("other"));
    let again = claude_mem(&mut raw, &src, &ruled).unwrap();
    assert_eq!((counts(&again), again.masked), ((0, 0, 0, 1, 2), 13));
    // The session's id.
    let (home, ruled) = home_with("[[redaction.extra_rules]]\nid = \"s\"\nregex = 'agent-[0-9]'\n");
    let mut raw = crate::raw::open(home.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &ruled).unwrap();
    // Out: the session's four observations, its summary and its prompt.
    assert_eq!((counts(&stats), stats.masked), ((4, 0, 0, 0, 2), 6));
    let stored = op_bodies(&raw);
    assert!(
        stored.iter().all(|op| !op.contains("agent-1")),
        "{stored:?}"
    );
    let sessions: Vec<String> = docs(&raw).into_iter().map(|d| d.session).collect();
    assert_eq!(sessions.len(), 4, "{sessions:?}");
}

/// I10: the files op of a document filed before goes under the label the document was stored
/// under, which no gate read then: `--files`, and a row whose project claude-mem lost, leave
/// it out too.
#[test]
fn a_files_op_under_a_stored_label_the_gate_changes_is_left_out() {
    let rule = "[[redaction.extra_rules]]\nid = \"fm\"\nregex = 'free-mem'\n";
    for lost in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-mem.db");
        claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
        let (home, ruled) = home_with(rule);
        let mut raw = crate::raw::open(home.path()).unwrap();
        // Filed under no rule, before claude-mem kept an observation's files.
        claude_mem(&mut raw, &src, &Default::default()).unwrap();
        with_files(&src, &[(10, json!(["src/a.rs"]), json!([]))]);
        if lost {
            Connection::open(&src)
                .unwrap()
                .execute("UPDATE observations SET project = NULL WHERE id = 10", [])
                .unwrap();
        }
        let stats = claude_mem_with(&mut raw, &src, &ruled, !lost).unwrap();
        assert_eq!((stats.files, stats.masked), (0, 1), "lost: {lost}");
        assert!(files_ops(&raw).is_empty(), "lost: {lost}");
        // Under no rule it comes in.
        let plain = claude_mem_with(&mut raw, &src, &Default::default(), !lost).unwrap();
        assert_eq!((plain.files, plain.masked), (1, 0), "lost: {lost}");
        assert_eq!(files_ops(&raw)[0].repo, repo("free-mem"));
    }
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

/// I6, slice 4a (Codex on #489): a project whose repository is never recorded brings no work
/// state, its worktree's neither, by the name search reads them under (D13); its rows are
/// counted, and an import after the undo brings them.
#[test]
fn a_repository_never_recorded_keeps_its_imported_work_state_out() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    let mut raw = crate::raw::open(dir.path()).unwrap();
    raw.exclude_capture(("repo", "github.com/o/free-mem"), false)
        .unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(
        (stats.work_state, stats.refused, stats.never_recorded),
        (0, 0, 4)
    );
    assert!(work_ops(&raw).is_empty());
    raw.exclude_capture(("repo", "github.com/o/free-mem"), true)
        .unwrap();
    let after = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!((after.work_state, after.never_recorded), (2, 0));
}

/// Owner decision 43 (Codex's security review of the switch): the import gates every field
/// with its own settings' rules, which its writes owe the op text scan by: a rule they hold
/// masks a title (egress, read per row, would not here), and with redaction off a work state
/// row comes in as typed and the scan recorded before is owed again.
#[test]
fn the_import_gates_with_its_settings_and_owes_the_scan_when_they_are_off() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    Connection::open(&src)
        .unwrap()
        .execute(
            "INSERT INTO observations(id, memory_session_id, project, created_at_epoch, type,
                   title, narrative, facts)
                 VALUES(41, 'mem-1', 'free-mem', 1800, 'change', 'Deploy acme-482915', 'Done.',
                   '[]')",
            [],
        )
        .unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    let ruled_home = tempfile::tempdir().unwrap();
    std::fs::write(ruled_home.path().join("config.toml"), rule).unwrap();
    let ruled = crate::capture::Settings::load(ruled_home.path()).unwrap();
    let mut raw = crate::raw::open(ruled_home.path()).unwrap();
    claude_mem(&mut raw, &src, &ruled).unwrap();
    let titles: Vec<String> = docs(&raw).into_iter().map(|d| d.title).collect();
    assert!(
        titles.iter().any(|t| t == "Deploy [REDACTED]"),
        "{titles:?}"
    );
    let off_home = tempfile::tempdir().unwrap();
    std::fs::write(
        off_home.path().join("config.toml"),
        "[redaction]\nenabled = false\n",
    )
    .unwrap();
    let off = crate::capture::Settings::load(off_home.path()).unwrap();
    let mut raw = crate::raw::open(off_home.path()).unwrap();
    let mark = raw.op_text_scan_begun().unwrap();
    assert!(raw.ops_text_scanned("v", &mark).unwrap());
    raw.gates_with(&off.rules);
    claude_mem(&mut raw, &src, &off).unwrap();
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    assert!(work_ops(&raw).iter().any(|o| o.to_string().contains(&key)));
    assert!(raw.ops_text_owed("v").unwrap());
}

/// Codex on #489: a repository excluded after the import read the list refuses its rows in
/// the append; the batch's other rows still come in, and the refused ones are counted.
#[test]
fn an_exclusion_made_during_the_import_leaves_out_only_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(dir.path()).unwrap();
    let op = |project: &str, id: &str| {
        (
            OpKind::WorkState,
            json!({"repo": repo(project), "list": "plan",
                    "fields": {"task": "Port search", "status": "doing"}, "clock": 1_000,
                    "at": 1_000, "source": "claude-mem:b62f07076e19", "source_id": id}),
        )
    };
    let batch = [op("free-mem", "w1"), op("other", "w2")];
    // After the import's read of the list, before its append.
    raw.exclude_capture(("repo", "github.com/o/free-mem"), false)
        .unwrap();
    let mut stats = Stats {
        work_state: 2,
        ..Default::default()
    };
    append_work_state(&mut raw, &batch, &Default::default(), &mut stats).unwrap();
    assert_eq!((stats.work_state, stats.never_recorded), (1, 1));
    let ops = work_ops(&raw);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["source_id"], "w2");
}

/// Parity row 14 (CodeRabbit on #563): a pattern that names a claude-mem project keeps the
/// project's imported work state out, as a repository never recorded does.
#[test]
fn a_pattern_keeps_a_projects_imported_work_state_out() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    let mut raw = crate::raw::open(dir.path()).unwrap();
    raw.exclude_capture(("pattern", "free-*"), false).unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!((stats.work_state, stats.never_recorded), (0, 4));
    assert!(work_ops(&raw).is_empty());
}

/// Codex on #489: a repository excluded under a rule that masks part of its name keeps its
/// claude-mem project's work state out too: the project's name is matched as labelled now.
#[test]
fn a_masked_repository_name_keeps_its_imported_work_state_out() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    let rule = "[[redaction.extra_rules]]\nid = \"fm\"\nregex = 'free-mem'\n";
    std::fs::write(dir.path().join("config.toml"), rule).unwrap();
    let settings = crate::capture::Settings::load(dir.path()).unwrap();
    let label = crate::capture::exclusion_label("repo", "github.com/o/free-mem", &settings);
    assert!(!label.contains("free-mem"), "{label}");
    let mut raw = crate::raw::open(dir.path()).unwrap();
    raw.exclude_capture(("repo", &label), false).unwrap();
    let stats = claude_mem(&mut raw, &src, &settings).unwrap();
    assert_eq!((stats.work_state, stats.never_recorded), (0, 4));
    assert!(work_ops(&raw).is_empty());
}

/// Codex on #431: a row claude-mem merges into another project after it came in comes in
/// again under that project, with the same uid, and the index files it there (readers take a
/// document's newest row); a run after that adds nothing.
#[test]
fn a_row_merged_after_its_import_comes_in_again_under_its_project() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    let merge = |into: Option<&str>| {
        Connection::open(&src)
            .unwrap()
            .execute(
                "UPDATE observations SET merged_into_project = ?1 WHERE id = 40",
                [into],
            )
            .unwrap();
    };
    merge(None);
    let mut raw = crate::raw::open(dir.path()).unwrap();
    claude_mem(&mut raw, &src, &Default::default()).unwrap();
    let o40 = |raw: &Raw| -> Vec<String> {
        docs(raw)
            .into_iter()
            .filter(|d| d.source_id == "o40")
            .map(|d| d.repo)
            .collect()
    };
    assert_eq!(o40(&raw), ["claude-mem:free-mem/wt"]);
    merge(Some("free-mem"));
    let moved = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(moved.observations, 1);
    assert_eq!(o40(&raw), ["claude-mem:free-mem/wt", "claude-mem:free-mem"]);
    let again = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(again.observations, 0);
    assert_eq!(o40(&raw).len(), 2);
    drop(raw);
    crate::worker::run_once(dir.path()).unwrap();
    let k = crate::knowledge::open(dir.path()).unwrap();
    let uid: String = k
        .query_row(
            "SELECT uid FROM imported WHERE source_id = 'o40' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let line = crate::search::b::get(dir.path(), &uid).unwrap().unwrap();
    assert!(line.contains(" claude-mem:free-mem (imported from"));
    // The row's first import also on another device, at a higher op seq than this device's
    // (a copied store): op seqs order one device's ops only, and the row is filed under the
    // merged project already, so nothing more comes in (Codex on #431).
    let mut raw = crate::raw::open(dir.path()).unwrap();
    let first = raw
        .ops_after(raw.device(), 0, 10_000)
        .unwrap()
        .into_iter()
        .find(|o| o.kind == OpKind::Import && o.body["source_id"] == "o40")
        .unwrap();
    Connection::open(dir.path().join("raw.db"))
        .unwrap()
        .execute(
            "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES('elsewhere', 1000000, 'import', 0, ?1, 1000000)",
            [first.body.to_string()],
        )
        .unwrap();
    let copied = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(copied.observations, 0);
}

/// Test 1: a database at a schema newer than the import knows is refused, nothing written.
#[test]
fn a_database_newer_than_v74_is_refused_with_nothing_written() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    // The newest version is not the last row: claude-mem's media migration runs after the
    // others, so its v72 is stamped after v73 and v74.
    Connection::open(&src)
        .unwrap()
        .execute_batch(
            "INSERT INTO schema_versions VALUES(3, 65, '');
                 INSERT INTO schema_versions VALUES(4, 66, '');
                 INSERT INTO schema_versions VALUES(5, 67, '');
                 INSERT INTO schema_versions VALUES(6, 68, '');
                 INSERT INTO schema_versions VALUES(7, 69, '');
                 INSERT INTO schema_versions VALUES(8, 70, '');
                 INSERT INTO schema_versions VALUES(9, 71, '');
                 INSERT INTO schema_versions VALUES(10, 73, '');
                 INSERT INTO schema_versions VALUES(11, 74, '');
                 INSERT INTO schema_versions VALUES(12, 75, '');
                 INSERT INTO schema_versions VALUES(13, 72, '');",
        )
        .unwrap();
    let mut raw = crate::raw::open(dir.path()).unwrap();
    let said = claude_mem(&mut raw, &src, &Default::default())
        .err()
        .unwrap()
        .to_string();
    assert!(said.contains("v75") && said.contains("v74"), "{said}");
    assert!(docs(&raw).is_empty() && work_ops(&raw).is_empty());
}

/// I2: 13.36.0's v65, which only recomputes `observations.title_norm_key`, 13.40.0's v67,
/// which adds the work state's sync columns, 15.0.2's v68, which adds an index of the
/// observations' files, and 15.12.0's v74, with the tables of kept images, the records of
/// deleted sessions and their triggers, come in as v64 does: a path only that index holds
/// comes in nowhere, and neither does an image's link or a deleted session's record.
#[test]
fn a_v65_v67_v68_or_v74_database_comes_in_as_v64_does() {
    let dir = tempfile::tempdir().unwrap();
    let (v64, v65, v67, v68, v74) = (
        dir.path().join("v64.db"),
        dir.path().join("v65.db"),
        dir.path().join("v67.db"),
        dir.path().join("v68.db"),
        dir.path().join("v74.db"),
    );
    for db in [&v64, &v65, &v67, &v68, &v74] {
        claude_mem_v64(db, "2025-12-14T16:09:58.769Z");
    }
    let v65_batch = "ALTER TABLE observations ADD COLUMN title_norm_key TEXT;
             UPDATE observations SET title_norm_key = lower(title);
             INSERT INTO schema_versions VALUES(3, 65, '2026-10-13T00:22:16.544Z');";
    Connection::open(&v65)
        .unwrap()
        .execute_batch(v65_batch)
        .unwrap();
    let v67_batch = "ALTER TABLE work_state_entries ADD COLUMN synced_at INTEGER;
             ALTER TABLE work_state_entries ADD COLUMN origin_device_id TEXT;
             ALTER TABLE work_state_entries ADD COLUMN origin_local_id TEXT;
             ALTER TABLE work_state_entries ADD COLUMN sync_rev TEXT NOT NULL DEFAULT '1';
             INSERT INTO schema_versions VALUES(4, 66, '2026-10-16T14:58:42.000Z');
             INSERT INTO schema_versions VALUES(5, 67, '2026-10-16T14:58:42.000Z');";
    for db in [&v67, &v68, &v74] {
        let c = Connection::open(db).unwrap();
        c.execute_batch(v65_batch).unwrap();
        c.execute_batch(v67_batch).unwrap();
    }
    let v68_batch = "CREATE TABLE observation_files(path TEXT NOT NULL,
               observation_id INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
               PRIMARY KEY (path, observation_id));
             CREATE INDEX idx_observation_files_observation
               ON observation_files(observation_id);
             INSERT INTO observation_files VALUES('src/only-in-the-index.rs', 10);
             INSERT INTO schema_versions VALUES(6, 68, '2026-10-23T21:24:16.070Z');";
    for db in [&v68, &v74] {
        Connection::open(db)
            .unwrap()
            .execute_batch(v68_batch)
            .unwrap();
    }
    // claude-mem 15.15.0's tables, as `sqlite/media.ts` (v69 to v72) and `SessionStore.ts`
    // (v73, v74) make them, with a row in each a person's use fills, and its triggers on
    // the tables the import reads, their bodies shortened, made after the rows. An image is
    // linked to observation 10; a session and a memory session other than the fixture's
    // were deleted. The versions are stamped in the order a real database holds them: the
    // media migration runs last.
    Connection::open(&v74)
        .unwrap()
        .execute_batch(
            "CREATE TABLE tool_uses(id INTEGER PRIMARY KEY AUTOINCREMENT,
                   tool_use_id TEXT NOT NULL, content_session_id TEXT NOT NULL,
                   memory_session_id TEXT, session_db_id INTEGER, project TEXT NOT NULL,
                   platform_source TEXT NOT NULL DEFAULT 'claude', tool_name TEXT NOT NULL,
                   tool_input TEXT, tool_response TEXT, created_at TEXT NOT NULL,
                   created_at_epoch INTEGER NOT NULL, UNIQUE(content_session_id, tool_use_id));
                 CREATE TABLE media_events(event_key TEXT PRIMARY KEY,
                   session_db_id INTEGER NOT NULL, platform TEXT NOT NULL,
                   event_identity TEXT NOT NULL,
                   state TEXT NOT NULL DEFAULT 'retained' CHECK(state IN ('retained','deleted')),
                   result_state TEXT NOT NULL DEFAULT 'unprocessed'
                     CHECK(result_state IN ('unprocessed','stored','skipped')),
                   created_at INTEGER NOT NULL, carry_prompt_number INTEGER);
                 CREATE TABLE media_attachments(id TEXT PRIMARY KEY,
                   replay_key TEXT NOT NULL UNIQUE, provenance TEXT NOT NULL,
                   recipe TEXT NOT NULL CHECK(recipe IN ('screenshot-v1','photo-v1')),
                   state TEXT NOT NULL CHECK(state IN ('converting','ready','failed','deleted')),
                   failure_code TEXT, encoder_version TEXT, variants TEXT,
                   lease_until INTEGER NOT NULL DEFAULT 0,
                   upload_state TEXT NOT NULL DEFAULT 'pending'
                     CHECK(upload_state IN ('pending','uploaded','failed','cancelled')),
                   created_at INTEGER NOT NULL, origin TEXT NOT NULL DEFAULT 'native',
                   upload_attempts INTEGER NOT NULL DEFAULT 0,
                   upload_retry_at INTEGER NOT NULL DEFAULT 0, upload_error TEXT);
                 CREATE TABLE media_event_refs(event_key TEXT NOT NULL,
                   attachment_id TEXT NOT NULL, label TEXT NOT NULL,
                   PRIMARY KEY(event_key, attachment_id), UNIQUE(event_key, label),
                   FOREIGN KEY(event_key) REFERENCES media_events(event_key),
                   FOREIGN KEY(attachment_id) REFERENCES media_attachments(id));
                 CREATE TABLE observation_media_links(observation_id INTEGER NOT NULL,
                   attachment_id TEXT NOT NULL, event_key TEXT NOT NULL, label TEXT NOT NULL,
                   inspection TEXT NOT NULL DEFAULT 'uninspected'
                     CHECK(inspection IN ('inspected','uninspected')),
                   PRIMARY KEY(observation_id, attachment_id),
                   FOREIGN KEY(attachment_id) REFERENCES media_attachments(id));
                 CREATE TABLE media_cleanup_jobs(attachment_id TEXT PRIMARY KEY,
                   directory TEXT NOT NULL, queued_at INTEGER NOT NULL,
                   attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0,
                   last_error TEXT);
                 CREATE TABLE media_event_results(event_key TEXT NOT NULL,
                   observation_id INTEGER NOT NULL, PRIMARY KEY(event_key, observation_id));
                 CREATE TABLE media_cloud_deletions(attachment_id TEXT PRIMARY KEY,
                   queued_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
                   retry_at INTEGER NOT NULL DEFAULT 0, last_error TEXT);
                 CREATE TABLE committed_observation_tool_uses(platform_source TEXT NOT NULL,
                   content_session_id TEXT NOT NULL, tool_use_id TEXT NOT NULL,
                   committed_at_epoch_ms INTEGER NOT NULL,
                   PRIMARY KEY (platform_source, content_session_id, tool_use_id));
                 CREATE TABLE deleted_content_sessions(platform_source TEXT NOT NULL,
                   content_session_id TEXT NOT NULL, deleted_at_epoch_ms INTEGER NOT NULL,
                   PRIMARY KEY (platform_source, content_session_id));
                 CREATE TABLE deleted_memory_sessions(memory_session_id TEXT PRIMARY KEY,
                   deleted_at_epoch_ms INTEGER NOT NULL);
                 INSERT INTO tool_uses(tool_use_id, content_session_id, memory_session_id,
                   session_db_id, project, platform_source, tool_name, created_at,
                   created_at_epoch)
                   VALUES('tool-1', 'agent-1', 'mem-1', 1, 'free-mem', 'codex', 'Read', '', 1650);
                 INSERT INTO media_events VALUES('ev-1', 1, 'codex', 'tool-1', 'retained',
                   'stored', 1650, NULL);
                 INSERT INTO media_attachments(id, replay_key, provenance, recipe, state,
                   variants, created_at)
                   VALUES('att-1', 'replay-1', 'tool_result', 'screenshot-v1', 'ready', '{}', 1650);
                 INSERT INTO media_event_refs VALUES('ev-1', 'att-1', 'image 1');
                 INSERT INTO observation_media_links VALUES(10, 'att-1', 'ev-1', 'image 1',
                   'uninspected');
                 INSERT INTO media_event_results VALUES('ev-1', 10);
                 INSERT INTO committed_observation_tool_uses VALUES('codex', 'agent-1', 'tool-1',
                   1650);
                 INSERT INTO deleted_content_sessions VALUES('codex', 'agent-gone', 1900);
                 INSERT INTO deleted_memory_sessions VALUES('mem-gone', 1900);
                 CREATE TRIGGER media_observation_delete AFTER DELETE ON observations BEGIN
                   DELETE FROM observation_media_links WHERE observation_id = OLD.id;
                   DELETE FROM media_event_results WHERE observation_id = OLD.id;
                 END;
                 CREATE TRIGGER media_session_delete BEFORE DELETE ON sdk_sessions BEGIN
                   DELETE FROM media_event_refs WHERE event_key IN
                     (SELECT event_key FROM media_events WHERE session_db_id = OLD.id);
                   UPDATE media_events SET state = 'deleted' WHERE session_db_id = OLD.id;
                 END;
                 CREATE TRIGGER block_pre_deletion_sdk_session_insert
                   BEFORE INSERT ON sdk_sessions
                   WHEN EXISTS (SELECT 1 FROM deleted_content_sessions
                       WHERE platform_source = NEW.platform_source
                         AND content_session_id = NEW.content_session_id
                         AND NEW.started_at_epoch <= deleted_at_epoch_ms)
                     OR EXISTS (SELECT 1 FROM deleted_memory_sessions
                       WHERE memory_session_id = NEW.memory_session_id)
                   BEGIN SELECT RAISE(ABORT, 'session_deleted'); END;
                 CREATE TRIGGER block_deleted_sdk_session_rekey
                   BEFORE UPDATE OF memory_session_id, content_session_id, platform_source
                   ON sdk_sessions
                   WHEN EXISTS (SELECT 1 FROM deleted_memory_sessions
                       WHERE memory_session_id = NEW.memory_session_id)
                   BEGIN SELECT RAISE(ABORT, 'session_deleted'); END;
                 CREATE TRIGGER block_pre_deletion_tool_use_insert BEFORE INSERT ON tool_uses
                   WHEN EXISTS (SELECT 1 FROM deleted_content_sessions
                       WHERE platform_source = NEW.platform_source
                         AND content_session_id = NEW.content_session_id)
                     AND NOT EXISTS (SELECT 1 FROM sdk_sessions
                       WHERE id = NEW.session_db_id AND platform_source = NEW.platform_source
                         AND content_session_id = NEW.content_session_id)
                   BEGIN SELECT RAISE(ABORT, 'session_deleted'); END;
                 INSERT INTO schema_versions VALUES(7, 69, '');
                 INSERT INTO schema_versions VALUES(8, 70, '');
                 INSERT INTO schema_versions VALUES(9, 71, '');
                 INSERT INTO schema_versions VALUES(10, 73, '');
                 INSERT INTO schema_versions VALUES(11, 74, '');
                 INSERT INTO schema_versions VALUES(12, 72, '');",
        )
        .unwrap();
    let import = |src: &Path, home: &Path| {
        std::fs::create_dir(home).unwrap();
        let mut raw = crate::raw::open(home).unwrap();
        let stats = claude_mem(&mut raw, src, &Default::default()).unwrap();
        let docs: Vec<_> = docs(&raw)
            .into_iter()
            .map(|d| (d.source_id, d.repo, d.body))
            .collect();
        let files: Vec<_> = files_ops(&raw)
            .into_iter()
            .map(|f| (f.source_id, f.files_read, f.files_modified))
            .collect();
        (
            stats.observations,
            stats.summaries,
            stats.prompts,
            stats.work_state,
            docs,
            files,
        )
    };
    let (a, b, c, d, e) = (
        dir.path().join("a"),
        dir.path().join("b"),
        dir.path().join("c"),
        dir.path().join("d"),
        dir.path().join("e"),
    );
    let want = import(&v64, &a);
    assert_eq!(import(&v65, &b), want);
    assert_eq!(import(&v67, &c), want);
    assert_eq!(import(&v68, &d), want);
    assert_eq!(import(&v74, &e), want);
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
    raw.work_state(repo, "plan", done.as_object().unwrap(), None)
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

/// CodeRabbit on #431: work state rows go in batches within the size an append takes as well
/// as the count, so rows a rule makes long (every `x` a mask) still all come in.
#[test]
fn work_state_rows_a_rule_lengthens_come_in_within_the_batch_size() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_v64(&src, "2025-12-14T16:09:58.769Z");
    let c = Connection::open(&src).unwrap();
    let note = format!(r#"{{"note": "{}"}}"#, vec!["x"; 990].join(" "));
    for i in 0..500 {
        c.execute(
            "INSERT INTO work_state_entries(project, list_name, fields, created_at,
                   created_at_epoch) VALUES('free-mem', 'long', ?1, '', ?2)",
            params![note, 10_000 + i],
        )
        .unwrap();
    }
    drop(c);
    let rule = "[[redaction.extra_rules]]\nid = \"x\"\nregex = 'x'\n";
    std::fs::write(dir.path().join("config.toml"), rule).unwrap();
    let settings = crate::capture::Settings::load(dir.path()).unwrap();
    let mut raw = crate::raw::open(dir.path()).unwrap();
    let stats = claude_mem(&mut raw, &src, &settings).unwrap();
    assert_eq!((stats.work_state, stats.refused), (502, 2));
    let ops = work_ops(&raw);
    let bytes: usize = ops.iter().map(|o| o.to_string().len()).sum();
    assert!(bytes > crate::raw::MAX_BATCH_BYTES, "{bytes}");
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
    raw.work_state(repo, "plan", done.as_object().unwrap(), None)
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
    raw.work_state(
        &crafted,
        "plan",
        task("Written here").as_object().unwrap(),
        None,
    )
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

/// I8 (parity row 14): claude-mem's exclusion patterns, read from the settings beside its
/// database, come in once as patterns never recorded; one taken back is not added again, and
/// a database with no settings beside it imports all the same.
#[test]
fn claude_mems_exclusions_come_in_once_as_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    assert_eq!(
        claude_mem(&mut raw, &src, &Default::default())
            .unwrap()
            .exclusions,
        0
    );
    let long = "x".repeat(257);
    let settings = serde_json::json!({
        "CLAUDE_MEM_EXCLUDED_PROJECTS":
            format!("secret-a, ~/private/** ,, secret-b, secret-a, {long}"),
        "CLAUDE_MEM_MODEL": "unrelated",
    });
    std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    // One oboete does not take is counted, never dropped unsaid (the commit's security review).
    assert_eq!((stats.exclusions, stats.exclusions_refused), (3, 1));
    let patterns = raw.capture_exclusions().unwrap().patterns;
    assert_eq!(
        Vec::from_iter(patterns),
        ["secret-a", "secret-b", "~/private/**"]
    );
    raw.exclude_capture(("pattern", "secret-b"), true).unwrap();
    assert_eq!(
        claude_mem(&mut raw, &src, &Default::default())
            .unwrap()
            .exclusions,
        0
    );
    assert_eq!(raw.capture_exclusions().unwrap().patterns.len(), 2);
}

/// I8: claude-mem's settings are read as claude-mem reads them, a BOM skipped and the keys
/// under `env` when that holds them; a file that is there but cannot be read so stops the
/// import before any write (Codex on #563).
#[test]
fn claude_mems_settings_are_read_as_it_reads_them_or_stop_the_import() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("claude-mem.db");
    claude_mem_db(&src, "2025-12-14T16:09:58.769Z");
    let settings = dir.path().join("settings.json");
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    std::fs::create_dir(&settings).unwrap();
    let unreadable = claude_mem(&mut raw, &src, &Default::default());
    std::fs::remove_dir(&settings).unwrap();
    for (given, why) in [
        ("{\"CLAUDE_MEM_EXCLUDED_PROJECTS\": \"a\"", "is not JSON"),
        ("[\"a\"]", "is not a JSON object"),
        (
            "{\"CLAUDE_MEM_EXCLUDED_PROJECTS\": [\"a\"]}",
            "other than one string",
        ),
    ] {
        std::fs::write(&settings, given).unwrap();
        let Err(err) = claude_mem(&mut raw, &src, &Default::default()) else {
            panic!("{given} imported");
        };
        let err = err.to_string();
        assert!(
            err.contains(why) && err.contains("nothing is imported"),
            "{given}: {err}"
        );
    }
    assert!(
        unreadable
            .err()
            .is_some_and(|err| err.to_string().contains("cannot be read"))
    );
    let nested = serde_json::json!({
        "env": {"CLAUDE_MEM_EXCLUDED_PROJECTS": "nested-a", "CLAUDE_MEM_MODEL": "m"},
        "CLAUDE_MEM_EXCLUDED_PROJECTS": "stale-root",
        "theme": "dark",
    });
    std::fs::write(&settings, format!("\u{feff}{nested}")).unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    // The stopped runs wrote nothing: every row comes in now, none seen before.
    assert_eq!((stats.exclusions, stats.seen), (1, 0));
    assert!(stats.observations > 0);
    assert_eq!(
        Vec::from_iter(raw.capture_exclusions().unwrap().patterns),
        ["nested-a"]
    );
    // An `env` without claude-mem's keys leaves them at the top, as claude-mem reads them.
    let flat = r#"{"env": {"PATH": "/bin"}, "CLAUDE_MEM_EXCLUDED_PROJECTS": "flat-a"}"#;
    std::fs::write(&settings, flat).unwrap();
    let stats = claude_mem(&mut raw, &src, &Default::default()).unwrap();
    assert_eq!(stats.exclusions, 1);
    std::fs::write(&settings, r#"{"CLAUDE_MEM_EXCLUDED_PROJECTS": null}"#).unwrap();
    assert_eq!(
        claude_mem(&mut raw, &src, &Default::default())
            .unwrap()
            .exclusions,
        0
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

/// Decision 52 (docs/prompt-context.md, "Decision 52"): claude-mem 14.0.1's names by path.
#[test]
fn a_checkouts_names_are_claude_mems_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().canonicalize().unwrap();
    let mk = |p: &Path| std::fs::create_dir_all(p).unwrap();
    // A checkout and a folder in it: the top level's name.
    let main = d.join("payments");
    mk(&main.join(".git/worktrees/payments-fix"));
    mk(&main.join("src/deep"));
    assert_eq!(names_of(&main), ["payments"]);
    assert_eq!(names_of(&main.join("src/deep")), ["payments"]);
    // A linked worktree: the main repository's name, then `<main>/<worktree>`.
    let wt = d.join("payments-fix");
    mk(&wt.join("src"));
    let gitdir = |to: &Path| format!("gitdir: {}\n", to.display());
    std::fs::write(
        wt.join(".git"),
        gitdir(&main.join(".git/worktrees/payments-fix")),
    )
    .unwrap();
    assert_eq!(
        names_of(&wt.join("src")),
        ["payments", "payments/payments-fix"]
    );
    // One named as its main repository writes to it, the doubled key kept readable.
    let same = d.join("elsewhere/payments");
    mk(&same);
    std::fs::write(
        same.join(".git"),
        gitdir(&main.join(".git/worktrees/payments")),
    )
    .unwrap();
    assert_eq!(names_of(&same), ["payments/payments", "payments"]);
    // A submodule by a relative gitdir: the superproject's name, its own, `<super>/<path>`.
    let sub = main.join("vendor/lib");
    mk(&sub);
    std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/vendor/lib\n").unwrap();
    assert_eq!(names_of(&sub), ["payments", "lib", "payments/vendor/lib"]);
    // A `.git` file of more than its line, or naming neither, is a checkout of its own.
    std::fs::write(
        sub.join(".git"),
        "gitdir: ../../.git/modules/vendor/lib\nmore\n",
    )
    .unwrap();
    assert_eq!(names_of(&sub), ["lib"]);
    std::fs::write(wt.join(".git"), gitdir(&d.join("other/.git"))).unwrap();
    assert_eq!(names_of(&wt), ["payments-fix"]);
    // A bare repository's worktree (`<repo>.git/worktrees/<name>`) is not one to claude-mem.
    std::fs::write(
        wt.join(".git"),
        gitdir(&d.join("bare.git/worktrees/payments-fix")),
    )
    .unwrap();
    assert_eq!(names_of(&wt), ["payments-fix"]);
    // Outside git: the nearest marker root's name, the folder's own below it; no marker, the
    // folder's. A path is folded first, as `path.resolve` folds it.
    let site = d.join("site");
    mk(&site.join("pages/blog"));
    std::fs::write(site.join(".claude-mem.json"), "{}").unwrap();
    assert_eq!(names_of(&site), ["site"]);
    assert_eq!(names_of(&site.join("pages/blog")), ["blog", "site"]);
    assert_eq!(names_of(&site.join("pages/blog/..")), ["pages", "site"]);
    let loose = d.join("notes/today");
    mk(&loose);
    assert_eq!(names_of(&loose), ["today"]);
    // A link before `..` is folded away with it, as `path.resolve` folds it, not followed to
    // its target's parent (CodeRabbit on #567).
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&loose, site.join("pages/today")).unwrap();
        assert_eq!(names_of(&site.join("pages/today/..")), ["pages", "site"]);
    }
    // The walk stops before a stop (the home directory) and never starts in the config one.
    let home = d.join("home");
    mk(&home.join("work"));
    std::fs::write(home.join(".claude-mem-project"), "").unwrap();
    let config = [d.join("home/.claude")];
    let stops = [home.clone(), config[0].clone()];
    assert_eq!(marker_walk(&home.join("work"), &config, &stops), None);
    assert_eq!(
        marker_walk(&home.join("work"), &config, &[]),
        Some(home.clone())
    );
    assert_eq!(marker_walk(&home.join(".claude/x"), &config, &[]), None);
    #[cfg(unix)]
    assert_eq!(names_of(Path::new("/")), [UNKNOWN_PROJECT]);
}
