//! Milestone 2 Task 9 (spec 4.9, plan D12): the manifest of each checkout (repo x branch x
//! device), rebuilt from raw when the checkout gets records. Facts point at records by
//! (device, seq); a build reads the few bodies it shows back through `Raw::after`, so what a
//! tombstone hides there (Task 7) is hidden here too. File paths are the one copy (labels).

use crate::manifest::{self, Line, Parts};
use crate::raw::{Event, Item, Raw, Target};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Manifest;

/// Records per step, as the FTS consumer.
const BATCH: usize = 500;
/// ponytail: one size for every agent until spec 1.5's per-agent injection sizes; under Cursor's
/// 9,500-unit cut with room for the recording-failure line.
pub const CAP: usize = 6_000;
/// How much of one prompt, reply, command or output a manifest shows.
const CLIP: usize = 400;
const FILES: usize = 10;
const DIRECTIVES: usize = 10;
const TODOS: usize = 20;
const SESSIONS: usize = 5;
/// ponytail: the owner lines a build reads (a negation older than these no longer matters); a
/// claims table replaces the scan in milestone 3.
const OWNER_LINES: i64 = 500;
/// Spec 4.9: sessions with records this close to the checkout's last one are active.
const ACTIVE_MS: i64 = 30 * 60 * 1000;
/// D9's `git status` in the worker, given up after this.
const GIT_TIMEOUT: Duration = Duration::from_millis(500);

fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS manifest_facts(
           device TEXT NOT NULL, seq INTEGER NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           session TEXT NOT NULL, ts INTEGER NOT NULL, fact TEXT NOT NULL,
           label TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS manifest_facts_seq
           ON manifest_facts(device, repo, branch, fact, seq);
         CREATE INDEX IF NOT EXISTS manifest_facts_ts ON manifest_facts(device, repo, fact, ts);
         CREATE TABLE IF NOT EXISTS manifests(
           repo TEXT NOT NULL, branch TEXT NOT NULL, device TEXT NOT NULL,
           built_at INTEGER NOT NULL, text TEXT NOT NULL,
           PRIMARY KEY (repo, branch, device)
         );
         -- Checkouts whose facts changed since their manifest was built.
         CREATE TABLE IF NOT EXISTS manifest_dirty(
           repo TEXT NOT NULL, branch TEXT NOT NULL, device TEXT NOT NULL,
           PRIMARY KEY (repo, branch, device)
         );",
    )?;
    Ok(())
}

/// The manifest SessionStart shows for this checkout, if its records built one. Read-only: a
/// hook never writes knowledge.db, and none is made when the worker has not run yet.
#[allow(dead_code)] // SessionStart reads it once #104 and #108 are in (this task)
pub fn text(home: &Path, repo: &str, branch: &str, device: &str) -> Result<Option<String>> {
    let path = home.join("knowledge.db");
    if !path.exists() {
        return Ok(None);
    }
    let k = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let built = k
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'manifests'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !built {
        return Ok(None);
    }
    Ok(k.query_row(
        "SELECT text FROM manifests WHERE repo = ?1 AND branch = ?2 AND device = ?3",
        params![repo, branch, device],
        |r| r.get(0),
    )
    .optional()?)
}

impl Consumer for Manifest {
    fn name(&self) -> &'static str {
        "manifest"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> {
        schema(k)?;
        let device = raw.device();
        let recs = raw.after(device, after, BATCH)?;
        for r in &recs {
            match &r.item {
                Item::Event(e) => facts(k, device, r.seq, e)?,
                Item::Tombstone(
                    Target::Record { device: d, seq } | Target::Range { device: d, seq, .. },
                ) => {
                    // The target's facts again, from what raw returns now (masked, or none
                    // when it is removed), in a build of its checkout.
                    k.execute(
                        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                         SELECT DISTINCT repo, branch, device FROM manifest_facts
                         WHERE device = ?1 AND seq = ?2",
                        params![d, seq],
                    )?;
                    k.execute(
                        "DELETE FROM manifest_facts WHERE device = ?1 AND seq = ?2",
                        params![d, seq],
                    )?;
                    if let Some(e) = event(raw, d, *seq)? {
                        facts(k, d, *seq, &e)?;
                    }
                }
                Item::Removed => {}
            }
        }
        // A backlog is built once, at its end, not once per batch.
        if recs.len() < BATCH {
            rebuild(raw, k, device)?;
        }
        Ok(recs.last().map_or(after, |r| r.seq))
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM manifest_facts WHERE device = ?1 AND seq > ?2",
            params![device, to],
        )?;
        // Every checkout of the device is built again on the step that follows the rewind.
        k.execute(
            "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
             SELECT repo, branch, device FROM manifests WHERE device = ?1",
            [device],
        )?;
        Ok(())
    }
}

/// The facts of one event, and its checkout marked for a rebuild. Events without a repo label
/// belong to no checkout.
fn facts(k: &Connection, device: &str, seq: i64, e: &Event) -> Result<()> {
    let Some(repo) = e.repo.as_deref() else {
        return Ok(());
    };
    let branch = e.branch.as_deref().unwrap_or("");
    let add = |fact: &str, label: &str| -> Result<()> {
        k.execute(
            "INSERT INTO manifest_facts(device, seq, repo, branch, session, ts, fact, label)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![device, seq, repo, branch, e.session, e.ts, fact, label],
        )?;
        Ok(())
    };
    add("event", "")?;
    let body: Value = serde_json::from_str(&e.body).unwrap_or(Value::Null);
    match e.kind.as_str() {
        "prompt" => {
            add("prompt", "")?;
            if manifest::is_owner_line(str_at(&body, "prompt")) {
                add("owner", "")?;
            }
        }
        "reply" => add("reply", "")?,
        "end" => add("end", "")?,
        "tool" => {
            let key = call_key(&body);
            if failed(&body) {
                add("fail", &key)?;
            } else if k
                .query_row(
                    "SELECT 1 FROM manifest_facts WHERE device = ?1 AND repo = ?2 AND fact = 'fail'
                       AND branch = ?3 AND label = ?4 AND seq < ?5 LIMIT 1",
                    params![device, repo, branch, key, seq],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                add("fixed", &key)?;
            }
            let input: Value = serde_json::from_str(str_at(&body, "input")).unwrap_or(Value::Null);
            if todos(&input).is_some() {
                add("todo", "")?;
            }
            for path in paths(&input, e.cwd.as_deref()) {
                add("file", &path)?;
            }
        }
        _ => {}
    }
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device) VALUES(?1, ?2, ?3)",
        params![repo, branch, device],
    )?;
    Ok(())
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// What a tool call ran, for "the same call succeeded later": a command without Claude's
/// `description`, else the whole input.
fn call_key(body: &Value) -> String {
    use sha2::{Digest, Sha256};
    let text = format!(
        "{}\n{}",
        str_at(body, "tool"),
        what_ran(str_at(body, "input"))
    );
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn what_ran(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(v) => match v.get("command") {
            Some(Value::String(c)) => c.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => input.to_owned(),
        },
        Err(_) => input.to_owned(),
    }
}

/// A failed call: the hook said so (PostToolUseFailure), or its output carries a non-zero exit
/// (Grok's `exit_code`, Codex's `Exit code: N` text).
fn failed(body: &Value) -> bool {
    if body.get("failed").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let output = str_at(body, "output");
    let code = match serde_json::from_str::<Value>(output) {
        Ok(v) => v.get("exit_code").and_then(Value::as_i64),
        Err(_) => output
            .strip_prefix("Exit code: ")
            .and_then(|r| r.lines().next())
            .and_then(|n| n.trim().parse().ok()),
    };
    code.is_some_and(|c| c != 0)
}

/// A todo list's items as `[status] text`: Claude Code's TodoWrite (`todos`) or Codex's
/// `update_plan` (`plan`).
fn todos(input: &Value) -> Option<Vec<String>> {
    let (items, text) = match (input.get("todos"), input.get("plan")) {
        (Some(Value::Array(a)), _) => (a, "content"),
        (_, Some(Value::Array(a))) => (a, "step"),
        _ => return None,
    };
    Some(
        items
            .iter()
            .map(|i| {
                format!(
                    "[{}] {}",
                    str_at(i, "status"),
                    one_line(str_at(i, text), CLIP)
                )
            })
            .collect(),
    )
}

/// The files a call names: the file path fields agents use (not a bare `path`: Glob, Grep and
/// LS take a directory there), and the file lines of Codex's `apply_patch`. A path under the
/// call's cwd is shown relative to it.
fn paths(input: &Value, cwd: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = ["file_path", "notebook_path", "target_file"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    for line in str_at(input, "command").lines() {
        for tag in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(p) = line.strip_prefix(tag) {
                out.push(p.trim().to_owned());
            }
        }
    }
    let prefix = cwd.map(|c| format!("{}/", c.trim_end_matches('/')));
    let mut out: Vec<String> = out
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(
            |p| match prefix.as_deref().and_then(|c| p.strip_prefix(c)) {
                Some(rel) => rel.to_owned(),
                None => p,
            },
        )
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `s` on one line, cut to `n` characters.
fn one_line(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// Every dirty checkout of this device built again (or its row removed when no record is left).
fn rebuild(raw: &Raw, k: &Connection, device: &str) -> Result<()> {
    let dirty: Vec<(String, String)> = k
        .prepare("SELECT repo, branch FROM manifest_dirty WHERE device = ?1 ORDER BY repo, branch")?
        .query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (repo, branch) in dirty {
        match build(raw, k, device, &repo, &branch)? {
            Some(text) => k.execute(
                "INSERT INTO manifests(repo, branch, device, built_at, text) VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(repo, branch, device) DO UPDATE SET built_at = excluded.built_at,
                   text = excluded.text",
                params![repo, branch, device, crate::db::now_ms(), text],
            )?,
            None => k.execute(
                "DELETE FROM manifests WHERE repo = ?1 AND branch = ?2 AND device = ?3",
                params![repo, branch, device],
            )?,
        };
    }
    k.execute("DELETE FROM manifest_dirty WHERE device = ?1", [device])?;
    Ok(())
}

/// One record's event, read back through raw (`None` once a tombstone removed it).
fn event(raw: &Raw, device: &str, seq: i64) -> Result<Option<Event>> {
    Ok(raw
        .after(device, seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == seq)
        .and_then(|r| match r.item {
            Item::Event(e) => Some(*e),
            _ => None,
        }))
}

fn body(e: &Event) -> Value {
    serde_json::from_str(&e.body).unwrap_or(Value::Null)
}

/// The manifest's text for one checkout from its facts: every part but the git state comes from
/// records, so the same records give the same text (spec 4.9).
fn build(
    raw: &Raw,
    k: &Connection,
    device: &str,
    repo: &str,
    branch: &str,
) -> Result<Option<String>> {
    let last = |fact: &str| -> Result<Option<i64>> {
        Ok(k.query_row(
            "SELECT MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = ?3 AND branch = ?4",
            params![device, repo, fact, branch],
            |r| r.get(0),
        )?)
    };
    let Some((last_seq, as_of, uncurated)) = k
        .query_row(
            "SELECT MAX(seq), MAX(ts), COUNT(*) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND branch = ?3",
            params![device, repo, branch],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, i64>(1).unwrap_or(0),
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .map(|(s, t, n)| s.map(|s| (s, t, n)))?
    else {
        return Ok(None);
    };
    let mut p = Parts {
        as_of: crate::db::utc(as_of),
        uncurated: u64::try_from(uncurated).unwrap_or(0),
        ..Parts::default()
    };
    if let Some(e) = event(raw, device, last_seq)?
        && let Some(cwd) = e.cwd.as_deref()
    {
        p.git = risky(Path::new(cwd), branch);
    }
    // The last failure not followed by a success of the same call.
    let failing: Option<i64> = k
        .query_row(
            "SELECT f.seq FROM manifest_facts f
             WHERE f.device = ?1 AND f.repo = ?2 AND f.fact = 'fail' AND f.branch = ?3
               AND NOT EXISTS (SELECT 1 FROM manifest_facts x
                 WHERE x.device = f.device AND x.repo = f.repo AND x.fact = 'fixed'
                   AND x.branch = f.branch AND x.label = f.label AND x.seq > f.seq)
             ORDER BY f.seq DESC LIMIT 1",
            params![device, repo, branch],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(e) = failing
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        let b = body(&e);
        p.failing = Some(format!(
            "{}: {}\n  failed with: {}",
            str_at(&b, "tool"),
            one_line(&what_ran(str_at(&b, "input")), CLIP),
            one_line(str_at(&b, "output"), CLIP)
        ));
    }
    let mut owner: Vec<(i64, String, i64)> = k
        .prepare(
            "SELECT seq, session, ts FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'owner' ORDER BY seq DESC LIMIT ?3",
        )?
        .query_map(params![device, repo, OWNER_LINES], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    owner.reverse();
    let mut lines = Vec::new();
    for (seq, session, ts) in owner {
        if let Some(e) = event(raw, device, seq)? {
            lines.push(Line {
                date: crate::db::utc(ts)[..10].to_owned(),
                session,
                text: str_at(&body(&e), "prompt").to_owned(),
            });
        }
    }
    // A line said again is shown once, at its latest date.
    let mut kept: Vec<Line> = Vec::new();
    for l in manifest::directives(&lines) {
        let text = one_line(&l.text, CLIP);
        kept.retain(|k| k.text != text);
        kept.push(Line { text, ..l });
    }
    p.directives = kept.split_off(kept.len().saturating_sub(DIRECTIVES));
    if let Some(e) = last("todo")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        let input: Value = serde_json::from_str(str_at(&body(&e), "input")).unwrap_or(Value::Null);
        p.todo = todos(&input)
            .unwrap_or_default()
            .into_iter()
            .take(TODOS)
            .collect();
    }
    if let Some(e) = last("prompt")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        p.last_prompt = Some(one_line(str_at(&body(&e), "prompt"), CLIP));
    }
    if let Some(e) = last("reply")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        p.last_reply = Some(one_line(str_at(&body(&e), "assistant"), CLIP));
    }
    p.files = k
        .prepare(
            "SELECT label FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'file' AND branch = ?3
             GROUP BY label ORDER BY MAX(seq) DESC LIMIT ?4",
        )?
        .query_map(params![device, repo, branch, FILES as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    // Sessions on the repo with records in the 30 minutes before as-of that have not ended.
    let sessions: Vec<(String, String, i64, i64)> = k
        .prepare(
            "SELECT session, branch, MAX(ts), MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND ts >= ?3
             GROUP BY session ORDER BY MAX(ts) DESC, session",
        )?
        .query_map(params![device, repo, as_of - ACTIVE_MS], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (session, on, ts, seq) in sessions {
        let ended = k
            .query_row(
                "SELECT 1 FROM manifest_facts WHERE device = ?1 AND repo = ?2 AND fact = 'end' AND seq = ?3",
                params![device, repo, seq],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !ended && p.others.len() < SESSIONS {
            let short: String = session.chars().take(8).collect();
            let on = if on.is_empty() { "no branch" } else { &on };
            p.others.push(format!(
                "session {short} on {on}, last at {}",
                &crate::db::utc(ts)[11..]
            ));
        }
    }
    Ok(Some(manifest::render(&p, CAP)))
}

/// D9's risky git state of the checkout at `cwd`, while it is still on `branch` (or detached, as
/// during a rebase): nothing when `cwd` is gone, is no repository, or now has another branch.
fn risky(cwd: &Path, branch: &str) -> Vec<String> {
    let g = crate::capture::git(cwd);
    let Some(gitdir) = g.gitdir.as_deref().map(Path::new) else {
        return Vec::new();
    };
    if g.branch.as_deref().is_some_and(|b| b != branch) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (path, what) in [
        ("rebase-merge", "a rebase is in progress"),
        ("rebase-apply", "a rebase is in progress"),
        ("MERGE_HEAD", "a merge is in progress"),
        ("CHERRY_PICK_HEAD", "a cherry-pick is in progress"),
        ("REVERT_HEAD", "a revert is in progress"),
    ] {
        if gitdir.join(path).exists() && !out.iter().any(|o| o == what) {
            out.push(what.to_owned());
        }
    }
    if g.branch.is_none()
        && let Some(head) = g.head
    {
        let at: String = head.chars().take(12).collect();
        out.push(format!("detached HEAD at {at}"));
    }
    if let Some(status) = status(cwd) {
        out.extend(porcelain(&status));
    }
    out
}

/// The counts in `git status --porcelain=v2 --branch` output that a new session should know.
fn porcelain(status: &str) -> Vec<String> {
    let (mut changed, mut conflicts, mut untracked, mut ahead) = (0, 0, 0, 0);
    for line in status.lines() {
        match line.split_once(' ') {
            Some(("1" | "2", _)) => changed += 1,
            Some(("u", _)) => conflicts += 1,
            Some(("?", _)) => untracked += 1,
            Some(("#", rest)) => {
                if let Some(ab) = rest.strip_prefix("branch.ab +") {
                    ahead = ab
                        .split(' ')
                        .next()
                        .and_then(|a| a.parse().ok())
                        .unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    [
        (conflicts, "file(s) with merge conflicts"),
        (changed, "uncommitted change(s)"),
        (untracked, "untracked file(s)"),
        (ahead, "commit(s) not pushed"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect()
}

/// `git status --porcelain=v2 --branch` in `cwd`, given up after `GIT_TIMEOUT`. No optional
/// locks (the index is never written) and no fsmonitor (a repository's config cannot make the
/// worker run a command).
fn status(cwd: &Path) -> Option<String> {
    let mut child = Command::new("git")
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read while git runs: a full pipe would otherwise stop it before it exits.
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        (&mut stdout).take(1 << 20).read_to_string(&mut s).ok();
        std::io::copy(&mut stdout, &mut std::io::sink()).ok();
        s
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                child.kill().ok();
                child.wait().ok();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    ok.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{knowledge, raw, worker};

    fn ev(kind: &str, session: &str, ts: i64, cwd: &Path, body: Value) -> Event {
        Event {
            session: session.into(),
            kind: kind.into(),
            ts,
            repo: Some("r".into()),
            branch: Some("main".into()),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            body: body.to_string(),
            ..raw::test_event("")
        }
    }

    fn tool(cwd: &Path, ts: i64, tool: &str, input: Value, output: &str, failed: bool) -> Event {
        let body = serde_json::json!({"tool": tool, "input": input.to_string(), "output": output,
            "failed": failed});
        ev("tool", "s1", ts, cwd, body)
    }

    fn manifest(home: &Path) -> (String, i64) {
        let k = knowledge::open(home).unwrap();
        let device = raw::open(home).unwrap().device().to_owned();
        k.query_row(
            "SELECT text, built_at FROM manifests WHERE repo = 'r' AND branch = 'main' AND device = ?1",
            [device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// A session's records: a directive, a failing and a fixed command, a todo list, an edit.
    fn session(home: &Path, cwd: &Path) {
        let mut raw = raw::open(home).unwrap();
        let min = 60_000;
        for e in [
            ev(
                "prompt",
                "s1",
                min,
                cwd,
                serde_json::json!({"prompt": "from now on run the linter first"}),
            ),
            tool(
                cwd,
                2 * min,
                "Bash",
                serde_json::json!({"command": "cargo lint"}),
                "no such command",
                true,
            ),
            tool(
                cwd,
                3 * min,
                "Bash",
                serde_json::json!({"command": "cargo test"}),
                "Exit code: 101\nfailed",
                false,
            ),
            tool(
                cwd,
                4 * min,
                "Bash",
                serde_json::json!({"command": "cargo lint", "description": "again"}),
                "ok",
                false,
            ),
            tool(
                cwd,
                5 * min,
                "TodoWrite",
                serde_json::json!({"todos": [{"content": "fix the test", "status": "in_progress"}]}),
                "",
                false,
            ),
            tool(
                cwd,
                6 * min,
                "Edit",
                serde_json::json!({"file_path": format!("{}/src/lib.rs", cwd.display())}),
                "",
                false,
            ),
            ev(
                "reply",
                "s1",
                7 * min,
                cwd,
                serde_json::json!({"assistant": "The test still fails."}),
            ),
            ev(
                "prompt",
                "s2",
                8 * min,
                cwd,
                serde_json::json!({"prompt": "from now on run the linter first"}),
            ),
            ev(
                "prompt",
                "s2",
                9 * min,
                cwd,
                serde_json::json!({"prompt": "look at it"}),
            ),
        ] {
            raw.append(&e).unwrap();
        }
    }

    #[test]
    fn the_manifest_is_the_same_bytes_for_the_same_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap(); // no repository: the git state stays empty
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let (first, built) = manifest(home.path());
        // Said twice, shown once.
        assert_eq!(
            first
                .matches("\"from now on run the linter first\"")
                .count(),
            1,
            "{first}"
        );
        assert!(
            first.contains("cargo test") && !first.contains("cargo lint:"),
            "{first}"
        );
        assert!(first.contains("[in_progress] fix the test"), "{first}");
        assert!(first.contains("- src/lib.rs"), "{first}");
        assert!(first.contains("prompt: look at it"), "{first}");
        assert!(first.contains("9 record(s) not yet curated"), "{first}");
        worker::run_once(home.path()).unwrap(); // nothing new: nothing built
        assert_eq!(manifest(home.path()), (first.clone(), built));
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            std::fs::remove_file(home.path().join(f)).ok();
        }
        worker::run_once(home.path()).unwrap(); // built again from raw
        assert_eq!(manifest(home.path()).0, first);
    }

    #[test]
    fn session_start_reads_the_manifest_without_writing_knowledge_db() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(text(home.path(), "r", "main", "d").unwrap(), None);
        assert!(!home.path().join("knowledge.db").exists());
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let device = raw::open(home.path()).unwrap().device().to_owned();
        let shown = text(home.path(), "r", "main", &device).unwrap().unwrap();
        assert_eq!(shown, manifest(home.path()).0);
    }

    #[test]
    fn a_rewind_takes_the_lost_records_out_of_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        assert!(manifest(home.path()).0.contains("look at it"));
        let c = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > 8", []).unwrap(); // lost commits (MUST-M14)
        drop(c);
        worker::run_once(home.path()).unwrap();
        let text = manifest(home.path()).0;
        assert!(!text.contains("look at it"), "{text}");
        assert!(text.contains("prompt: from now on"), "{text}");
    }

    #[test]
    fn the_counts_that_matter_come_from_git_status() {
        let status = "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... x\n1 M. N... y\nu UU N... z\n? new\n";
        assert_eq!(
            porcelain(status),
            vec![
                "1 file(s) with merge conflicts",
                "2 uncommitted change(s)",
                "1 untracked file(s)",
                "2 commit(s) not pushed"
            ]
        );
        assert!(porcelain("# branch.ab +0 -3\n").is_empty());
    }

    #[test]
    fn a_merge_in_progress_and_changes_are_risky_state() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
        };
        if !git(&["init", "-q", "-b", "main"]).status.success() {
            return; // no git here
        }
        std::fs::write(dir.path().join("a"), "x").unwrap();
        std::fs::write(dir.path().join(".git/MERGE_HEAD"), "0\n").unwrap();
        let state = risky(dir.path(), "main");
        assert!(
            state.contains(&"a merge is in progress".to_owned()),
            "{state:?}"
        );
        assert!(
            state.contains(&"1 untracked file(s)".to_owned()),
            "{state:?}"
        );
        assert!(risky(dir.path(), "other").is_empty()); // the checkout is on another branch now
    }

    #[test]
    fn a_failure_is_read_from_each_agent_shape() {
        let b =
            |output: &str, failed: bool| serde_json::json!({"output": output, "failed": failed});
        assert!(failed(&b("x", true)));
        assert!(failed(&b("Exit code: 1\nWall time: 0 seconds", false)));
        assert!(!failed(&b("Exit code: 0\nWall time: 0 seconds", false)));
        assert!(failed(&b(r#"{"exit_code": 2}"#, false)));
        assert!(!failed(&b(r#"{"stdout": "ok"}"#, false)));
        let patch = serde_json::json!({"command": "*** Begin Patch\n*** Update File: src/a.rs\n*** Add File: b.md\n"});
        assert_eq!(paths(&patch, None), vec!["b.md", "src/a.rs"]);
    }
}
