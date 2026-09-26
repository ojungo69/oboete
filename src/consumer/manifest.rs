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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Manifest {
    home: PathBuf,
}

impl Manifest {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }
}

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
         -- A failure and the success of the same call (a seek, not a scan of later successes).
         CREATE INDEX IF NOT EXISTS manifest_facts_label
           ON manifest_facts(device, repo, branch, fact, label, seq);
         CREATE TABLE IF NOT EXISTS manifests(
           repo TEXT NOT NULL, branch TEXT NOT NULL, device TEXT NOT NULL,
           built_at INTEGER NOT NULL, text TEXT NOT NULL,
           -- The ruleset version its fields were gated with (Task 3b).
           ruleset TEXT NOT NULL DEFAULT '',
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
/// None while the saved text may show what raw now hides (D8): a tombstone the worker has not
/// applied yet, or a checkout still marked for a rebuild.
/// None too when it was built under other redaction rules than `ruleset` (the version now): its
/// fields were flattened and clipped, so a rule of another shape cannot be applied to it after.
/// The session it is shown to is left out of "Other active sessions" (after a compaction the
/// text was built while that session was running).
pub fn text(
    home: &Path,
    raw: &Raw,
    repo: &str,
    branch: &str,
    session: &str,
    ruleset: &str,
) -> Result<Option<String>> {
    let device = raw.device();
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
    let dirty = k
        .query_row(
            "SELECT 1 FROM manifest_dirty WHERE repo = ?1 AND branch = ?2 AND device = ?3",
            params![repo, branch, device],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let at = crate::knowledge::checkpoint::get(&k, "manifest", device)?;
    if dirty || !raw.tombstones_after(device, at)?.is_empty() {
        return Ok(None);
    }
    let text: Option<(String, String)> = k
        .query_row(
            "SELECT text, ruleset FROM manifests WHERE repo = ?1 AND branch = ?2 AND device = ?3",
            params![repo, branch, device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(text
        .filter(|(_, built)| built == ruleset)
        .map(|(t, _)| without_session(&t, session)))
}

/// `text` without its line for `session` under "Other active sessions" (and the heading, when
/// that was the only one).
fn without_session(text: &str, session: &str) -> String {
    let own = format!("- session {} on ", short(session));
    let mut out = String::with_capacity(text.len());
    let mut lines = text.lines().filter(|l| !l.starts_with(&own)).peekable();
    while let Some(l) = lines.next() {
        let empty = l == "## Other active sessions"
            && lines.peek().is_none_or(|next| next.starts_with("## "));
        if !empty {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

fn short(session: &str) -> String {
    session.chars().take(8).collect()
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
                    // Every checkout of its repo, as for a new record: a directive or a session
                    // shows on the repo's other branches too.
                    k.execute(
                        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                         SELECT repo, branch, device FROM manifests
                         WHERE device = ?1 AND repo IN
                           (SELECT repo FROM manifest_facts WHERE device = ?1 AND seq = ?2)",
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
        // A backlog is built once, at its end, not once per batch. Each field is gated with the
        // rules as they are now before it is flattened and clipped, and a text built under other
        // rules is built again. Settings that do not load stop capture too (doctor names them):
        // the bundled rules then.
        if recs.len() < BATCH {
            let rules = crate::capture::Settings::load(&self.home)
                .map(|s| s.rules)
                .unwrap_or_default();
            k.execute(
                "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                 SELECT repo, branch, device FROM manifests WHERE device = ?1 AND ruleset != ?2",
                params![device, rules.version()],
            )?;
            rebuild(raw, k, device, &rules)?;
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
        // An interrupted call returned nothing: neither a failure nor the fix of one.
        "tool" if body.get("interrupted").and_then(Value::as_bool) == Some(true) => {}
        "tool" => {
            // Every success, not only one after a failure of its call: a tombstone can change a
            // failure's key later, and the build pairs them then.
            let key = call_key(&body);
            add(if failed(&body) { "fail" } else { "fixed" }, &key)?;
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
    // Owner directives and other sessions are the repo's, not the branch's: every checkout of
    // the repo is built again, so its text depends on the records, not on the order they came.
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device) VALUES(?1, ?2, ?3)",
        params![repo, branch, device],
    )?;
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
         SELECT repo, branch, device FROM manifests WHERE repo = ?1 AND device = ?2",
        params![repo, device],
    )?;
    Ok(())
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// What a tool call ran, for "the same call succeeded later": a command (Codex's `cmd`) without
/// Claude's `description`, else the whole input.
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
        Ok(v) => match v.get("command").or_else(|| v.get("cmd")) {
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
/// (Grok's `exit_code`; Codex's header, `Exit code: N` or `Process exited with code N`, in the
/// lines before `Output:`, so a command's own output is never read as its exit).
fn failed(body: &Value) -> bool {
    if body.get("failed").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let output = str_at(body, "output");
    let code = match serde_json::from_str::<Value>(output) {
        Ok(v) => v.get("exit_code").and_then(Value::as_i64),
        Err(_) => output
            .lines()
            .take_while(|l| l.trim_end() != "Output:")
            .find_map(|l| {
                l.strip_prefix("Exit code: ")
                    .or_else(|| l.strip_prefix("Process exited with code "))
            })
            .and_then(|n| n.trim().parse().ok()),
    };
    code.is_some_and(|c| c != 0)
}

/// A todo list's items as (status, text): Claude Code's TodoWrite (`todos`) or Codex's
/// `update_plan` (`plan`).
fn todos(input: &Value) -> Option<Vec<(String, String)>> {
    let (items, text) = match (input.get("todos"), input.get("plan")) {
        (Some(Value::Array(a)), _) => (a, "content"),
        (_, Some(Value::Array(a))) => (a, "step"),
        _ => return None,
    };
    Some(
        items
            .iter()
            .map(|i| (str_at(i, "status").to_owned(), str_at(i, text).to_owned()))
            .collect(),
    )
}

/// The files a call names: the file path fields agents use (not a bare `path`: Glob, Grep and
/// LS take a directory there), and the file lines of Codex's `apply_patch` (its hook's
/// `command`, its transcript's `input`). A path under the call's cwd is shown relative to it.
fn paths(input: &Value, cwd: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = ["file_path", "notebook_path", "target_file"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let patch = format!("{}\n{}", str_at(input, "command"), str_at(input, "input"));
    for line in patch.lines() {
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
    // `/` on every platform, so a Windows path under a Windows cwd is relative too.
    let slashes = |s: &str| s.replace('\\', "/");
    let prefix = cwd.map(|c| format!("{}/", slashes(c).trim_end_matches('/')));
    let mut out: Vec<String> = out
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| slashes(&p))
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
/// `s` on one line, cut to `n` characters at a space: a token is shown whole or not at all, so
/// a rule added after the manifest was built still matches it at SessionStart's gate. A first
/// token longer than `n` is left out, not cut.
fn one_line(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(n) {
        Some((at, c)) => {
            let end = if c == ' ' {
                at
            } else {
                flat[..at].rfind(' ').unwrap_or(0)
            };
            format!("{}…", &flat[..end])
        }
        None => flat,
    }
}

/// Every dirty checkout of this device built again (or its row removed when no record is left).
fn rebuild(raw: &Raw, k: &Connection, device: &str, rules: &crate::redact::Rules) -> Result<()> {
    let dirty: Vec<(String, String)> = k
        .prepare("SELECT repo, branch FROM manifest_dirty WHERE device = ?1 ORDER BY repo, branch")?
        .query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (repo, branch) in dirty {
        match build(raw, k, device, &repo, &branch, rules)? {
            Some(text) => k.execute(
                "INSERT INTO manifests(repo, branch, device, built_at, text, ruleset)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(repo, branch, device) DO UPDATE SET built_at = excluded.built_at,
                   text = excluded.text, ruleset = excluded.ruleset",
                params![
                    repo,
                    branch,
                    device,
                    crate::db::now_ms(),
                    text,
                    rules.version()
                ],
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
    rules: &crate::redact::Rules,
) -> Result<Option<String>> {
    // Every stored field it shows is gated before it is flattened or clipped.
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
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
            one_line(&gate(str_at(&b, "tool")), CLIP),
            one_line(&gate(&what_ran(str_at(&b, "input"))), CLIP),
            one_line(&gate(str_at(&b, "output")), CLIP)
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
    // D13 matches directive lines one by one: taking one back leaves the others of its prompt.
    // ponytail: lines and `。`; English sentences on one line stay one line.
    let mut lines = Vec::new();
    for (seq, session, ts) in owner {
        if let Some(e) = event(raw, device, seq)? {
            let prompt = gate(str_at(&body(&e), "prompt"));
            for text in prompt.split(['\n', '。']).map(str::trim) {
                if manifest::is_owner_line(text) {
                    lines.push(Line {
                        date: crate::db::utc(ts)[..10].to_owned(),
                        session: session.clone(),
                        text: text.to_owned(),
                    });
                }
            }
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
            .map(|(status, text)| {
                let (status, text) = (gate(&status), gate(&text));
                format!("[{}] {}", one_line(&status, CLIP), one_line(&text, CLIP))
            })
            .collect();
    }
    if let Some(e) = last("prompt")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        p.last_prompt = Some(one_line(&gate(str_at(&body(&e), "prompt")), CLIP));
    }
    if let Some(e) = last("reply")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        p.last_reply = Some(one_line(&gate(str_at(&body(&e), "assistant")), CLIP));
    }
    p.files = k
        .prepare(
            "SELECT label FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'file' AND branch = ?3
             GROUP BY label ORDER BY MAX(seq) DESC LIMIT ?4",
        )?
        .query_map(params![device, repo, branch, FILES as i64], |r| r.get(0))?
        .map(|f| f.map(|f: String| gate(&f)))
        .collect::<rusqlite::Result<_>>()?;
    // Sessions on the repo with records in the 30 minutes before as-of that have not ended. One
    // aggregate, so SQLite takes the branch and time from the session's last record.
    let sessions: Vec<(String, String, i64, i64)> = k
        .prepare(
            "SELECT session, branch, ts, MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND ts >= ?3
             GROUP BY session ORDER BY ts DESC, session",
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
            let short = short(&gate(&session));
            let on = if on.is_empty() {
                "no branch".to_owned()
            } else {
                gate(&on)
            };
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
                // No blocking wait: a git stuck in the kernel may not exit at once. It is
                // reaped when the worker exits.
                child.kill().ok();
                let _ = child.try_wait();
                break false;
            }
        }
    };
    if !ok {
        // A killed git's pipe can stay open in what it started: the reader finishes on its own.
        return None;
    }
    reader.join().ok()
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
    fn a_rule_anchored_to_a_field_is_applied_to_each_field_shown() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let input = serde_json::json!({"command": "run it"});
        store
            .append(&tool(cwd.path(), 1, "acme-secret", input, "no", true))
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^acme-secret$' }]\n",
        )
        .unwrap();
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        assert!(
            shown.contains("failed with: no") && !shown.contains("acme"),
            "{shown}"
        );
    }

    /// The manifest SessionStart shows for checkout (r, main) under the rules the home has now.
    fn shown(home: &Path, store: &Raw) -> Option<String> {
        let rules = crate::capture::Settings::load(home).unwrap().rules;
        text(home, store, "r", "main", "none", rules.version()).unwrap()
    }

    #[test]
    fn session_start_reads_the_manifest_without_writing_knowledge_db() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = raw::open(home.path()).unwrap();
        assert_eq!(shown(home.path(), &store), None);
        assert!(!home.path().join("knowledge.db").exists());
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        assert_eq!(shown, manifest(home.path()).0);
    }

    #[test]
    fn a_tombstone_the_worker_has_not_applied_hides_the_saved_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let device = store.device().to_owned();
        assert!(shown(home.path(), &store).unwrap().contains("look at it"));
        let seq = store
            .after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .find(|r| matches!(&r.item, Item::Event(e) if e.body.contains("look at it")))
            .unwrap()
            .seq;
        store
            .append_tombstone(Target::Record {
                device: device.clone(),
                seq,
            })
            .unwrap();
        // No worker step yet: the saved text still has the prompt, so none is shown.
        assert_eq!(shown(home.path(), &store), None);
        worker::run_once(home.path()).unwrap();
        let shown = shown(home.path(), &store).unwrap();
        assert!(!shown.contains("look at it"), "{shown}");
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
        // Codex's newer header, and a command's own output that only looks like one.
        let newer =
            "Chunk ID: a1\nWall time: 1.2 seconds\nProcess exited with code 101\nOutput:\nx";
        assert!(failed(&b(newer, false)));
        assert!(!failed(&b(
            "Exit code: 0\nWall time: 0 seconds\nOutput:\nExit code: 1",
            false
        )));
        // Codex's transcript: the patch under `input`, the command under `cmd`.
        let custom = serde_json::json!({"input": "*** Begin Patch\n*** Update File: src/http.ts\n*** End Patch"});
        assert_eq!(paths(&custom, None), vec!["src/http.ts"]);
        assert_eq!(what_ran(r#"{"cmd": "rg fetchJson"}"#), "rg fetchJson");
        let windows = serde_json::json!({"file_path": "C:\\repo\\src\\a.rs"});
        assert_eq!(paths(&windows, Some("C:\\repo\\")), vec!["src/a.rs"]);
    }

    #[test]
    fn a_clipped_field_never_cuts_a_token() {
        assert_eq!(one_line("aaaa acme-123456 zzz", 10), "aaaa…");
        assert_eq!(one_line("aaaa acme-123456 zzz", 16), "aaaa acme-123456…");
        assert_eq!(one_line("acme-123456", 4), "…"); // longer than the clip: left out
        assert_eq!(one_line("a  b\nc", 10), "a b c");
        assert_eq!(one_line("日本語 の本文です", 5), "日本語…");
    }

    #[test]
    fn a_session_that_changed_branch_shows_the_last_one() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for (ts, branch, session) in [
            (60_000, "main", "s1"),
            (120_000, "feature", "s1"),
            (180_000, "main", "s2"),
        ] {
            store
                .append(&Event {
                    branch: Some(branch.into()),
                    ..ev(
                        "prompt",
                        session,
                        ts,
                        cwd.path(),
                        serde_json::json!({"prompt": "x"}),
                    )
                })
                .unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let main = manifest(home.path()).0;
        assert!(
            main.contains("session s1 on feature") && !main.contains("session s1 on main"),
            "{main}"
        );
    }

    #[test]
    fn the_session_it_is_shown_to_is_no_other_session() {
        let t = "## Other active sessions\n- session abcdefgh on main, last at 10:00 UTC\n\
                 ## As of\nnow\n";
        assert_eq!(without_session(t, "abcdefgh-1234"), "## As of\nnow\n");
        let two = t.replace(
            "## As of",
            "- session zzzzzzzz on dev, last at 10:01 UTC\n## As of",
        );
        assert_eq!(
            without_session(&two, "abcdefgh-1234"),
            "## Other active sessions\n- session zzzzzzzz on dev, last at 10:01 UTC\n## As of\nnow\n"
        );
        assert_eq!(without_session(t, "other"), t);
    }

    #[test]
    fn a_failure_a_new_rule_masks_is_paired_with_the_success_stored_masked() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let run = |c: &str| serde_json::json!({"command": format!("deploy {c}")});
        // The failure stored before the rule, its retry after it (masked at capture).
        store
            .append(&tool(cwd.path(), 1, "Bash", run("acme-123456"), "no", true))
            .unwrap();
        store
            .append(&tool(
                cwd.path(),
                2,
                "Bash",
                run("***********"),
                "ok",
                false,
            ))
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(shown(home.path(), &store).unwrap().contains("failed with"));
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(home.path()).unwrap(); // the rescan masks the failure: same call now
        let shown = shown(home.path(), &store).unwrap();
        assert!(!shown.contains("failed with"), "{shown}");
    }

    #[test]
    fn a_tombstoned_directive_leaves_every_branch_of_its_repo() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        store
            .append(&ev(
                "prompt",
                "s1",
                60_000,
                cwd.path(),
                serde_json::json!({"prompt": "look at main"}),
            ))
            .unwrap();
        let on_feature = store
            .append(&Event {
                branch: Some("feature".into()),
                ..ev(
                    "prompt",
                    "s2",
                    120_000,
                    cwd.path(),
                    serde_json::json!({"prompt": "always run zqxlint first"}),
                )
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(manifest(home.path()).0.contains("zqxlint"));
        let device = store.device().to_owned();
        store
            .append_tombstone(Target::Record {
                device,
                seq: on_feature,
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let main = manifest(home.path()).0;
        assert!(!main.contains("zqxlint"), "{main}");
    }

    #[test]
    fn an_interrupted_retry_fixes_nothing_and_a_retraction_takes_back_only_its_line() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let build = serde_json::json!({"command": "cargo build"}).to_string();
        let mut store = raw::open(home.path()).unwrap();
        for e in [
            ev(
                "prompt",
                "s1",
                60_000,
                cwd.path(),
                serde_json::json!({"prompt": "from now on run clippy\nalways write tests first"}),
            ),
            tool(
                cwd.path(),
                120_000,
                "Bash",
                serde_json::json!({"command": "cargo build"}),
                "E0308",
                true,
            ),
            ev(
                "tool",
                "s1",
                180_000,
                cwd.path(),
                serde_json::json!({"tool": "Bash", "input": build, "output": "", "failed": false,
                    "interrupted": true}),
            ),
        ] {
            store.append(&e).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let before = manifest(home.path()).0;
        assert!(
            before.contains("cargo build") && before.contains("\"from now on run clippy\""),
            "{before}"
        );
        // Taken back on another branch: the directives are the repo's, so main is built again.
        store
            .append(&Event {
                branch: Some("feature".into()),
                ..ev(
                    "prompt",
                    "s2",
                    240_000,
                    cwd.path(),
                    serde_json::json!({"prompt": "cancel that clippy rule"}),
                )
            })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let after = manifest(home.path()).0;
        assert!(
            after.contains("\"always write tests first\"")
                && !after.contains("\"from now on run clippy\""),
            "{after}"
        );
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            std::fs::remove_file(home.path().join(f)).ok();
        }
        worker::run_once(home.path()).unwrap();
        assert_eq!(manifest(home.path()).0, after); // the same records, the same bytes
    }
}
