use super::*;
use crate::capture::{self, Settings};
use crate::raw::{self, Event, Item};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::PathBuf;

const CODEX: &str = "src/testdata/transcripts/codex-basic.jsonl";
const CODEX_SESSION: &str = "22222222-2222-4222-8222-222222222222";
const CLAUDE_MS: i64 = 1_788_220_800_000;
const CODEX_MS: i64 = 1_788_307_200_000;

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(path, target).unwrap();
        }
    }
}

fn fixtures(dir: &Path) -> (PathBuf, PathBuf) {
    let claude = dir.join("claude/projects");
    let project = claude.join("-work-app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::copy(CLAUDE, project.join("claude-basic.jsonl")).unwrap();
    copy_tree(
        Path::new("src/testdata/transcripts/claude-basic"),
        &project.join("claude-basic"),
    );
    let codex = dir.join("codex/sessions");
    let day = codex.join("2026/09/02");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::copy(CODEX, day.join("rollout-basic.jsonl")).unwrap();
    // Neither a workflow journal nor an unrelated JSONL file is a rollout.
    std::fs::write(codex.join("journal.jsonl"), b"not a transcript\n").unwrap();
    (claude, codex)
}

fn records(home: &Path) -> Vec<Event> {
    let raw = raw::open(home).unwrap();
    let mut out = Vec::new();
    let mut after = 0;
    loop {
        let page = raw.after(raw.device(), after, 1_000).unwrap();
        let Some(last) = page.last() else {
            return out;
        };
        after = last.seq;
        out.extend(page.into_iter().filter_map(|r| match r.item {
            Item::Event(e) => Some(*e),
            _ => None,
        }));
    }
}

fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn read(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = if path.is_dir() {
                read(base, &path, out);
                None
            } else {
                Some(std::fs::read(&path).unwrap())
            };
            out.insert(path.strip_prefix(base).unwrap().to_owned(), bytes);
        }
    }
    let mut out = BTreeMap::new();
    read(dir, dir, &mut out);
    out
}

/// #541: what the import stores is what its capture masked, and the rescan the worker runs
/// after it finds nothing new: a numbered `.env` listing as a Bash result (a plain value after
/// `*_SECRET=`, and escaped line breaks that would join a URL to a later `@`), and a read of
/// code that builds a PEM.
#[test]
fn an_import_leaves_the_rescan_nothing_to_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("claude/projects");
    std::fs::create_dir_all(root.join("-w")).unwrap();
    // split: scanners
    let low = format!("{}{}", "a3a3b1b1c2c2a3a3", "b1b1c2c2a3a3b1b1");
    let high = format!("{}{}", "e4b1c9a27f3d8b05", "a6e2c7d94f1b3a58");
    let listing = format!(
        "10:ACCESS_TEAM_DOMAIN=https://team.example.test\n11:ACCESS_AUD={high}{high}\n\
             15:BOT_CHANNEL_ID=1234567890\n17:BOT_CHANNEL_SECRET={low}\n\
             18:BOT_ACCOUNT_ID=@123abcde"
    );
    let pem = "  return `-----BEGIN PRIVATE KEY-----\\n${b64.match(/.{1,64}/g)!.join(\"\\n\")}\\n-----END PRIVATE KEY-----`;";
    let lines: Vec<String> = [
            json!({"type": "user", "message": {"role": "user", "content": "look"}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "grep -n = .env"}}]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": listing}]}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/w/a.test.ts"}}]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t2", "content": pem}]},
                "toolUseResult": {"type": "text", "file": {"filePath": "/w/a.test.ts", "content": pem}}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "text", "text": "done"}]}}),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, mut v)| {
            v["sessionId"] = json!("s541");
            v["cwd"] = json!("/w");
            v["timestamp"] = json!(format!("2026-09-01T00:00:{i:02}.000Z"));
            v.to_string()
        })
        .collect();
    std::fs::write(root.join("-w/s541.jsonl"), lines.join("\n") + "\n").unwrap();
    let home = dir.path().join("home");
    import(&home, &[("claude", root.as_path())], true, &mut Vec::new()).unwrap();
    let stored = records(&home);
    assert_eq!(stored.iter().filter(|e| e.kind == "tool").count(), 2);
    for e in &stored {
        assert!(
            !e.body.contains(&low) && !e.body.contains(&high),
            "{}",
            e.body
        );
    }
    crate::worker::run_once(&home).unwrap();
    let tombstones: i64 = rusqlite::Connection::open(home.join("raw.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE type = 'tombstone' AND target_offset IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tombstones, 0);
}

#[test]
fn only_lines_before_the_sessions_first_raw_record_are_imported() {
    for (agent, session, cut, total) in [
        ("claude", "claude-basic", CLAUDE_MS + 4_000, 18),
        ("codex", CODEX_SESSION, CODEX_MS + 8_000, 8),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (claude, codex) = fixtures(dir.path());
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mut raw = raw::open(&home).unwrap();
        let live = capture::events(
            agent,
            "UserPromptSubmit",
            &json!({"session_id": session, "cwd": "/work", "prompt": "Already captured"}),
            cut,
            &Settings::default(),
        );
        raw.append_with_ledger(&live[0].event, &live[0].ledger, "test")
            .unwrap();
        drop(raw);
        let roots = [("claude", claude.as_path()), ("codex", codex.as_path())];
        let first = import(&home, &roots, true, &mut Vec::new()).unwrap();
        assert_eq!(first.agents[agent].events, 4);
        assert_eq!(first.agents[agent].cut, total - 4);
        let other = if agent == "claude" { "codex" } else { "claude" };
        assert_eq!(
            first.agents[other].events,
            if other == "claude" { 18 } else { 8 }
        );
        let got = records(&home);
        let imported: Vec<_> = got.iter().filter(|e| e.source == "transcript").collect();
        assert!(
            imported
                .iter()
                .filter(|e| e.agent == agent)
                .all(|e| e.ts < cut)
        );
        assert_eq!(got.iter().filter(|e| e.source == "hook").count(), 1);
        assert!(
            imported
                .iter()
                .all(|e| e.branch.is_none() && e.head.is_none())
        );
        let again = import(&home, &roots, true, &mut Vec::new()).unwrap();
        assert!(again.agents.values().all(|s| s.events == 0));
        assert_eq!(again.agents[agent].seen, total);
        assert_eq!(records(&home), got);
        let raw = raw::open(&home).unwrap();
        let checkpoints = raw.migration_checkpoints("transcript:").unwrap();
        assert_eq!(
            checkpoints[&format!("transcript:{agent}:{session}")].through,
            total as i64
        );
    }
}

#[test]
fn a_trimmed_v1_head_comes_back() {
    for (agent, fixture, session, first_prompt, boundary_prompt) in [
        (
            "claude",
            CLAUDE,
            "claude-basic",
            "キャッシュの方針を決めたい",
            "どちらが良い？",
        ),
        (
            "codex",
            CODEX,
            CODEX_SESSION,
            "Add a 50ms timeout to fetchJson",
            "Try again with 100ms",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (claude, codex) = fixtures(dir.path());
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("oboete.db");
        let v1 = Connection::open(&path).unwrap();
        v1.pragma_update(None, "journal_mode", "WAL").unwrap();
        v1.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL,
                   cwd TEXT, started_at INTEGER NOT NULL, last_event_at INTEGER NOT NULL);
                 CREATE TABLE session_repos(session_id TEXT NOT NULL, repo TEXT NOT NULL,
                   PRIMARY KEY(session_id, repo)) WITHOUT ROWID;
                 CREATE TABLE events(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
                   event TEXT NOT NULL, ts INTEGER NOT NULL, payload TEXT NOT NULL);
                 CREATE TABLE observations(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT,
                   ts INTEGER, kind TEXT, title TEXT, body TEXT, uid TEXT);
                 CREATE TABLE summaries(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT,
                   ts INTEGER, body TEXT, uid TEXT);
                 CREATE TABLE prompts(id INTEGER PRIMARY KEY, session_id TEXT, repo TEXT,
                   ts INTEGER, body TEXT, uid TEXT);
                 INSERT INTO meta VALUES('device_id', 'd1e5');",
        )
        .unwrap();
        v1.execute(
            "INSERT INTO sessions VALUES(?1, ?2, '/work', '/work', 0, 0)",
            params![session, agent],
        )
        .unwrap();
        v1.execute("INSERT INTO session_repos VALUES(?1, '/work')", [session])
            .unwrap();
        let (lines, _) = events(fixture, agent);
        let mid = lines
            .iter()
            .position(|e| e["payload"]["prompt"] == boundary_prompt)
            .unwrap();
        let clock = Connection::open_in_memory().unwrap();
        let mut first_ts = 0;
        for (i, line) in lines[mid..].iter().enumerate() {
            let ts: i64 = clock
                .query_row(
                    "SELECT CAST(round(unixepoch(?1, 'subsec') * 1000) AS INTEGER)",
                    [line["ts"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            if i == 0 {
                first_ts = ts + 1;
            }
            let captured = capture::imported(
                agent,
                line["event"].as_str().unwrap(),
                &line["payload"],
                ts,
                "/work",
                Some("/work"),
                &Settings::default(),
            );
            let body = &captured[0].event.body;
            // The hook stamps a turn later than its transcript entry does (spec 7.4).
            v1.execute(
                "INSERT INTO events(session_id, event, ts, payload) VALUES(?1, ?2, ?3, ?4)",
                params![session, line["event"].as_str().unwrap(), ts + 1, body],
            )
            .unwrap();
        }
        drop(v1);
        let before = std::fs::read(&path).unwrap();
        let root = if agent == "claude" { &claude } else { &codex };
        let before_preview = snapshot(&home);
        let preview = import(&home, &[(agent, root)], false, &mut Vec::new()).unwrap();
        assert_eq!(
            preview.agents[agent].events,
            if agent == "claude" { 6 } else { 8 }
        );
        assert!(
            snapshot(&home) == before_preview,
            "preview changed v1's files"
        );
        let mut out = Vec::new();
        let stats = import(&home, &[(agent, root)], true, &mut out).unwrap();
        assert!(stats.agents[agent].events > 2);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("[summary] curate is not set")
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let got = records(&home);
        assert!(
            got.iter()
                .any(|e| e.source == "oboete-v1" && e.kind == "prompt")
        );
        assert!(
            got.iter()
                .filter(|e| e.source == "transcript")
                .all(|e| e.ts < first_ts)
        );
        let mut prompts = BTreeMap::<String, Vec<String>>::new();
        for e in got.iter().filter(|e| e.kind == "prompt") {
            let body: Value = serde_json::from_str(&e.body).unwrap();
            prompts
                .entry(body["prompt"].as_str().unwrap().to_owned())
                .or_default()
                .push(e.source.clone());
        }
        assert_eq!(prompts[first_prompt], ["transcript"]);
        let doubled: Vec<_> = prompts
            .iter()
            .filter(|(_, sources)| sources.len() > 1)
            .collect();
        assert_eq!(doubled.len(), 1);
        assert_eq!(doubled[0].0, boundary_prompt);
        assert_eq!(doubled[0].1, &["oboete-v1", "transcript"]);
    }
}

#[test]
fn nothing_is_written_without_yes() {
    let dir = tempfile::tempdir().unwrap();
    let (claude, codex) = fixtures(dir.path());
    let home = dir.path().join("missing");
    let roots = [("claude", claude.as_path()), ("codex", codex.as_path())];
    for existing in [false, true] {
        if existing {
            std::fs::create_dir_all(&home).unwrap();
            drop(raw::open(&home).unwrap());
        }
        let before = existing.then(|| snapshot(&home));
        let mut out = Vec::new();
        let stats = import(&home, &roots, false, &mut out).unwrap();
        assert_eq!(
            (
                stats.agents["claude"].files,
                stats.agents["claude"].sessions,
                stats.agents["claude"].events
            ),
            (1, 1, 18)
        );
        assert_eq!(
            (
                stats.agents["codex"].files,
                stats.agents["codex"].sessions,
                stats.agents["codex"].events
            ),
            (1, 1, 8)
        );
        assert!(stats.agents.values().all(|s| s.bytes > 0));
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("--yes"));
        let printed: Value = serde_json::from_str(said.lines().last().unwrap()).unwrap();
        assert_eq!(printed, serde_json::to_value(stats).unwrap());
        if let Some(before) = before {
            assert_eq!(snapshot(&home), before);
        } else {
            assert!(!home.exists());
        }
    }
}

#[test]
fn a_changed_file_waits() {
    for which in ["codex", "claude", "subagent"] {
        let dir = tempfile::tempdir().unwrap();
        let (claude, codex) = fixtures(dir.path());
        let agent = if which == "codex" { "codex" } else { "claude" };
        let root = if agent == "codex" { &codex } else { &claude };
        let changed = match which {
            "codex" => codex.join("2026/09/02/rollout-basic.jsonl"),
            "claude" => claude.join("-work-app/claude-basic.jsonl"),
            _ => claude.join("-work-app/claude-basic/subagents/workflows/wf_1/agent-w1.jsonl"),
        };
        AFTER_PARSE.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(changed)
                    .unwrap()
                    .write_all(b"\n")
                    .unwrap();
            }))
        });
        let home = dir.path().join("home");
        let first = import(&home, &[(agent, root)], true, &mut Vec::new()).unwrap();
        assert_eq!(
            (first.agents[agent].events, first.agents[agent].waiting),
            (0, 1)
        );
        assert!(records(&home).is_empty());
        assert!(
            raw::open(&home)
                .unwrap()
                .migration_checkpoints("transcript:")
                .unwrap()
                .is_empty()
        );
        let again = import(&home, &[(agent, root)], true, &mut Vec::new()).unwrap();
        assert_eq!(
            again.agents[agent].events,
            if agent == "claude" { 18 } else { 8 }
        );
        assert_eq!(again.agents[agent].waiting, 0);
    }
}

#[test]
fn composite_preview_reports_waiting_without_authorizing_a_later_file_version() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let changed = root.join("2026/09/02/rollout-basic.jsonl");
    AFTER_PARSE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            std::fs::OpenOptions::new()
                .append(true)
                .open(changed)
                .unwrap()
                .write_all(b"\n")
                .unwrap();
        }))
    });
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let codex = &shown.preview.candidates.agents["codex"];
    assert_eq!((codex.events, codex.waiting), (0, 1));
    assert!(!home.exists());
    // Its confirm leaves the file for a later import: none of the later version is stored.
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// A transcript a session at work wrote to after the preview bound it, before the preview
/// came to it: counted as waiting, as one that changes while it is read. The preview is shown,
/// and a confirm of it leaves the file for a later import: it was shown none of that version.
#[test]
fn a_preview_counts_a_transcript_that_grew_before_its_turn_as_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let changed = root.join("2026/09/02/rollout-basic.jsonl");
    AFTER_BOUND.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            std::fs::OpenOptions::new()
                .append(true)
                .open(changed)
                .unwrap()
                .write_all(b"\n")
                .unwrap();
        }))
    });
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let codex = &shown.preview.candidates.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(!home.exists());
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// The same for a file another took the place of while it was read, of the same size and
/// time: its lines read as stable, and it is not the file the preview bound. A confirm
/// stores nothing of the one that took its place.
#[test]
fn a_preview_counts_a_transcript_replaced_while_it_was_read_as_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let replaced = root.join("2026/09/02/rollout-basic.jsonl");
    AFTER_PARSE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            let copy = replaced.with_extension("copy");
            std::fs::copy(&replaced, &copy).unwrap();
            let written = std::fs::metadata(&replaced).unwrap().modified().unwrap();
            let file = std::fs::File::options().write(true).open(&copy).unwrap();
            file.set_modified(written).unwrap();
            drop(file);
            std::fs::rename(&copy, &replaced).unwrap();
        }))
    });
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let codex = &shown.preview.candidates.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(!home.exists());
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// A Claude Code transcript of `prompts` prompts, one session, under `dir`: its root and file.
fn prompts_transcript(dir: &Path, prompts: usize) -> (PathBuf, PathBuf) {
    let root = dir.join("claude/projects");
    let project = root.join("-work-app");
    std::fs::create_dir_all(&project).unwrap();
    let lines: String = (0..prompts)
            .map(|i| {
                let at = format!("2026-09-01T{:02}:{:02}:{:02}Z", i / 3600, i / 60 % 60, i % 60);
                let line = json!({"type": "user", "timestamp": at, "sessionId": "grows",
                    "cwd": "/work/app", "message": {"role": "user", "content": format!("Prompt {i}.")}});
                format!("{line}\n")
            })
            .collect();
    let file = project.join("grows.jsonl");
    std::fs::write(&file, lines).unwrap();
    (root, file)
}

fn grow(file: &Path) {
    let mut file = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    file.write_all(b"\n").unwrap();
}

/// A transcript a session at work wrote to after the preview read it: the preview has its
/// lines, read from the file it bound, and writes nothing, so it reads on and is shown with
/// the file's events counted. A confirm of it leaves the file for a later import: it is not
/// the one it was shown. With one batch and with two.
#[test]
fn a_preview_reads_on_when_a_transcript_grows_after_it_was_read() {
    for prompts in [3, raw::IMPORT_BATCH + 1] {
        let dir = tempfile::tempdir().unwrap();
        let (root, file) = prompts_transcript(dir.path(), prompts);
        let grown = file.clone();
        AFTER_READ.with_borrow_mut(|hook| *hook = Some(Box::new(move || grow(&grown))));
        let home = dir.path().join("home");
        let roots = [("claude", root.as_path())];
        let shown = preview_plan(&home, &roots).unwrap();
        let claude = &shown.preview.candidates.agents["claude"];
        assert_eq!((claude.files, claude.waiting), (1, 0), "{prompts}");
        assert!(claude.events >= prompts as u64, "{prompts}: {claude:?}");
        assert!(!home.exists());
        let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
        let claude = &done.stats.agents["claude"];
        assert_eq!(
            (claude.files, claude.events, claude.waiting),
            (1, 0, 1),
            "{prompts}"
        );
        assert!(records(&home).is_empty(), "{prompts}");
    }
}

/// A confirm leaves a file that grows after it was read, before a batch of it is written,
/// for a later import: none of the file is stored. With one batch (the check at the file's
/// end) and with two (the check before a batch).
#[test]
fn a_confirm_leaves_a_transcript_that_grows_after_it_was_read_for_later() {
    for prompts in [3, raw::IMPORT_BATCH + 1] {
        let dir = tempfile::tempdir().unwrap();
        let (root, file) = prompts_transcript(dir.path(), prompts);
        let home = dir.path().join("home");
        let roots = [("claude", root.as_path())];
        let shown = preview_plan(&home, &roots).unwrap();
        // The confirm reads the file once, to import it: it grows after that reading.
        AFTER_READ.with_borrow_mut(|hook| *hook = Some(Box::new(move || grow(&file))));
        let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
        let claude = &done.stats.agents["claude"];
        assert_eq!(
            (claude.files, claude.events, claude.waiting),
            (1, 0, 1),
            "{prompts}"
        );
        assert!(records(&home).is_empty(), "{prompts}");
    }
}

/// A file that grows after a confirm stored a batch of it keeps that batch, read from the
/// file that was bound, and the rest waits: the next import brings the rest, and nothing
/// twice.
#[test]
fn a_confirm_keeps_the_batch_it_stored_before_a_transcript_grew() {
    let dir = tempfile::tempdir().unwrap();
    let (root, file) = prompts_transcript(dir.path(), raw::IMPORT_BATCH + 1);
    let roots = [("claude", root.as_path())];
    let bodies = |home: &Path| {
        let mut bodies: Vec<String> = records(home).into_iter().map(|e| e.body).collect();
        bodies.sort();
        bodies
    };
    // What one import of the whole file stores, in another home.
    let whole = dir.path().join("whole");
    import(&whole, &roots, true, &mut Vec::new()).unwrap();
    let whole = bodies(&whole);
    let home = dir.path().join("home");
    let shown = preview_plan(&home, &roots).unwrap();
    let mut grown = false;
    let done = run(&home, &roots, Some(shown), &mut |_| {
        // The first batch is stored: the session writes on.
        if !std::mem::replace(&mut grown, true) {
            grow(&file);
        }
    })
    .unwrap();
    let claude = &done.stats.agents["claude"];
    assert_eq!((claude.files, claude.waiting), (1, 1));
    let first = records(&home).len();
    assert!(
        0 < first && first < whole.len(),
        "{first} of {}",
        whole.len()
    );
    import(&home, &roots, true, &mut Vec::new()).unwrap();
    assert_eq!(bodies(&home), whole);
}

/// A line whose transcript named no directory gets no repository: "." is the importing
/// process's directory, never the session's.
#[test]
fn a_line_with_no_directory_gets_no_repository() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("claude/projects/-work-app");
    std::fs::create_dir_all(&project).unwrap();
    let line = json!({"type": "user", "timestamp": "2026-09-01T00:00:01Z",
                          "sessionId": "no-dir", "message": {"role": "user",
                          "content": "Keep the parser errors on stderr."}});
    std::fs::write(project.join("no-dir.jsonl"), format!("{line}\n")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let claude = dir.path().join("claude/projects");
    import(&home, &[("claude", &claude)], true, &mut Vec::new()).unwrap();
    let got = records(&home);
    assert!(got.iter().any(|e| e.kind == "prompt"), "{got:?}");
    let here = crate::repo::key(&std::env::current_dir().unwrap());
    for e in got {
        assert_ne!(e.repo.as_deref(), Some(here.as_str()), "{e:?}");
    }
}

/// Slice 4a (Codex on #489): a folder excluded before a rule that masks part of its path is
/// still never recorded by an import: the import matches the list as its labels are made now.
#[test]
fn a_rule_added_after_a_folder_was_excluded_keeps_its_transcript_out() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("claude/projects/-x");
    std::fs::create_dir_all(&project).unwrap();
    for (session, cwd, said) in [
        ("s", "/x/acme-482915/app", "canary-4a"),
        ("t", "/x/open", "Open work."),
    ] {
        let line = json!({"type": "user", "timestamp": "2026-09-01T00:00:01Z",
                              "sessionId": session, "cwd": cwd,
                              "message": {"role": "user", "content": said}});
        std::fs::write(
            project.join(format!("{session}.jsonl")),
            format!("{line}\n"),
        )
        .unwrap();
    }
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    raw::open(&home)
        .unwrap()
        .exclude_capture(("folder", "/x/acme-482915"), false)
        .unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let claude = dir.path().join("claude/projects");
    import(&home, &[("claude", &claude)], true, &mut Vec::new()).unwrap();
    let got = records(&home);
    assert!(got.iter().any(|e| e.body.contains("Open work.")), "{got:?}");
    assert!(got.iter().all(|e| e.session != "s"), "{got:?}");
}

/// The parser's own SessionEnd is not imported: a session resumed after an import gets its
/// next events in by a rerun.
#[test]
fn a_resumed_session_imports_its_next_events() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("claude/projects/-work-app");
    std::fs::create_dir_all(&project).unwrap();
    let prompt = |ts: &str, text: &str| {
        let line = json!({"type": "user", "timestamp": ts, "sessionId": "resumed",
                              "cwd": "/work/app", "message": {"role": "user", "content": text}});
        format!("{line}\n")
    };
    let file = project.join("resumed.jsonl");
    std::fs::write(&file, prompt("2026-09-01T00:00:01Z", "first")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let claude = dir.path().join("claude/projects");
    import(&home, &[("claude", &claude)], true, &mut Vec::new()).unwrap();
    let mut resumed = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    std::io::Write::write_all(
        &mut resumed,
        prompt("2026-09-02T00:00:01Z", "second").as_bytes(),
    )
    .unwrap();
    import(&home, &[("claude", &claude)], true, &mut Vec::new()).unwrap();
    let got = records(&home);
    let prompts: Vec<&str> = got
        .iter()
        .filter(|e| e.kind == "prompt")
        .map(|e| e.body.as_str())
        .collect();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts[0].contains("first") && prompts[1].contains("second"));
    assert!(got.iter().all(|e| e.kind != "end"), "{got:?}");
}

#[test]
fn housekeeping_sessions_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (_, codex) = fixtures(dir.path());
    let path = codex.join("2026/09/02/rollout-basic.jsonl");
    let cwd = crate::config::home_dir().join(".codex/memories/consolidate");
    let text: String = std::fs::read(&path)
        .unwrap()
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut line: Value = serde_json::from_slice(line).unwrap();
            if line["payload"].get("cwd").is_some() {
                line["payload"]["cwd"] = json!(cwd);
            }
            format!("{line}\n")
        })
        .collect();
    std::fs::write(path, text).unwrap();
    let home = dir.path().join("home");
    let stats = import(&home, &[("codex", &codex)], true, &mut Vec::new()).unwrap();
    assert_eq!(
        (
            stats.agents["codex"].sessions,
            stats.agents["codex"].housekeeping,
            stats.agents["codex"].events
        ),
        (1, 1, 0)
    );
    assert!(records(&home).is_empty());
}

#[test]
fn appended_tool_results_are_imported_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("rollout-resumed.jsonl");
    let fixture = std::fs::read_to_string(CODEX).unwrap();
    let unfinished = fixture.lines().take(5).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&file, &unfinished).unwrap();
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let first = import(&home, &roots, true, &mut Vec::new()).unwrap();
    assert_eq!(first.agents["codex"].events, 2);
    std::fs::write(&file, unfinished + fixture.lines().nth(5).unwrap() + "\n").unwrap();
    import(&home, &roots, true, &mut Vec::new()).unwrap();
    let got = records(&home);
    let tools: Vec<_> = got.iter().filter(|e| e.kind == "tool").collect();
    assert_eq!(tools.len(), 1, "{tools:?}");
    let body: Value = serde_json::from_str(&tools[0].body).unwrap();
    assert_eq!(body["output"], "src/http.ts:3");
    assert!(body.get("interrupted").is_none());
    assert_eq!(
        import(&home, &roots, true, &mut Vec::new()).unwrap().agents["codex"].events,
        0
    );
    assert_eq!(records(&home), got);
}

/// #692: an imported call's record is at its result's time, and keeps its call's time as its
/// start, a second before in the fixture.
#[test]
fn an_imported_call_keeps_its_calls_time_as_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    std::fs::create_dir(&root).unwrap();
    let fixture = std::fs::read_to_string(CODEX).unwrap();
    let first_call = fixture.lines().take(6).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(root.join("rollout-call.jsonl"), first_call).unwrap();
    let home = dir.path().join("home");
    import(&home, &[("codex", root.as_path())], true, &mut Vec::new()).unwrap();
    let got = records(&home);
    let tool = got.iter().find(|e| e.kind == "tool").unwrap();
    let body: Value = serde_json::from_str(&tool.body).unwrap();
    assert_eq!(body["started"], tool.ts - 1_000, "{body}");
}

#[test]
fn late_subagent_events_refuse_a_changed_imported_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects/p");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("claude-basic.jsonl");
    std::fs::copy(CLAUDE, &file).unwrap();
    let home = dir.path().join("home");
    let roots = [("claude", root.parent().unwrap())];
    let first = import(&home, &roots, true, &mut Vec::new()).unwrap();
    assert!(first.agents["claude"].events > 0);
    let before = records(&home);
    let subagents = file.with_extension("").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::copy(
        "src/testdata/transcripts/claude-basic/subagents/agent-a1.jsonl",
        subagents.join("agent-a1.jsonl"),
    )
    .unwrap();
    let mut out = Vec::new();
    let refused = import(&home, &roots, true, &mut out).unwrap_err();
    assert!(
        format!("{refused:#}").contains("imported prefix"),
        "{refused:#}"
    );
    let said = String::from_utf8(out).unwrap();
    assert!(
        said.contains(file.file_name().unwrap().to_str().unwrap()),
        "{said}"
    );
    let report: Value = serde_json::from_str(said.lines().last().unwrap()).unwrap();
    assert_eq!(report["agents"]["claude"]["refused"], 1);
    assert_eq!(records(&home), before);
}

fn appended(file: &Path, lines: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    file.write_all(lines.as_bytes()).unwrap();
}

/// One agent's import of the files under `root`, with what the run said when it fails.
fn imports(home: &Path, agent: &str, root: &Path) -> AgentStats {
    let mut out = Vec::new();
    let done = import(home, &[(agent, root)], true, &mut out);
    let said = String::from_utf8(out).unwrap();
    let done = done.unwrap_or_else(|e| panic!("{e:#}\n{said}"));
    done.agents[agent].clone()
}

/// A Claude Code transcript's record at `second` of 2026-09-01T00:00, or with no time.
fn claude_wrote(kind: &str, second: Option<u32>, content: Value) -> String {
    let mut v = json!({"type": kind, "sessionId": "pending", "cwd": "/work/app",
            "message": {"role": kind, "content": content}});
    if let Some(second) = second {
        v["timestamp"] = json!(format!("2026-09-01T00:00:{second:02}.000Z"));
    }
    format!("{v}\n")
}

/// A turn's two calls, made in one record: `slow` gets no result.
fn two_calls(second: Option<u32>) -> String {
    let calls = json!([
            {"type": "tool_use", "id": "slow", "name": "Bash", "input": {"command": "sleep 60"}},
            {"type": "tool_use", "id": "fast", "name": "Bash", "input": {"command": "true"}}]);
    claude_wrote("assistant", second, calls)
}

fn fast_result(second: u32) -> String {
    let result = json!([{"type": "tool_result", "tool_use_id": "fast", "content": "done"}]);
    claude_wrote("user", Some(second), result)
}

const STOPPED: &str = "[Request interrupted by user]";

/// The one record the second run stored: the call that got no result, as interrupted, at
/// `at`, the time the call was made.
fn the_interrupted_call(before: &[Event], after: &[Event], at: i64) {
    assert_eq!(after[..before.len()], before[..]);
    let [slow] = &after[before.len()..] else {
        panic!("{:#?}", &after[before.len()..]);
    };
    assert_eq!((slow.kind.as_str(), slow.ts), ("tool", at), "{slow:#?}");
    assert!(slow.body.contains("sleep 60"), "{}", slow.body);
    let body: Value = serde_json::from_str(&slow.body).unwrap();
    assert_eq!(body["interrupted"], true, "{body}");
}

/// #950: a call with no result yet when an import ran (a call made beside it had its result
/// by then), and interrupted after it. Its line has the time the call was made, and stands
/// where the record that stopped the turn is: the next import takes it after the lines
/// imported before, and the file is not refused.
#[test]
fn a_call_interrupted_after_an_import_comes_after_the_imported_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (home, root) = (dir.path().join("home"), dir.path().join("projects"));
    std::fs::create_dir_all(root.join("p")).unwrap();
    let file = root.join("p/pending.jsonl");
    let turn =
        claude_wrote("user", Some(0), json!("Run both.")) + &two_calls(Some(1)) + &fast_result(2);
    std::fs::write(&file, turn).unwrap();
    assert!(imports(&home, "claude", &root).events > 0);
    let imported = records(&home);
    // The developer stops the turn: the call with no result ends there.
    appended(&file, &claude_wrote("user", Some(9), json!(STOPPED)));
    assert_eq!(imports(&home, "claude", &root).events, 1);
    let after = records(&home);
    the_interrupted_call(&imported, &after, CLAUDE_MS + 1_000);
    assert_eq!(imports(&home, "claude", &root).events, 0);
    assert_eq!(records(&home), after);
}

/// The same for a Codex rollout, whose `turn_aborted` ends the calls of its turn.
#[test]
fn a_codex_call_aborted_after_an_import_comes_after_the_imported_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (home, root) = (dir.path().join("home"), dir.path().join("sessions"));
    std::fs::create_dir(&root).unwrap();
    let line = |second: u32, kind: &str, payload: Value| {
        let at = format!("2026-09-02T00:00:{second:02}.000Z");
        format!(
            "{}\n",
            json!({"timestamp": at, "type": kind, "payload": payload})
        )
    };
    let call = |id: &str, cmd: &str| {
        let arguments = json!({"cmd": cmd}).to_string();
        let call = json!({"type": "function_call", "name": "exec_command",
                "arguments": arguments, "call_id": id});
        line(2, "response_item", call)
    };
    let meta = json!({"id": CODEX_SESSION, "cwd": "/work/svc", "cli_version": "0.200.0"});
    let prompt = json!({"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "Run both."}]});
    let result = json!({"type": "function_call_output", "call_id": "fast", "output": "done"});
    let file = root.join("rollout-pending.jsonl");
    let turn = [
        line(0, "session_meta", meta),
        line(1, "response_item", prompt),
        call("slow", "sleep 60"),
        call("fast", "true"),
        line(3, "response_item", result),
    ];
    std::fs::write(&file, turn.concat()).unwrap();
    assert!(imports(&home, "codex", &root).events > 0);
    let imported = records(&home);
    let aborted = json!({"type": "turn_aborted", "reason": "interrupted"});
    appended(&file, &line(9, "event_msg", aborted));
    assert_eq!(imports(&home, "codex", &root).events, 1);
    let after = records(&home);
    the_interrupted_call(&imported, &after, CODEX_MS + 2_000);
    assert_eq!(imports(&home, "codex", &root).events, 0);
    assert_eq!(records(&home), after);
}

/// The import's time cut reads the time a call was made (#950): a call made before the
/// session's first record from a hook and interrupted after it is imported, with the lines
/// before that record.
#[test]
fn a_call_made_before_the_cut_and_interrupted_after_it_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    let (home, root) = (dir.path().join("home"), dir.path().join("projects"));
    std::fs::create_dir_all(root.join("p")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let mut raw = raw::open(&home).unwrap();
    let live = capture::events(
        "claude",
        "UserPromptSubmit",
        &json!({"session_id": "pending", "cwd": "/work/app", "prompt": "Already captured"}),
        CLAUDE_MS + 5_000,
        &Settings::default(),
    );
    raw.append_with_ledger(&live[0].event, &live[0].ledger, "test")
        .unwrap();
    drop(raw);
    let turn = claude_wrote("user", Some(0), json!("Run both."))
        + &two_calls(Some(1))
        + &fast_result(2)
        + &claude_wrote("user", Some(9), json!(STOPPED))
        + &claude_wrote("user", Some(12), json!("Go on."));
    std::fs::write(root.join("p/pending.jsonl"), turn).unwrap();
    let stats = imports(&home, "claude", &root);
    let got = records(&home);
    let imported: Vec<&Event> = got.iter().filter(|e| e.source == "transcript").collect();
    assert!(
        imported.iter().all(|e| e.ts < CLAUDE_MS + 5_000),
        "{imported:#?}"
    );
    let slow: Vec<&&Event> = (imported.iter())
        .filter(|e| e.kind == "tool" && e.body.contains("sleep 60"))
        .collect();
    assert_eq!(slow.len(), 1, "{imported:#?}");
    assert_eq!(slow[0].ts, CLAUDE_MS + 1_000);
    // The prompt after the interruption is behind the cut.
    assert_eq!(stats.cut, 1, "{stats:?}");
}

#[test]
fn rewritten_or_truncated_imported_prefixes_are_refused() {
    for truncate in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (_, root) = fixtures(dir.path());
        let file = root.join("2026/09/02/rollout-basic.jsonl");
        let home = dir.path().join("home");
        let roots = [("codex", root.as_path())];
        assert_eq!(
            import(&home, &roots, true, &mut Vec::new()).unwrap().agents["codex"].events,
            8
        );
        let before = records(&home);
        let fixture = std::fs::read_to_string(&file).unwrap();
        let changed = if truncate {
            fixture.lines().take(5).collect::<Vec<_>>().join("\n") + "\n"
        } else {
            fixture.replace(
                "Add a 50ms timeout to fetchJson",
                "Rewrite the earlier prompt",
            )
        };
        std::fs::write(&file, changed).unwrap();
        let mut out = Vec::new();
        let refused = import(&home, &roots, true, &mut out).unwrap_err();
        assert!(
            format!("{refused:#}").contains("imported prefix"),
            "{refused:#}"
        );
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains(file.file_name().unwrap().to_str().unwrap()),
            "{said}"
        );
        let report: Value = serde_json::from_str(said.lines().last().unwrap()).unwrap();
        assert_eq!(report["agents"]["codex"]["refused"], 1);
        assert_eq!(records(&home), before);
    }
}

#[test]
fn typed_transcript_refusal_keeps_independent_commits_and_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    import(&home, &roots, true, &mut Vec::new()).unwrap();
    let path = root.join("2026/09/02/rollout-basic.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("Add a 50ms timeout to fetchJson", "Changed prior prompt"),
    )
    .unwrap();
    std::fs::write(
        root.join("2026/09/02/rollout-new.jsonl"),
        text.replace(CODEX_SESSION, "33333333-3333-4333-8333-333333333333"),
    )
    .unwrap();
    let mut progress = Vec::new();
    let result = run(&home, &roots, None, &mut |c| progress.push(c.clone())).unwrap_err();
    assert_eq!(result.code, crate::preview::FailureCode::Refused);
    let stats = &result.outcome.stats.agents["codex"];
    assert_eq!((stats.events, stats.refused), (8, 1));
    assert!(matches!(
        progress.last(),
        Some(Committed::Transcripts { events: 8, .. })
    ));
    let again = run(&home, &roots, None, &mut |_| {}).unwrap_err();
    let stats = &again.outcome.stats.agents["codex"];
    assert_eq!((stats.events, stats.refused), (0, 1));
}

/// A forget registered while a transcript is imported, after its file passed the cut and
/// before a later batch of it: the store refuses that batch for the forget's sake (#949), so
/// the file is refused where it stands and the file after it is still read. The refusal inside
/// the batch's transaction was a plain error, which ended the run there.
#[test]
fn a_forget_between_a_files_batches_refuses_that_file_and_the_next_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions/2026/09/02");
    std::fs::create_dir_all(&root).unwrap();
    let at = |ms: i64| format!("2026-09-02T00:00:{:02}.{:03}Z", ms / 1000, ms % 1000);
    let rollout = |name: &str, id: &str, calls: i64, bytes: usize| {
        let mut file = std::fs::File::create(root.join(name)).unwrap();
        writeln!(file, "{}", json!({"timestamp": at(0), "type": "session_meta", "payload": {"id": id, "cwd": "/gone/repo"}})).unwrap();
        writeln!(file, "{}", json!({"timestamp": at(0), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"text": "Import the tool history."}]}})).unwrap();
        for i in 0..calls {
            writeln!(file, "{}", json!({"timestamp": at(i * 2 + 1), "type": "response_item", "payload": {"type": "function_call", "call_id": format!("t{i}"), "name": "Read", "arguments": json!({"file": format!("file {i}")}).to_string()}})).unwrap();
            writeln!(file, "{}", json!({"timestamp": at(i * 2 + 2), "type": "response_item", "payload": {"type": "function_call_output", "call_id": format!("t{i}"), "output": "x".repeat(bytes)}})).unwrap();
        }
    };
    // More than one batch by its bytes, and fewer records in the first than one forget takes.
    rollout("rollout-a.jsonl", "big", 100, 60_000);
    rollout("rollout-b.jsonl", "next", 1, 1);
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // A hook recorded the session's next prompt: the transcript's lines are all before it.
    let mut raw = raw::open(&home).unwrap();
    let live = capture::events(
        "codex",
        "UserPromptSubmit",
        &json!({"session_id": "big", "cwd": "/work", "prompt": "Already captured"}),
        CODEX_MS + 10_000,
        &Settings::default(),
    );
    raw.append_with_ledger(&live[0].event, &live[0].ledger, "test")
        .unwrap();
    drop(raw);
    let mut forgot = false;
    let result = run(&home, &[("codex", &root)], None, &mut |c| {
        if !forgot && matches!(c, Committed::Transcripts { .. }) {
            forgot = true;
            // The hook's record is the store's first.
            let (device, seq) = {
                let raw = raw::open(&home).unwrap();
                let first = raw.after(raw.device(), 0, 1).unwrap()[0].seq;
                (raw.device().to_owned(), first)
            };
            let target = crate::forget::Target::Record { device, seq };
            let preview = crate::forget::preview(&home, target).unwrap();
            crate::forget::start(&home, &preview).unwrap();
        }
    })
    .unwrap_err();
    assert!(forgot);
    assert_eq!(result.code, crate::preview::FailureCode::Refused);
    assert_eq!(result.outcome.stats.agents["codex"].refused, 1);
    assert!(
        records(&home).iter().any(|e| e.session == "next"),
        "the file after the refused one was not read"
    );
}

/// What a confirm still stops for, with nothing stored (#947 changed what a changed file
/// does, and none of this): a preview of the codex folder, then `change` on the home, then
/// its confirm, asked for `agent`'s folder.
fn stale_confirm(agent: &str, change: impl FnOnce(&Path)) {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let home = dir.path().join("home");
    let shown = preview_plan(&home, &[("codex", root.as_path())]).unwrap();
    change(&home);
    let store = raw::path(&home).exists();
    let failed = run(&home, &[(agent, root.as_path())], Some(shown), &mut |_| {
        panic!("no committed effects")
    })
    .unwrap_err();
    assert_eq!(failed.code, FailureCode::Stale, "{failed:?}");
    assert!(failed.outcome.stats.agents.is_empty());
    assert_eq!(raw::path(&home).exists(), store);
    assert!(!store || records(&home).is_empty());
}

#[test]
fn a_confirm_stops_when_the_settings_changed_since_its_preview() {
    stale_confirm("codex", |home| {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join("config.toml"), "# new\n").unwrap();
    });
}

#[test]
fn a_confirm_stops_when_the_store_appeared_since_its_preview() {
    stale_confirm("codex", |home| {
        std::fs::create_dir_all(home).unwrap();
        drop(raw::open(home).unwrap());
    });
}

#[test]
fn a_confirm_stops_when_it_is_asked_for_other_folders_than_its_preview_read() {
    stale_confirm("claude", |_| {});
}

/// A bound file whose bytes changed since the preview, its size and time as they were, waits
/// for a later import: the confirm stores none of it.
#[test]
fn a_confirm_leaves_a_transcript_whose_bytes_changed_since_its_preview_for_later() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let path = root.join("2026/09/02/rollout-basic.jsonl");
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let times =
        std::fs::FileTimes::new().set_modified(file.metadata().unwrap().modified().unwrap());
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Try again with 100ms", "Try again with 200ms");
    std::fs::write(&path, text).unwrap();
    file.set_times(times).unwrap();
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// A file the preview could not read as stable, and so counted as waiting with none of its
/// lines bound, is not read by the confirm, though it is the same file by its bytes: the
/// preview showed none of it.
#[test]
fn a_confirm_reads_no_transcript_its_preview_counted_as_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let touched = root.join("2026/09/02/rollout-basic.jsonl");
    // Its time changes while the preview reads it, its bytes as they were.
    AFTER_PARSE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&touched)
                .unwrap();
            let later =
                file.metadata().unwrap().modified().unwrap() + std::time::Duration::from_secs(60);
            file.set_times(std::fs::FileTimes::new().set_modified(later))
                .unwrap();
        }))
    });
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let codex = &shown.preview.candidates.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// Two files of one session: the preview counts the first as what is imported and the second
/// as seen before. When the first waits, the second is not imported in its stead (Codex's
/// security review of #947): the preview showed none of it as to be imported, and the
/// session's checkpoint would be of the copy, not of the file a later import reads first.
/// For each way a bound file comes to wait.
#[test]
fn a_confirm_imports_no_copy_of_a_session_whose_first_file_waits() {
    for why in [
        "it grew before its turn",
        "its time changed while it was read",
        "it was replaced while it was read",
        "it reads as other lines",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_, root) = fixtures(dir.path());
        let first = root.join("2026/09/02/rollout-basic.jsonl");
        // The same session again, with a prompt the first file does not have.
        let copy = root.join("2026/09/02/rollout-copy.jsonl");
        let mut text = std::fs::read_to_string(&first).unwrap();
        text.push_str(
                r#"{"timestamp":"2026-09-02T00:00:20.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"only in the copy"}]}}"#,
            );
        text.push('\n');
        std::fs::write(&copy, text).unwrap();
        let home = dir.path().join("home");
        let roots = [("codex", root.as_path())];
        let mut shown = preview_plan(&home, &roots).unwrap();
        let codex = &shown.preview.candidates.agents["codex"];
        assert_eq!((codex.files, codex.events, codex.waiting), (2, 8, 0));
        let seen = codex.seen;
        assert!(seen > 0, "the copy is counted as seen before");
        // The confirm reads the first file first: a hook below runs in that reading.
        match why {
            "it grew before its turn" => {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&first)
                    .unwrap();
                file.write_all(b"\n").unwrap();
            }
            "its time changed while it was read" => AFTER_PARSE.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&first)
                        .unwrap();
                    let later = file.metadata().unwrap().modified().unwrap()
                        + std::time::Duration::from_secs(60);
                    file.set_times(std::fs::FileTimes::new().set_modified(later))
                        .unwrap();
                }))
            }),
            "it was replaced while it was read" => AFTER_PARSE.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    // Other bytes, its size and time as they were.
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&first)
                        .unwrap();
                    let times = std::fs::FileTimes::new()
                        .set_modified(file.metadata().unwrap().modified().unwrap());
                    let text = std::fs::read_to_string(&first).unwrap();
                    let other = text.replace("Try again with 100ms", "Try again with 200ms");
                    assert_ne!(other, text);
                    std::fs::write(&first, other).unwrap();
                    file.set_times(times).unwrap();
                }))
            }),
            _ => shown.files[0].parsed = parsed_digest(&[]).unwrap(),
        }
        let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
        let codex = &done.stats.agents["codex"];
        assert_eq!(
            (codex.files, codex.events, codex.waiting, codex.seen),
            (2, 0, 1, seen),
            "{why}"
        );
        assert!(records(&home).is_empty(), "{why}");
    }
}

/// A bound file that is the same by its bytes and reads as other lines than the preview read
/// of it (the plan is made to say so here) waits too: none of it is stored.
#[test]
fn a_confirm_leaves_a_transcript_that_reads_as_other_lines_than_its_preview_read() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let mut shown = preview_plan(&home, &roots).unwrap();
    shown.files[0].parsed = parsed_digest(&[]).unwrap();
    let done = run(&home, &roots, Some(shown), &mut |_| panic!("no commits")).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 0, 1));
    assert!(records(&home).is_empty());
}

/// A transcript that appeared since the preview is none the preview bound: the confirm
/// imports what was bound and does not read it. The next import brings it.
#[test]
fn a_confirm_reads_no_transcript_its_preview_did_not_bind() {
    const LATER: &str = "33333333-3333-4333-8333-333333333333";
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let text = std::fs::read_to_string(CODEX).unwrap();
    std::fs::write(
        root.join("2026/09/02/rollout-new.jsonl"),
        text.replace(CODEX_SESSION, LATER),
    )
    .unwrap();
    let done = run(&home, &roots, Some(shown), &mut |_| {}).unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (1, 8, 0));
    let stored = records(&home);
    assert_eq!(stored.len(), 8);
    assert!(stored.iter().all(|e| e.session == CODEX_SESSION));
    import(&home, &roots, true, &mut Vec::new()).unwrap();
    let stored = records(&home);
    assert_eq!(stored.iter().filter(|e| e.session == LATER).count(), 8);
    assert_eq!(stored.len(), 16);
}

/// The next bound file changes after a commit of the one before it: the confirm keeps what
/// it stored and leaves the changed file for a later import, which brings it.
#[test]
fn a_confirm_leaves_the_next_bound_file_for_later_when_it_changes_after_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = fixtures(dir.path());
    let next = root.join("2026/09/02/rollout-new.jsonl");
    let text = std::fs::read_to_string(CODEX)
        .unwrap()
        .replace(CODEX_SESSION, "33333333-3333-4333-8333-333333333333");
    std::fs::write(&next, &text).unwrap();
    let home = dir.path().join("home");
    let roots = [("codex", root.as_path())];
    let shown = preview_plan(&home, &roots).unwrap();
    let done = run(&home, &roots, Some(shown), &mut |c| {
        if matches!(c, Committed::Transcripts { .. }) {
            std::fs::write(
                &next,
                text.replace("Try again with 100ms", "Try again with 200ms"),
            )
            .unwrap();
        }
    })
    .unwrap();
    let codex = &done.stats.agents["codex"];
    assert_eq!((codex.files, codex.events, codex.waiting), (2, 8, 1));
    assert_eq!(records(&home).len(), 8);
    let shown = preview_plan(&home, &roots).unwrap();
    let resumed = run(&home, &roots, Some(shown), &mut |_| {}).unwrap();
    assert_eq!(resumed.stats.agents["codex"].events, 8);
    assert_eq!(records(&home).len(), 16);
}

#[test]
fn a_killed_import_resumes_without_duplicates_or_gaps() {
    for (count, bytes) in [(100, 60_000), (600, 10)] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions/2026/09/02");
        std::fs::create_dir_all(&root).unwrap();
        let mut file = std::fs::File::create(root.join("rollout-big.jsonl")).unwrap();
        let at = |ms: i64| format!("2026-09-02T00:00:{:02}.{:03}Z", ms / 1000, ms % 1000);
        writeln!(file, "{}", json!({"timestamp": at(0), "type": "session_meta", "payload": {"id": "big", "cwd": "/gone/repo"}})).unwrap();
        writeln!(file, "{}", json!({"timestamp": at(0), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"text": "Import the tool history."}]}})).unwrap();
        for i in 0..count {
            writeln!(file, "{}", json!({"timestamp": at(i * 2 + 1), "type": "response_item", "payload": {"type": "function_call", "call_id": format!("t{i}"), "name": "Read", "arguments": json!({"file": format!("file {i}")}).to_string()}})).unwrap();
            writeln!(file, "{}", json!({"timestamp": at(i * 2 + 2), "type": "response_item", "payload": {"type": "function_call_output", "call_id": format!("t{i}"), "output": "x".repeat(bytes)}})).unwrap();
        }
        drop(file);
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        drop(raw::open(&home).unwrap());
        // Arm after parsing so SQLite setup is outside the two batch commits.
        AFTER_PARSE.with_borrow_mut(|hook| *hook = Some(Box::new(|| crate::crash::at(2))));
        let mut progress = Vec::new();
        let killed = run(&home, &[("codex", &root)], None, &mut |c| {
            progress.push(c.clone())
        });
        crate::crash::off();
        let killed = killed.unwrap_err();
        let landed = records(&home).len();
        assert!(
            landed > 0 && landed < count as usize + 2,
            "{landed} landed: {killed:?}"
        );
        assert_eq!(killed.outcome.stats.agents["codex"].events, landed as u64);
        assert!(
            matches!(progress.last(), Some(Committed::Transcripts { events, .. }) if *events == landed as u64)
        );
        import(&home, &[("codex", &root)], true, &mut Vec::new()).unwrap();
        let got = records(&home);
        assert_eq!(got.len(), count as usize + 2);
        assert!(
            got.iter()
                .all(|e| e.source == "transcript" && e.repo.as_deref() == Some("/gone/repo"))
        );
        let inputs: Vec<Value> = got
            .iter()
            .filter(|e| e.kind == "tool")
            .map(|e| {
                let body: Value = serde_json::from_str(&e.body).unwrap();
                serde_json::from_str(body["input"].as_str().unwrap()).unwrap()
            })
            .collect();
        let want: Vec<Value> = (0..count)
            .map(|i| json!({"file": format!("file {i}")}))
            .collect();
        assert_eq!(inputs, want);
        assert!(got.windows(2).all(|w| w[0].ts <= w[1].ts));
        let again = import(&home, &[("codex", &root)], true, &mut Vec::new()).unwrap();
        assert_eq!(
            (again.agents["codex"].events, again.agents["codex"].seen),
            (0, count as u64 + 2)
        );
    }
}

#[test]
fn a_denied_line_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let (_, codex) = fixtures(dir.path());
    let path = codex.join("2026/09/02/rollout-basic.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Add a 50ms timeout to fetchJson", raw::DENIED_IN_TESTS)
        .replace("Try again with 100ms", "Try again\u{2028}with 100ms");
    std::fs::write(path, text).unwrap();
    let home = dir.path().join("home");
    let first = import(&home, &[("codex", &codex)], true, &mut Vec::new()).unwrap();
    assert_eq!(first.agents["codex"].events, 7);
    let got = records(&home);
    assert_eq!(got.len(), 7);
    assert!(got.iter().all(|e| !e.body.contains(raw::DENIED_IN_TESTS)));
    assert!(
        got.iter()
            .any(|e| e.body.contains("Try again\u{2028}with 100ms"))
    );
    let again = import(&home, &[("codex", &codex)], true, &mut Vec::new()).unwrap();
    assert_eq!(
        (again.agents["codex"].events, again.agents["codex"].seen),
        (0, 8)
    );
}

#[test]
fn a_masked_session_cannot_bypass_the_cut_or_leak_through_its_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let (claude, _) = fixtures(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[redaction]\nextra_rules = [{id = 'session', regex = 'claude-basic'}]\n",
    )
    .unwrap();
    let settings = Settings::load(&home).unwrap();
    let live = capture::events(
        "claude",
        "UserPromptSubmit",
        &json!({"session_id": "claude-basic", "prompt": "Live"}),
        CLAUDE_MS + 4_000,
        &settings,
    );
    assert_ne!(live[0].event.session, "claude-basic");
    let mut raw = raw::open(&home).unwrap();
    raw.append_with_ledger(&live[0].event, &live[0].ledger, settings.rules.version())
        .unwrap();
    drop(raw);
    let mut out = Vec::new();
    let stats = import(&home, &[("claude", &claude)], true, &mut out).unwrap();
    assert_eq!(stats.agents["claude"].masked, 1);
    assert!(!String::from_utf8(out).unwrap().contains("claude-basic"));
    assert_eq!(records(&home).len(), 1);
    assert!(
        raw::open(&home)
            .unwrap()
            .migration_checkpoints("transcript:")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn preview_reads_wal_events_without_changing_the_home_or_a_symlink_target() {
    #[cfg(unix)]
    let variants = [false, true];
    #[cfg(not(unix))]
    let variants = [false];
    for link in variants {
        let dir = tempfile::tempdir().unwrap();
        let (_, codex) = fixtures(dir.path());
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let path = if link {
            dir.path().join("store.db")
        } else {
            home.join("oboete.db")
        };
        let v1 = Connection::open(&path).unwrap();
        v1.pragma_update(None, "journal_mode", "WAL").unwrap();
        v1.execute_batch(
            "CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL);
                 CREATE TABLE events(session_id TEXT NOT NULL, ts INTEGER NOT NULL);
                 PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
        v1.execute("INSERT INTO sessions VALUES(?1, 'codex')", [CODEX_SESSION])
            .unwrap();
        v1.execute(
            "INSERT INTO events VALUES(?1, ?2)",
            params![CODEX_SESSION, CODEX_MS + 8_000],
        )
        .unwrap();
        #[cfg(unix)]
        if link {
            std::os::unix::fs::symlink(&path, home.join("oboete.db")).unwrap();
        }
        let before = snapshot(dir.path());
        let stats = import(&home, &[("codex", &codex)], false, &mut Vec::new()).unwrap();
        assert_eq!(
            (stats.agents["codex"].events, stats.agents["codex"].cut),
            (4, 4)
        );
        assert!(
            snapshot(dir.path()) == before,
            "preview changed a source or destination file"
        );
    }
}

fn events(path: &str, agent: &str) -> (Vec<Value>, Stats) {
    let mut buf = Vec::new();
    let stats = convert(Path::new(path), agent, &mut buf).unwrap();
    let text = String::from_utf8(buf).unwrap();
    (
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
        stats,
    )
}

fn names(v: &[Value]) -> Vec<&str> {
    v.iter().map(|e| e["event"].as_str().unwrap()).collect()
}

const CLAUDE: &str = "src/testdata/transcripts/claude-basic.jsonl";

#[test]
fn a_queued_prompt_leaves_its_turn_running_and_a_stop_keeps_its_directory() {
    let dir = std::env::temp_dir().join(format!("oboete-transcript-q-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("q.jsonl");
    let at = |s: u32| format!("2026-09-01T00:00:0{s}Z");
    let user = |s, cwd: &str, text: &str| {
        json!({"type": "user", "timestamp": at(s), "cwd": cwd, "sessionId": "s-1",
            "message": {"role": "user", "content": text}})
    };
    let said = |s, text: &str| {
        json!({"type": "assistant", "timestamp": at(s), "cwd": "/a",
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
    };
    let lines = [
        user(1, "/a", "first"),
        said(2, "working"),
        json!({"type": "queue-operation", "operation": "enqueue", "timestamp": at(3), "content": "also this"}),
        said(4, "done"),
        json!({"type": "system", "subtype": "stop_hook_summary", "timestamp": at(5), "cwd": "/a"}),
        user(6, "/a", "also this"),
        said(7, "did it"),
        // Resumed in another directory: the turn before keeps its own.
        user(8, "/b", "next"),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, text).unwrap();
    let (v, _) = events(path.to_str().unwrap(), "claude");
    std::fs::remove_dir_all(&dir).unwrap();
    // The recorded id, not the file's name.
    assert!(
        v.iter()
            .all(|e| e["session"] == "s-1" && e["payload"]["session_id"] == "s-1")
    );
    let got: Vec<(&str, &str, &str, &str)> = v
        .iter()
        .map(|e| {
            let p = &e["payload"];
            let said = p["prompt"]
                .as_str()
                .or(p["last_assistant_message"].as_str());
            (
                e["event"].as_str().unwrap(),
                &e["ts"].as_str().unwrap()[17..19],
                said.unwrap_or(""),
                p["cwd"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("SessionStart", "01", "", "/a"),
            ("UserPromptSubmit", "01", "first", "/a"),
            // Sent to the prompt hook when queued; the turn goes on until its Stop hook ran.
            ("UserPromptSubmit", "03", "also this", "/a"),
            ("Stop", "05", "done", "/a"),
            ("Stop", "08", "did it", "/a"),
            ("UserPromptSubmit", "08", "next", "/b"),
            ("SessionEnd", "08", "", "/b"),
        ]
    );
}

#[test]
fn an_unanswered_subagent_call_keeps_its_directory() {
    let dir = std::env::temp_dir().join(format!("oboete-transcript-sub-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("m").join("subagents")).unwrap();
    let main = json!({"type": "user", "timestamp": "2026-09-01T00:00:01Z", "cwd": "/a",
            "message": {"role": "user", "content": "first"}});
    let call = json!({"type": "assistant", "timestamp": "2026-09-01T00:00:02Z", "cwd": "/wt",
            "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Grep", "input": {}}]}});
    std::fs::write(dir.join("m.jsonl"), format!("{main}\n")).unwrap();
    std::fs::write(
        dir.join("m").join("subagents").join("agent-x.jsonl"),
        format!("{call}\n"),
    )
    .unwrap();
    let (v, _) = events(dir.join("m.jsonl").to_str().unwrap(), "claude");
    std::fs::remove_dir_all(&dir).unwrap();
    let cut = v
        .iter()
        .find(|e| e["payload"]["interrupted"] == true)
        .unwrap();
    assert_eq!(cut["payload"]["agent_id"], "x");
    assert_eq!(cut["payload"]["cwd"], "/wt");
    assert_eq!(v.last().unwrap()["payload"]["cwd"], "/a");
}

#[test]
fn claude_prompts_tools_answers_compaction_and_stops() {
    let (v, _) = events(CLAUDE, "claude");
    // In time order: subagent files are read last but sorted into place. The two calls the
    // interruption at 00:00:23 ended stand there (#950).
    assert_eq!(
        names(&v),
        [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUse",
            "Stop",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PostCompact",
            "PostToolUse",
            "Stop",
            "UserPromptSubmit",
            "UserPromptSubmit",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUse",
            "UserPromptSubmit",
            "SessionEnd"
        ]
    );
    for (i, e) in v.iter().enumerate() {
        assert_eq!(e["seq"], i as u64 + 1);
        assert_eq!(e["session"], "claude-basic");
        // The workflow agent ran in a worktree: its event keeps its own directory, and the
        // session's does not move.
        let cwd = if i == 6 { "/work/app-wt" } else { "/work/app" };
        assert_eq!(e["payload"]["cwd"], cwd, "{i}");
        assert_eq!(e["payload"]["hook_event_name"], e["event"]);
    }
    // An interrupted call's line has the time its call was made; the others' times rise.
    let ts: Vec<&str> = (v[1..].iter())
        .filter(|e| e["payload"]["interrupted"] != true)
        .map(|e| e["ts"].as_str().unwrap())
        .collect();
    assert!(ts.windows(2).all(|w| w[0] <= w[1]), "{ts:?}");
    assert_eq!(v[1]["payload"]["prompt"], "キャッシュの方針を決めたい");
    assert_eq!(v[1]["ts"], "2026-09-01T00:00:00.000Z");
    assert_eq!(v[2]["payload"]["tool_name"], "Read");
    // A subagent's call sits at its own time, inside the turn that started it.
    assert_eq!(v[3]["payload"]["tool_name"], "Grep");
    assert_eq!(v[3]["payload"]["agent_id"], "a1");
    // Stop at the end of the turn (the next prompt's time), with the turn's last text.
    assert_eq!(
        v[4]["payload"]["last_assistant_message"],
        "SQLite にします。"
    );
    assert_eq!(v[4]["ts"], v[5]["ts"]);
    assert_eq!(v[5]["payload"]["prompt"], "どちらが良い？");
    // A workflow agent's file sits deeper under subagents/.
    assert_eq!(v[6]["payload"]["tool_name"], "WebFetch");
    assert_eq!(v[6]["payload"]["agent_id"], "w1");
    assert_eq!(
        v[7]["payload"]["tool_input"]["answers"]["どちらにしますか?"],
        "A にする"
    );
    // A failure carries the error only, as the live hook gets it.
    assert_eq!(v[8]["payload"]["error"], "error: 2 tests failed");
    assert!(v[8]["payload"].get("tool_response").is_none());
    assert!(
        v[9]["payload"]["compact_summary"]
            .as_str()
            .unwrap()
            .contains("SQLite")
    );
    // The inline subagent: its tool call only, never its task as a prompt or its text as a Stop.
    assert_eq!(v[10]["payload"]["tool_name"], "Glob");
    assert_eq!(v[10]["payload"]["agent_id"], "b2");
    assert_eq!(
        v[11]["payload"]["last_assistant_message"],
        "テストを直します。"
    );
    // Command output and a local command are no prompt; a skill command is, as typed.
    assert_eq!(v[12]["payload"]["prompt"], "/graphify src");
    assert_eq!(v[13]["payload"]["prompt"], "/goal finish the cache");
    // A plain-text local command is no prompt; a queued prompt is sent once, when queued.
    assert_eq!(v[14]["payload"]["prompt"], "テストも直して");
    assert_eq!(v[14]["ts"], "2026-09-01T00:00:20.000Z");
    // A queued prompt that starts with markup is still a prompt.
    assert_eq!(v[17]["payload"]["prompt"], "<div>見出し</div> を直して");
    // SessionEnd takes the main file's last time, not a subagent file's.
    assert_eq!(v[18]["ts"], "2026-09-01T00:00:24.000Z");
}

#[test]
fn calls_without_result_end_at_the_interruption() {
    let (v, _) = events(CLAUDE, "claude");
    // Ended by the interruption at 00:00:23: each stands there, after the prompts before it,
    // with the time its call was made.
    assert_eq!(v[14]["ts"], "2026-09-01T00:00:20.000Z");
    assert_eq!(v[15]["payload"]["tool_name"], "Edit");
    assert_eq!(v[15]["payload"]["interrupted"], true);
    assert!(v[15]["payload"]["tool_response"].is_null());
    assert_eq!(v[15]["ts"], "2026-09-01T00:00:11.000Z");
    assert_eq!(v[16]["payload"]["tool_name"], "Bash");
    assert_eq!(v[16]["payload"]["interrupted"], true);
    assert_eq!(v[16]["ts"], "2026-09-01T00:00:22.000Z");
    assert_eq!(v[17]["ts"], "2026-09-01T00:00:24.000Z");
}

#[test]
fn unknown_and_broken_lines_are_skipped() {
    let (v, stats) = events(CLAUDE, "claude");
    let ignored = [
        ("atis-latch".to_string(), 1),
        ("permission-mode".to_string(), 1),
        ("queue-operation".to_string(), 1),
    ]
    .into();
    assert_eq!(
        stats,
        Stats {
            lines: 35,
            skipped: 1,
            events: 19,
            ignored
        }
    );
    assert_eq!(v.len(), 19);
    assert!(convert(Path::new(CLAUDE), "grok", Vec::new()).is_err());
}

#[test]
fn codex_rollout_maps_prompts_calls_turns_and_compaction() {
    let (v, stats) = events("src/testdata/transcripts/codex-basic.jsonl", "codex");
    assert_eq!(stats.ignored, [("world_state:".to_string(), 1)].into());
    assert_eq!(
        names(&v),
        [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUse",
            "Stop",
            "PostCompact",
            "PostToolUse",
            "UserPromptSubmit",
            "SessionEnd"
        ]
    );
    // The fork's own id and cwd, not its parent's second session_meta; then the directory a
    // later turn_context moves to.
    for (i, e) in v.iter().enumerate() {
        assert_eq!(e["session"], "22222222-2222-4222-8222-222222222222");
        let cwd = if i < 7 { "/work/svc" } else { "/work/svc2" };
        assert_eq!(e["payload"]["cwd"], cwd, "{i}");
    }
    assert_eq!(v[1]["payload"]["prompt"], "Add a 50ms timeout to fetchJson");
    assert_eq!(v[2]["payload"]["tool_input"]["cmd"], "rg fetchJson");
    assert_eq!(v[2]["payload"]["tool_response"], "src/http.ts:3");
    assert!(
        v[3]["payload"]["tool_input"]["input"]
            .as_str()
            .unwrap()
            .starts_with("*** Begin Patch")
    );
    assert_eq!(
        v[4]["payload"]["last_assistant_message"],
        "Added the timeout."
    );
    // Harness text marked by content_item_kinds is no prompt; an aborted turn sends no Stop,
    // and its unanswered call ends at the abort, not at the end of the rollout.
    assert_eq!(v[6]["payload"]["tool_input"]["cmd"], "sleep 100");
    assert_eq!(v[6]["payload"]["interrupted"], true);
    assert_eq!(v[7]["payload"]["prompt"], "Try again with 100ms");
}

/// claude-mem 13.35's `isRecursiveMemoryTool` on imported history: a rollout names an MCP
/// call by its namespace and its own name, which joined by `__` are the name Codex gives hooks
/// (Codex on #523), so a read of claude-mem's memory is kept without its output; Codex's own
/// namespaced tools keep their names.
#[test]
fn a_codex_rollouts_mcp_call_is_named_as_codex_names_it_to_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("rollout.jsonl");
    let call = |id: &str, namespace: &str, name: &str| {
        json!({"timestamp": "2026-09-02T00:00:01.000Z", "type": "response_item",
                   "payload": {"type": "function_call", "call_id": id, "namespace": namespace,
                               "name": name, "arguments": "{\"query\":\"tabs\"}"}})
    };
    let output = |id: &str| {
        json!({"timestamp": "2026-09-02T00:00:02.000Z", "type": "response_item",
                   "payload": {"type": "function_call_output", "call_id": id,
                               "output": "Use tabs (decided)."}})
    };
    let lines = [
        json!({"timestamp": "2026-09-02T00:00:00.000Z", "type": "session_meta",
                   "payload": {"id": "66666666-6666-4666-8666-666666666666", "cwd": "/work"}}),
        call("a", "mcp__mcp_search", "search"),
        output("a"),
        call("b", "history", "search_contents"),
        output("b"),
        call(
            "c",
            "mcp__codex_apps__codex_security_access",
            "_get_tac_status",
        ),
        output("c"),
        call("d", "mcp__mcp_search__", "timeline"),
        output("d"),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&rollout, text).unwrap();
    let (v, _) = events(rollout.to_str().unwrap(), "codex");
    let tools: Vec<_> = v.iter().filter(|e| e["event"] == "PostToolUse").collect();
    let names: Vec<_> = tools.iter().map(|e| &e["payload"]["tool_name"]).collect();
    assert_eq!(
        names,
        [
            "mcp__mcp_search__search",
            "search_contents",
            "mcp__codex_apps__codex_security_access__get_tac_status",
            "mcp__mcp_search__timeline"
        ]
    );
    let stored = |e: &Value| {
        let c = capture::imported(
            "codex",
            "PostToolUse",
            &e["payload"],
            1,
            "/work",
            Some("/work"),
            &Settings::default(),
        );
        serde_json::from_str::<Value>(&c[0].event.body).unwrap()
    };
    let read = stored(tools[0]);
    assert_eq!(
        (&read["output"], &read["memory_read"]),
        (&json!(""), &json!(true))
    );
    assert!(
        stored(tools[1])["output"]
            .as_str()
            .unwrap()
            .contains("Use tabs")
    );
}

/// #273: a rollout another agent started (`codex exec` here) says its prompts are the
/// agent's, and one with no originator (the fixture) says they are the user's. Only the
/// rollout's own `session_meta` counts, not a forked parent's after it.
#[test]
fn a_codex_rollout_says_who_sent_its_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("rollout.jsonl");
    let meta = |id: &str, originator: &str, source: &str| {
        json!({"timestamp": "2026-09-02T00:00:00.000Z", "type": "session_meta",
                   "payload": {"id": id, "cwd": "/work/svc", "originator": originator,
                               "source": source}})
    };
    let typed = json!({"timestamp": "2026-09-02T00:00:01.000Z", "type": "response_item",
                           "payload": {"type": "message", "role": "user",
                                       "content": [{"type": "input_text", "text": "Review the diff."}]}});
    let lines = [
        meta("44444444-4444-4444-8444-444444444444", "codex_exec", "exec"),
        meta("55555555-5555-4555-8555-555555555555", "codex-tui", "cli"),
        typed,
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&rollout, text).unwrap();
    let sender = |path: &str| {
        let (v, _) = events(path, "codex");
        let prompt = v.iter().find(|e| e["event"] == "UserPromptSubmit").unwrap();
        prompt["payload"][crate::capture::AGENT_SENT].clone()
    };
    assert_eq!(sender(rollout.to_str().unwrap()), true);
    assert_eq!(sender("src/testdata/transcripts/codex-basic.jsonl"), false);
    // Replayed (`oboete transcript` then `oboete replay`), the prompt is stored as the
    // agent's: `record` keeps the mark, which only the live hook drops from a payload.
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut raw = crate::raw::open(&home).unwrap();
    let (v, _) = events(rollout.to_str().unwrap(), "codex");
    // Replayed where the rollout is gone: only the parser's mark can say an agent sent it.
    std::fs::remove_file(&rollout).unwrap();
    for e in &v {
        let event = e["event"].as_str().unwrap();
        crate::hook::record(
            &home,
            &mut raw,
            "codex",
            event,
            &e["payload"],
            0,
            &Default::default(),
            &mut 0,
            &mut 0,
        )
        .unwrap();
    }
    let prompts: Vec<Value> = (raw.after(raw.device(), 0, 100).unwrap().into_iter())
        .filter_map(|r| match r.item {
            crate::raw::Item::Event(e) if e.kind == "prompt" => Some(e.body),
            _ => None,
        })
        .map(|b| serde_json::from_str(&b).unwrap())
        .collect();
    assert_eq!(
        prompts,
        [json!({"prompt": "Review the diff.", "agent_sent": true})]
    );
}

/// O2 (docs/my-work-model.md): an imported Claude Code line keeps its entry point, and one
/// whose `origin` or `turnOrigin` is not a person's typing (a task's notification, a scheduled
/// turn) is marked another program's; only the person's terminal prompts are typed.
#[test]
fn an_imported_claude_line_is_typed_only_when_its_origin_is_human() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("claude/projects");
    let project = root.join("-work-app");
    std::fs::create_dir_all(&project).unwrap();
    let session = "66666666-6666-4666-8666-666666666666";
    let line = |second: u32, text: &str, entrypoint: &str, origin: Value| {
        let mut v = json!({"type": "user", "cwd": "/work/app", "sessionId": session,
                "timestamp": format!("2026-09-02T00:00:{second:02}.000Z"),
                "entrypoint": entrypoint,
                "message": {"role": "user", "content": text}});
        (v.as_object_mut().unwrap()).extend(origin.as_object().unwrap().clone());
        format!("{v}\n")
    };
    let queued = |second: u32, text: &str| {
        let v = json!({"type": "queue-operation", "operation": "enqueue", "sessionId": session,
                "timestamp": format!("2026-09-02T00:00:{second:02}.000Z"), "content": text});
        format!("{v}\n")
    };
    let text = [
        line(
            1,
            "Fix the parser.",
            "cli",
            json!({"origin": {"kind": "human"}}),
        ),
        line(
            2,
            "Summarize the run.",
            "cli",
            json!({"origin": {"kind": "agent"}}),
        ),
        line(3, "Check the build.", "sdk-cli", json!({})),
        line(
            4,
            "Run the nightly check.",
            "cli",
            json!({"turnOrigin": "scheduled"}),
        ),
        line(
            5,
            "Ship it.",
            "cli",
            json!({"turnOrigin": "human", "promptSource": "typed"}),
        ),
        // Queued while a turn runs: the enqueue line names neither, its delivery both.
        queued(6, "Also run the tests."),
        line(
            7,
            "Also run the tests.",
            "cli",
            json!({"turnOrigin": "human"}),
        ),
        queued(8, "Check the nightly run."),
        line(
            9,
            "Check the nightly run.",
            "cli",
            json!({"turnOrigin": "scheduled"}),
        ),
        // Never delivered: the session's entry point is all it has.
        queued(10, "Then deploy."),
    ]
    .concat();
    std::fs::write(project.join(format!("{session}.jsonl")), text).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    import(&home, &[("claude", root.as_path())], true, &mut Vec::new()).unwrap();
    let prompts: Vec<Value> = (records(&home).into_iter())
        .filter(|e| e.kind == "prompt")
        .map(|e| serde_json::from_str(&e.body).unwrap())
        .collect();
    assert_eq!(
        prompts,
        [
            json!({"prompt": "Fix the parser.", "entrypoint": "cli"}),
            json!({"prompt": "Summarize the run.", "agent_sent": true, "entrypoint": "cli"}),
            json!({"prompt": "Check the build.", "entrypoint": "sdk-cli"}),
            json!({"prompt": "Run the nightly check.", "agent_sent": true, "entrypoint": "cli"}),
            json!({"prompt": "Ship it.", "entrypoint": "cli"}),
            json!({"prompt": "Also run the tests.", "entrypoint": "cli"}),
            json!({"prompt": "Check the nightly run.", "agent_sent": true, "entrypoint": "cli"}),
            json!({"prompt": "Then deploy.", "entrypoint": "cli"}),
        ]
    );
    let typed: Vec<bool> = (prompts.iter())
        .map(|b| crate::typed::typed("prompt", "claude", "transcript", b))
        .collect();
    assert_eq!(typed, [true, false, false, false, true, true, false, true]);
    // #841: their entry point is not their text.
    for e in records(&home).iter().filter(|e| e.kind == "prompt") {
        let body: Value = serde_json::from_str(&e.body).unwrap();
        assert_eq!(
            crate::curate::long_text(e).as_deref(),
            body["prompt"].as_str()
        );
    }
}

/// A line's identity is made of the bytes it had when transcripts were first imported: no
/// entry point, no mark of who sent it, no call's start, and `transcript_path` taken out as
/// it always was (the last field in its place). Spelled out here for lines as the parser
/// reads them now, with what it has read of them since in the middle of their fields, or
/// after them.
#[test]
fn a_parsed_line_has_the_identity_it_was_imported_with() {
    let record = |kind: &str, at: u32, more: Value, content: Value| {
        let mut v = json!({"type": kind, "sessionId": "then", "cwd": "/work/app",
                "timestamp": format!("2026-09-01T00:00:{at:02}.000Z"),
                "message": {"role": kind, "content": content}});
        for (field, value) in more.as_object().unwrap() {
            v[field] = value.clone();
        }
        format!("{v}\n")
    };
    let prompt = |at: u32, more: Value, text: &str| record("user", at, more, json!(text));
    let call = json!([{"type": "tool_use", "id": "fast", "name": "Bash",
            "input": {"command": "true"}}]);
    let result = json!([{"type": "tool_result", "tool_use_id": "fast", "content": "done"}]);
    let queued = record(
        "queue-operation",
        3,
        json!({"operation": "enqueue", "content": "Queued prompt."}),
        json!(null),
    );
    let then = |event: &str, at: u32, fields: &str| {
        let tail = format!(r#""session_id":"then","hook_event_name":"{event}","cwd":"/work/app""#);
        format!(
            r#"["claude","{event}","then","2026-09-01T00:00:{at:02}.000Z",{{{fields},{tail}}}]"#
        )
    };
    let start = then("SessionStart", 0, r#""source":"startup""#);
    let asked =
        |at: u32, text: &str| then("UserPromptSubmit", at, &format!(r#""prompt":"{text}""#));
    let done = r#""tool_name":"Bash","tool_input":{"command":"true"},"tool_response":"done""#;
    let cases = [
        // A prompt with an entry point and a call with a result: each read since stands in
        // the middle of its line's fields.
        (
            format!(
                "{}{}{}",
                prompt(0, json!({"entrypoint": "cli"}), "Run it."),
                record("assistant", 1, json!({}), call),
                record("user", 2, json!({}), result)
            ),
            vec![
                start.clone(),
                asked(0, "Run it."),
                then("PostToolUse", 2, done),
            ],
        ),
        // A queued prompt: its delivery says who sent it, after every other field.
        (
            format!(
                "{}{queued}{}",
                prompt(0, json!({}), "Run it."),
                prompt(5, json!({"turnOrigin": "scheduled"}), "Queued prompt.")
            ),
            vec![
                start.clone(),
                asked(0, "Run it."),
                asked(3, "Queued prompt."),
            ],
        ),
    ];
    for (n, (transcript, then)) in cases.into_iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("then.jsonl");
        std::fs::write(&file, transcript).unwrap();
        let mut lines = stable_lines(&file, "claude").unwrap().unwrap();
        lines.retain(|line| !line.synthetic && line.event != "SessionEnd");
        let read: Vec<String> = (lines.iter())
            .map(|line| format!("{} {} {}", line.event, line.ts, line.payload))
            .collect();
        // Read since the first imports, and still in the lines.
        let since = [
            crate::capture::ENTRYPOINT,
            crate::capture::STARTED,
            crate::capture::AGENT_SENT,
        ];
        assert!(
            lines
                .iter()
                .any(|line| since.iter().any(|f| line.payload.get(*f).is_some())),
            "case {n}: {read:#?}"
        );
        let now: Vec<String> = (lines.iter())
            .map(|line| {
                let mut identity = Sha256::new();
                hash_line(&mut identity, line).unwrap();
                format!("{:x}", identity.finalize())
            })
            .collect();
        let then: Vec<String> = (then.iter())
            .map(|bytes| format!("{:x}", Sha256::digest(format!("{bytes}\n"))))
            .collect();
        assert_eq!(now, then, "case {n}: {read:#?}");
    }
}

/// #950: a call the developer's stop ended is the line it was: its time is the time the call
/// was made, and nothing is added to it. A call with no time of its own has, as it had, the
/// time of the line written before its own, which is the last one before the stop and not
/// the one before the call. Only the line's place is the stop's, and only when that is
/// later. Spelled out as the bytes each line's identity is made of, in the order the lines
/// stand in: a checkpoint's prefix is both.
#[test]
fn an_interrupted_calls_line_keeps_its_bytes_and_stands_where_its_turn_was_stopped() {
    let then = |event: &str, second: u32, fields: &str| {
        let tail =
            format!(r#""session_id":"pending","hook_event_name":"{event}","cwd":"/work/app""#);
        format!(
            r#"["claude","{event}","pending","2026-09-01T00:00:{second:02}.000Z",{{{fields},{tail}}}]"#
        )
    };
    let start = |second: u32| then("SessionStart", second, r#""source":"startup""#);
    let asked = |second: u32, text: &str| {
        then("UserPromptSubmit", second, &format!(r#""prompt":"{text}""#))
    };
    let slow = |second: u32| {
        let call = r#""tool_name":"Bash","tool_input":{"command":"sleep 60"}"#;
        let ended = r#""tool_response":null,"interrupted":true"#;
        then("PostToolUse", second, &format!("{call},{ended}"))
    };
    let fast = |second: u32| {
        let call = r#""tool_name":"Bash","tool_input":{"command":"true"}"#;
        then(
            "PostToolUse",
            second,
            &format!(r#"{call},"tool_response":"done""#),
        )
    };
    let prompt = |second: Option<u32>, text: &str| claude_wrote("user", second, json!(text));
    let slow_call = |second: Option<u32>| {
        let call = json!([{"type": "tool_use", "id": "slow", "name": "Bash",
                "input": {"command": "sleep 60"}}]);
        claude_wrote("assistant", second, call)
    };
    let cases = [
        // A call made beside it got its result before the stop: the interrupted call's
        // line comes after that result's, with the time the call was made.
        (
            prompt(Some(0), "Run both.")
                + &two_calls(Some(1))
                + &fast_result(2)
                + &prompt(Some(9), STOPPED),
            vec![start(0), asked(0, "Run both."), fast(2), slow(1)],
        ),
        // Nothing between the call and the stop: the order a transcript imported before
        // has.
        (
            prompt(Some(0), "Run it.")
                + &slow_call(Some(1))
                + &prompt(Some(9), STOPPED)
                + &prompt(Some(12), "Go on."),
            vec![start(0), asked(0, "Run it."), slow(1), asked(12, "Go on.")],
        ),
        // The interrupted call is the session's first event: the session starts at the time
        // the call was made.
        (
            slow_call(Some(1)) + &prompt(Some(9), STOPPED),
            vec![start(1), slow(1)],
        ),
        // A call with no time of its own has the time of the line written before its own.
        (
            prompt(Some(0), "Run it.") + &slow_call(None) + &prompt(Some(9), STOPPED),
            vec![start(0), asked(0, "Run it."), slow(0)],
        ),
        // With a result between that call and the stop, that line is the result's: the
        // time the call's line had before #950, and not the time of the line before the
        // call (CodeRabbit on #969).
        (
            prompt(Some(0), "Run both.")
                + &two_calls(None)
                + &fast_result(2)
                + &prompt(Some(9), STOPPED),
            vec![start(0), asked(0, "Run both."), fast(2), slow(2)],
        ),
        // A prompt with no time of its own after the stop has the time the call was made,
        // as it had, and comes after the call's line.
        (
            prompt(Some(0), "Run both.")
                + &two_calls(Some(1))
                + &fast_result(2)
                + &prompt(Some(9), STOPPED)
                + &prompt(None, "Go on."),
            vec![
                start(0),
                asked(0, "Run both."),
                fast(2),
                slow(1),
                asked(1, "Go on."),
            ],
        ),
        // A stop with no time of its own leaves the call's line where it was.
        (
            prompt(Some(0), "Run both.")
                + &two_calls(Some(1))
                + &fast_result(2)
                + &prompt(None, STOPPED),
            vec![start(0), asked(0, "Run both."), slow(1), fast(2)],
        ),
    ];
    let mut wrong = Vec::new();
    for (n, (transcript, then)) in cases.into_iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pending.jsonl");
        std::fs::write(&file, transcript).unwrap();
        let mut lines = stable_lines(&file, "claude").unwrap().unwrap();
        lines.retain(|line| !line.synthetic && line.event != "SessionEnd");
        let read: Vec<String> = (lines.iter())
            .map(|line| format!("{} {} {}", line.event, line.ts, line.payload))
            .collect();
        let now: Vec<String> = (lines.iter())
            .map(|line| {
                let mut identity = Sha256::new();
                hash_line(&mut identity, line).unwrap();
                format!("{:x}", identity.finalize())
            })
            .collect();
        let then: Vec<String> = (then.iter())
            .map(|bytes| format!("{:x}", Sha256::digest(format!("{bytes}\n"))))
            .collect();
        if now != then {
            wrong.push(format!("case {n}: {read:#?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Codex on #659: a Claude Code line's marks (O2) are not part of its identity, so a transcript
/// imported before they were read still matches its checkpoint and its records; a Codex
/// rollout's mark (#273) still is part of it. The marks stand where the parser puts them,
/// before the fields every line gets: taken out, they leave those in the order they had.
#[test]
fn a_claude_lines_marks_leave_its_identity_as_it_was() {
    let line = |agent: &'static str, payload: Value| Line {
        seq: 1,
        agent,
        event: "UserPromptSubmit".into(),
        session: "s".into(),
        ts: "2026-09-02T00:00:01.000Z".into(),
        payload,
        synthetic: false,
        native_session_known: true,
    };
    let hash = |l: &Line| {
        let mut h = Sha256::new();
        hash_line(&mut h, l).unwrap();
        format!("{:x}", h.finalize())
    };
    let before = json!({"prompt": "Fix it.", "session_id": "s", "transcript_path": "/t",
            "cwd": "/w", "hook_event_name": "UserPromptSubmit"});
    let with = |field: &str, value: Value| {
        json!({"prompt": "Fix it.", field: value, "session_id": "s", "transcript_path": "/t",
                "cwd": "/w", "hook_event_name": "UserPromptSubmit"})
    };
    for mark in [
        with(crate::capture::ENTRYPOINT, json!("cli")),
        with(crate::capture::AGENT_SENT, json!(true)),
    ] {
        assert_eq!(
            hash(&line("claude", mark)),
            hash(&line("claude", before.clone()))
        );
    }
    assert_ne!(
        hash(&line(
            "codex",
            with(crate::capture::AGENT_SENT, json!(true))
        )),
        hash(&line("codex", before.clone()))
    );
    // Nor does a call's start (#692), whatever the agent.
    for agent in ["claude", "codex"] {
        let started = with(crate::capture::STARTED, json!("2026-09-02T00:00:00.500Z"));
        assert_eq!(
            hash(&line(agent, started)),
            hash(&line(agent, before.clone()))
        );
    }
}

#[test]
fn a_parsed_transcript_replays_through_the_hook_path() {
    let home = std::env::temp_dir().join(format!("oboete-transcript-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut raw = crate::raw::open(&home).unwrap();
    let (v, _) = events(CLAUDE, "claude");
    for e in &v {
        crate::hook::record(
            &home,
            &mut raw,
            "claude",
            e["event"].as_str().unwrap(),
            &e["payload"],
            0,
            &Default::default(),
            &mut 0,
            &mut 0,
        )
        .unwrap();
    }
    let recs = raw.after(raw.device(), 0, 1000).unwrap();
    let bodies: Vec<String> = recs
        .into_iter()
        .filter_map(|r| match r.item {
            crate::raw::Item::Event(e) if e.session == "claude-basic" => Some(e.body),
            _ => None,
        })
        .collect();
    assert!(bodies.len() >= 8, "{bodies:?}");
    // The developer's answer reaches raw.db, where curation will read it.
    assert_eq!(bodies.iter().filter(|b| b.contains("A にする")).count(), 1);
    drop(raw);
    std::fs::remove_dir_all(&home).ok();
}
