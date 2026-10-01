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
/// Delivered claims SessionStart shows with their bodies, and at most this many more one line each
/// (spec 4.4: about 10, then an index).
const BODIES: usize = 10;
const INDEX: usize = 20;
/// The newest delivered claims SessionStart chooses from.
// ponytail: an older claim reaches SessionStart only through search; a larger pool if M22's
// scale check leaves room.
const POOL: usize = 200;
/// How much of a claim an index line shows.
const BRIEF: usize = 80;
/// The index's first line: where the rest is (spec 4.4).
const TOOLS: &str = "`search` finds more of what is remembered here, `get` shows one in full by \
                     its id, and `timeline` lists the earlier sessions.";
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
         );
         -- Devices whose facts a rewind dropped: the next step reads raw from seq 1 again.
         CREATE TABLE IF NOT EXISTS manifest_rebuild(device TEXT PRIMARY KEY);",
    )?;
    Ok(())
}

/// What SessionStart shows for this checkout (spec 4.4, D4 of milestone 4's plan): the manifest
/// its records built, with the delivered claims and the digest read from knowledge.db's indexes
/// (`with_delivered`). Read-only: a hook never writes knowledge.db, and none is made when the
/// worker has not run yet. The session it is shown to is left out of "Other active sessions"
/// (after a compaction the text was built while that session was running).
pub fn text(
    home: &Path,
    raw: &Raw,
    repo: &str,
    branch: &str,
    session: &str,
    ruleset: &str,
) -> Result<Option<String>> {
    let path = home.join("knowledge.db");
    if !path.exists() {
        return Ok(None);
    }
    let k = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let manifest = stored(&k, raw, repo, branch, ruleset)?.map(|t| without_session(&t, session));
    with_delivered(&k, raw, repo, manifest)
}

/// Whether knowledge.db has the table or view `name` (`kind`): absent until the worker has run
/// this version's schema.
pub(crate) fn exists(k: &Connection, kind: &str, name: &str) -> Result<bool> {
    Ok(k.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2",
        [kind, name],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

/// The manifest the worker keeps for the checkout, if its records built one. None while the saved
/// text may show what raw now hides (D8): a tombstone the worker has not applied yet, or a
/// checkout still marked for a rebuild. None too when it was built under other redaction rules
/// than `ruleset` (the version now): its fields were flattened and clipped, so a rule of another
/// shape cannot be applied to it after.
fn stored(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    branch: &str,
    ruleset: &str,
) -> Result<Option<String>> {
    let device = raw.device();
    if !exists(k, "table", "manifests")? {
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
    let at = crate::knowledge::checkpoint::get(k, "manifest", device)?;
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
    Ok(text.filter(|(_, built)| built == ruleset).map(|(t, _)| t))
}

/// Spec 4.4's SessionStart around the checkout's `manifest`: the global preferences first; the
/// manifest, with `repo`'s delivered decisions, preferences, open items and lessons (spec 3.4) in
/// its third place (spec 4.9's order) and the digest after them while it is fresh; last, the rest
/// of the delivered claims one line each, after the tools that find more, so that the hook's cut
/// drops them first. About 10 claims have bodies, chosen by the words they share with the
/// manifest's files and last prompt, then by recency, each earlier claim with the later claim that
/// ended it (`claims::units`), and listed newest first. A checkout with no manifest to show (a new
/// branch, a dirty manifest, a tombstone not yet applied) still gets them (D4).
/// Read when the text is (D3): claims come from curation, which runs while the owner is idle and
/// appends no record this consumer steps on, so a section built with the text would miss the last
/// session's decisions; and what the worker has not applied yet (`claims::Pending`) is left out.
/// Unchanged where curation never ran (no claims view).
fn with_delivered(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    manifest: Option<String>,
) -> Result<Option<String>> {
    use crate::claims::{self, Claim};
    if !exists(k, "view", "active")? {
        return Ok(manifest);
    }
    let pending = claims::Pending::read(raw, k)?;
    let hidden = |uid: &str| pending.touches(k, uid);
    let mut prefs = Vec::new();
    for c in claims::global(k)? {
        if !hidden(&c.uid)? {
            prefs.push(c);
        }
    }
    let words = manifest.as_deref().map(terms).unwrap_or_default();
    let mut ranked = claims::decisions(k, repo, POOL)?;
    // Stable: among claims that share as many words, the newest first.
    ranked.sort_by_cached_key(|c| std::cmp::Reverse(shared(&c.body, &words)));
    let (mut bodies, rest) = claims::place(claims::units(k, &ranked, hidden)?, BODIES);
    let (mut index, _) = claims::place(rest, INDEX);
    claims::newest_first(&mut bodies);
    claims::newest_first(&mut index);
    let date = |c: &Claim| crate::db::utc(c.valid_from)[..10].to_owned();
    // An earlier claim names the claim above it that ended it.
    let full = |c: &Claim, unit: &[Claim]| {
        let ended = c
            .later
            .as_ref()
            .and_then(|l| unit.iter().find(|u| u.uid == *l))
            .map(|l| format!(", superseded by the {} {} above", date(l), l.kind))
            .unwrap_or_default();
        format!(
            "- {} {}{ended}: {}\n",
            date(c),
            c.kind,
            one_line(&c.body, CLIP)
        )
    };
    let id = |uid: &str| uid.chars().take(12).collect::<String>();
    let brief = |c: &Claim| {
        let ended = c
            .later
            .as_ref()
            .map(|l| format!(", superseded by {}", id(l)))
            .unwrap_or_default();
        format!(
            "- {} {} {}{ended}: {}\n",
            id(&c.uid),
            date(c),
            c.kind,
            one_line(&c.body, BRIEF)
        )
    };
    let section = |title: &str, lines: String| {
        if lines.is_empty() {
            String::new()
        } else {
            format!("## {title}\n{lines}")
        }
    };
    let first = section(
        "Global preferences",
        prefs.iter().map(|c| full(c, &[])).collect(),
    );
    let mut middle = section(
        "Decisions and open items",
        bodies
            .iter()
            .flat_map(|u| u.iter().map(|c| full(c, u)))
            .collect(),
    );
    if let Some(digest) = crate::digest::fresh(k, repo, hidden)? {
        let lines: String = digest
            .iter()
            .map(|l| format!("- {}\n", one_line(l, CLIP)))
            .collect();
        middle.push_str(&section("Digest of the last session", lines));
    }
    let last = if bodies.is_empty() && index.is_empty() {
        String::new()
    } else {
        let lines: String = index.iter().flatten().map(brief).collect();
        format!("## More from memory\n{TOOLS}\n{lines}")
    };
    let manifest = manifest.unwrap_or_default();
    let at = AFTER_DECISIONS
        .iter()
        .filter_map(|h| {
            let h = format!("## {h}\n");
            manifest
                .match_indices(&h)
                .map(|(i, _)| i)
                .find(|&i| i == 0 || manifest[..i].ends_with('\n'))
        })
        .min()
        .unwrap_or(manifest.len());
    let text = format!(
        "{first}{}{middle}{}{last}",
        &manifest[..at],
        &manifest[at..]
    );
    Ok((!text.is_empty()).then_some(text))
}

/// The words a claim may share with the checkout's manifest (D4): the names of the files touched
/// and the last prompt's words of four letters or more, lowercased.
// ponytail: words are ASCII letters, digits and `_`, so a prompt in Japanese gives only the ASCII
// words in it; bigrams for CJK text if SessionStart's choice needs them.
fn terms(manifest: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut section = "";
    for line in manifest.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h;
            continue;
        }
        let Some(item) = line.strip_prefix("- ") else {
            continue;
        };
        let words: Vec<&str> = match section {
            "Files touched" => item.rsplit(['/', '\\']).next().into_iter().collect(),
            "Last exchange" => item
                .strip_prefix("prompt: ")
                .map(|p| {
                    p.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .collect()
                })
                .unwrap_or_default(),
            _ => continue,
        };
        for w in words {
            let w = w.to_lowercase();
            if w.chars().count() >= 4 && !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

/// How many of `terms` `body` holds.
fn shared(body: &str, terms: &[String]) -> usize {
    let body = body.to_lowercase();
    terms.iter().filter(|t| body.contains(t.as_str())).count()
}

/// Spec 4.9's sections after the current decisions, in the manifest's words.
const AFTER_DECISIONS: [&str; 6] = [
    "Owner's directives",
    "Todo list",
    "Last exchange",
    "Files touched",
    "As of",
    "Other active sessions",
];

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

    fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let device = raw.device();
        // After a rewind, from seq 1: its checkpoint moves back, which the worker takes.
        let from = if k.execute("DELETE FROM manifest_rebuild WHERE device = ?1", [device])? > 0 {
            0
        } else {
            after
        };
        let recs = raw.after(device, from, BATCH)?;
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
        Ok(recs.last().map_or(from, |r| r.seq))
    }

    /// The lost commits may hold tombstones that changed or removed older records' facts, so
    /// the device's facts and texts are dropped and built again from seq 1, as a rebuild from an
    /// empty knowledge.db would.
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        for table in ["manifest_facts", "manifests", "manifest_dirty"] {
            k.execute(&format!("DELETE FROM {table} WHERE device = ?1"), [device])?;
        }
        k.execute(
            "INSERT OR IGNORE INTO manifest_rebuild(device) VALUES(?1)",
            [device],
        )?;
        Ok(())
    }
}

/// The facts of one event, and its checkout marked for a rebuild. Events without a repo label
/// belong to no checkout.
fn facts(k: &Connection, device: &str, seq: i64, e: &Event) -> Result<()> {
    // Imported records are history, not the checkout's state (milestone 4 D6): its facts and its
    // count of records not yet curated come from live records only.
    if !crate::raw::is_live(&e.source) {
        return Ok(());
    }
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
            // A prompt another agent sent holds no directive of the owner's, whatever it says
            // (#273).
            if body["agent_sent"] != true && manifest::is_owner_line(str_at(&body, "prompt")) {
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

/// A window op that moves the curation checkpoint over `from..=to` of `device`: the checkouts
/// with events there are built again, so their not-yet-curated count follows it.
pub(crate) fn curated(k: &Connection, device: &str, from: i64, to: i64) -> Result<()> {
    schema(k)?;
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
         SELECT DISTINCT repo, branch, device FROM manifest_facts
         WHERE device = ?1 AND fact = 'event' AND seq BETWEEN ?2 AND ?3",
        params![device, from, to],
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

/// `s` on one line, cut to `n` characters at a space, or after a Japanese or Chinese clause mark
/// (text in those languages has no spaces): a token is shown whole or not at all, so a rule added
/// after the manifest was built still matches it at SessionStart's gate. A first token or clause
/// longer than `n` is left out, not cut.
fn one_line(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(n) {
        Some((at, c)) => {
            let end = if c == ' ' {
                at
            } else {
                flat[..at]
                    .char_indices()
                    .rev()
                    .find_map(|(i, ch)| match ch {
                        ' ' => Some(i),
                        '、' | '。' | '，' | '！' | '？' => Some(i + ch.len_utf8()),
                        _ => None,
                    })
                    .unwrap_or(0)
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
    // Past the device's curation checkpoint: a record the checkpoint is inside of is curated
    // only up to its offset. The checkpoint is the op log's, so a rebuild gives the same count.
    let (checkpoint, part) = raw.curation_checkpoint(device)?;
    let curated = if part.is_some() {
        checkpoint - 1
    } else {
        checkpoint
    };
    let Some((last_seq, as_of, uncurated)) = k
        .query_row(
            "SELECT MAX(seq), MAX(ts), COALESCE(SUM(seq > ?4), 0) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND branch = ?3",
            params![device, repo, branch, curated],
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

    /// The not-yet-curated count follows the curation checkpoint (#204): a window op over some
    /// of the checkout's records leaves the rest, one over all of them leaves none, and a record
    /// a window stops inside of still counts. A rebuild gives the same count.
    #[test]
    fn the_not_yet_curated_count_follows_the_checkpoint() {
        use crate::raw::OpKind;
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        assert!(
            manifest(home.path())
                .0
                .contains("9 record(s) not yet curated")
        );
        let window = |to: i64, to_offset: Option<i64>| {
            let op = serde_json::json!({"from_seq": 1, "from_offset": null, "to_seq": to,
                "to_offset": to_offset, "outcome": "curated"});
            raw::open(home.path())
                .unwrap()
                .append_ops(&[(OpKind::Window, op)])
                .unwrap();
            worker::run_once(home.path()).unwrap();
            manifest(home.path()).0
        };
        assert!(window(5, Some(3)).contains("5 record(s) not yet curated"));
        assert!(window(5, None).contains("4 record(s) not yet curated"));
        let all = window(9, None);
        assert!(all.contains("0 record(s) not yet curated"), "{all}");
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            std::fs::remove_file(home.path().join(f)).ok();
        }
        worker::run_once(home.path()).unwrap();
        assert_eq!(manifest(home.path()).0, all);
        // A restore that lost the last two window ops while knowledge.db stays.
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq > 1", [])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let back = manifest(home.path()).0;
        assert!(back.contains("5 record(s) not yet curated"), "{back}");
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

    /// A claim op for an event appended now: quoting all of its text, of `kind` and `status`.
    fn claimed(
        store: &mut Raw,
        e: Event,
        kind: &str,
        status: &str,
        supersedes: Vec<String>,
    ) -> ((crate::raw::OpKind, Value), String) {
        let seq = store.append(&e).unwrap();
        let quote = crate::curate::long_text(&e).unwrap();
        let evidence = crate::claims::Evidence {
            device: store.device().to_owned(),
            seq,
            offset: 0,
            length: quote.len() as i64,
            sentence: 0,
            quote: quote.clone(),
            claim_at: None,
        };
        let uid = crate::claims::uid(kind, &evidence);
        let op = crate::claims::ClaimOp {
            id: format!("c{seq}"),
            kind: kind.into(),
            status: status.into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: quote,
            evidence: vec![evidence],
            supersedes,
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
        };
        let op = (crate::raw::OpKind::Claim, serde_json::to_value(op).unwrap());
        (op, uid)
    }

    /// A digest op of `repo` whose lines each cite `uids`.
    fn digest(repo: &str, lines: &[(&str, &[&String])]) -> (crate::raw::OpKind, Value) {
        let lines: Vec<Value> = lines
            .iter()
            .map(|(text, uids)| serde_json::json!({"text": text, "uids": uids}))
            .collect();
        let op = serde_json::json!({"agent": "claude", "session": "s3", "repo": repo,
            "through": {"device": "d", "seq": 1}, "lines": lines});
        (crate::raw::OpKind::Digest, op)
    }

    /// Spec 3.4, 4.4: SessionStart shows the repository's newest digest only while every claim it
    /// cites is current: a retracted or superseded one makes it stale, and an older digest is not
    /// shown in its place. Another repository's digest and a malformed one are never shown.
    #[test]
    fn the_manifest_shows_the_newest_digest_only_while_its_claims_are_current() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, tabs_uid) = claimed(
            &mut store,
            said(1, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (flaky, flaky_uid) = claimed(
            &mut store,
            said(2, "The CI test is flaky."),
            "open item",
            "decided",
            vec![],
        );
        store.append_ops(&[tabs, flaky.clone()]).unwrap();
        store
            .append_ops(&[
                digest("r", &[("An older digest.", &[&tabs_uid])]),
                digest(
                    "r",
                    &[
                        ("Tabs are the rule.", &[&tabs_uid]),
                        ("CI is flaky.", &[&flaky_uid]),
                    ],
                ),
                digest("other", &[("Another repository.", &[&tabs_uid])]),
                digest(
                    "r",
                    &[(
                        "x".repeat(crate::digest::MAX_CHARS + 1).as_str(),
                        &[&tabs_uid],
                    )],
                ),
            ])
            .unwrap();
        let shown_digest = |store: &Raw| {
            worker::run_once(home.path()).unwrap();
            let text = shown(home.path(), store).unwrap();
            text.split("## Digest of the last session\n")
                .nth(1)
                .map(|d| d.split("\n## ").next().unwrap().to_owned())
        };
        // The malformed one is the newest op, and is skipped with its reason.
        assert_eq!(
            shown_digest(&store).as_deref(),
            Some("- Tabs are the rule.\n- CI is flaky.")
        );
        let k = rusqlite::Connection::open(home.path().join("knowledge.db")).unwrap();
        let reason: String = k
            .query_row("SELECT reason FROM digest_skips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "over the 2,000-character cap");
        // Retracted: stale, and the older digest does not come back.
        let mut retract = flaky.1.clone();
        (retract["id"], retract["status"]) = ("r1".into(), "retracted".into());
        store.append_ops(&[(flaky.0, retract)]).unwrap();
        assert_eq!(shown_digest(&store), None);
        // A newer digest of current claims is shown, until one of them is superseded.
        store
            .append_ops(&[digest("r", &[("Tabs only.", &[&tabs_uid])])])
            .unwrap();
        assert_eq!(shown_digest(&store).as_deref(), Some("- Tabs only."));
        let (spaces, spaces_uid) = claimed(
            &mut store,
            said(3, "Use spaces instead."),
            "decision",
            "decided",
            vec![tabs_uid.clone()],
        );
        store.append_ops(&[spaces]).unwrap();
        assert_eq!(shown_digest(&store), None);
        // A claim the owner corrects after the digest: it no longer says what the digest saw.
        let mut now = digest("r", &[("Spaces now.", &[&spaces_uid])]);
        now.1["lines"][0]["seen"] =
            serde_json::json!([crate::digest::version("decided", "Use spaces instead.")]);
        store.append_ops(&[now]).unwrap();
        assert_eq!(shown_digest(&store).as_deref(), Some("- Spaces now."));
        let correction = crate::claims::CorrectionOp {
            uid: spaces_uid.clone(),
            anchor: crate::claims::Anchor {
                device: store.device().to_owned(),
                seq: 3,
            },
            status: None,
            body: Some("Spaces, four of them.".into()),
        };
        let correction = serde_json::to_value(correction).unwrap();
        store
            .append_ops(&[(crate::raw::OpKind::Correction, correction)])
            .unwrap();
        assert_eq!(shown_digest(&store), None);
    }

    /// Spec 1.7: `oboete rebuild` makes knowledge.db again from raw.db and the op log: the same
    /// current claims and the same manifest, no provider called, no old file left.
    #[test]
    fn rebuilding_gives_the_same_claims_with_no_call() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, old) = claimed(
            &mut store,
            said(1, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, _) = claimed(
            &mut store,
            said(2, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (flaky, _) = claimed(
            &mut store,
            said(3, "The CI test is flaky."),
            "open item",
            "decided",
            vec![],
        );
        store.append_ops(&[tabs, spaces, flaky]).unwrap();
        worker::run_once(home.path()).unwrap();
        let knowledge = home.path().join("knowledge.db");
        // Each read holds raw.db open only while it reads, as a hook does: a rebuild moves
        // knowledge.db aside only while no store is open.
        let snapshot = || {
            let k = rusqlite::Connection::open(&knowledge).unwrap();
            let claims = crate::claims::current(&k, "r").unwrap();
            let store = raw::open(home.path()).unwrap();
            (claims, shown(home.path(), &store).unwrap())
        };
        drop(store);
        let before = snapshot();
        assert_eq!(before.0.len(), 2);
        // Only the old file has it.
        rusqlite::Connection::open(&knowledge)
            .unwrap()
            .execute_batch("CREATE TABLE marker(x)")
            .unwrap();
        // A home that curates: the rebuild still asks no provider (the phase would open
        // providers.db for its pending window, here with no budget, so nothing leaves).
        let config = "[summary]\ncurate = true\n[[providers]]\nkind = \"openai\"\n\
                      name = \"spent\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\n\
                      daily_budget = 0\n";
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        worker::rebuild(home.path()).unwrap();
        assert_eq!(snapshot(), before);
        let k = rusqlite::Connection::open(&knowledge).unwrap();
        let marker: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marker, 0);
        assert!(!home.path().join("providers.db").exists());
        let left = std::fs::read_dir(home.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("rebuilding-")
        });
        assert!(!left);
    }

    /// Spec 4.4, 4.9: the repository's delivered decisions and open items, the newest first, read
    /// when the text is: an earlier decision that a later decision of the owner ended is listed
    /// after it, both dated (owner decision 31); a retracted, merely proposed or other
    /// repository's claim is not among them, and reading writes nothing.
    #[test]
    fn the_manifest_lists_the_current_decisions_of_its_repo() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let min = 60_000;
        let said = |ts: i64, text: &str| {
            ev(
                "prompt",
                "s3",
                ts,
                cwd.path(),
                serde_json::json!({"prompt": text}),
            )
        };
        let (tabs, old) = claimed(
            &mut store,
            said(10 * min, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, _) = claimed(
            &mut store,
            said(11 * min, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (flaky, _) = claimed(
            &mut store,
            said(12 * min, "The CI test is flaky."),
            "open item",
            "proposed",
            vec![],
        );
        let (cache, _) = claimed(
            &mut store,
            said(13 * min, "Maybe cache it."),
            "decision",
            "proposed",
            vec![],
        );
        let (gone, _) = claimed(
            &mut store,
            said(14 * min, "Drop the old API."),
            "decision",
            "retracted",
            vec![],
        );
        let other = Event {
            repo: Some("other".into()),
            ..said(15 * min, "Other repo rule.")
        };
        let (elsewhere, _) = claimed(&mut store, other, "decision", "decided", vec![]);
        store
            .append_ops(&[tabs, spaces, flaky, cache, gone, elsewhere])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let knowledge = home.path().join("knowledge.db");
        let before = std::fs::metadata(&knowledge).unwrap().modified().unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            std::fs::metadata(&knowledge).unwrap().modified().unwrap(),
            before
        );
        let section = text
            .split("## Decisions and open items\n")
            .nth(1)
            .unwrap()
            .split("\n## ")
            .next()
            .unwrap();
        assert_eq!(
            section,
            "- 1970-01-01 open item: The CI test is flaky.\n\
             - 1970-01-01 decision: Use spaces instead.\n\
             - 1970-01-01 decision, superseded by the 1970-01-01 decision above: Use tabs."
        );
        // Before the owner's directives, as spec 4.9 orders them.
        assert!(
            text.find("## Decisions and open items").unwrap()
                < text.find("## Owner's directives").unwrap()
        );
        // At most the limit, the newest first: the query stops there, not the caller.
        let k = rusqlite::Connection::open(&knowledge).unwrap();
        let newest: Vec<String> = crate::claims::decisions(&k, "r", 1)
            .unwrap()
            .into_iter()
            .map(|c| c.body)
            .collect();
        assert_eq!(newest, ["The CI test is flaky."]);
        // Through the indexes, newest first: no scan of the claims, however many there are.
        let sql = format!(
            "EXPLAIN QUERY PLAN {} {}",
            crate::claims::delivered("a.repo = ?1", crate::claims::LINKS_END_DECISIONS),
            crate::claims::DECIDED
        );
        let plan: Vec<String> = k
            .prepare(&sql)
            .unwrap()
            .query_map(("r", 1), |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(plan.iter().all(|p| !p.starts_with("SCAN")), "{plan:?}");
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
    fn a_lost_tombstone_gives_its_target_back_to_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let device = store.device().to_owned();
        let seq = store
            .after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .find(|r| matches!(&r.item, Item::Event(e) if e.body.contains("look at it")))
            .unwrap()
            .seq;
        store
            .append_tombstone(Target::Record { device, seq })
            .unwrap();
        worker::run_once(home.path()).unwrap();
        assert!(!manifest(home.path()).0.contains("look at it"));
        let top = store.max_seq().unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top]) // lost (MUST-M14)
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let text = manifest(home.path()).0;
        assert!(text.contains("look at it"), "{text}"); // as a rebuild of raw now gives
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
        // Text with no spaces is cut after a clause mark, never inside a clause.
        assert_eq!(
            one_line("鍵を分けた。次に山田太郎さんへ送る", 12),
            "鍵を分けた。…"
        );
        assert_eq!(one_line("鍵を分けた、次に送る", 8), "鍵を分けた、…");
        assert_eq!(one_line("山田太郎さんへ送る", 4), "…");
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

    /// #273: a prompt another agent sent is no directive of the owner's, whatever it says; the
    /// owner's own line of the same words is one.
    #[test]
    fn a_prompt_another_agent_sent_is_no_owner_directive() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let said = "always run zqxlint first";
        let shown = format!("\"{said}\"");
        let prompt =
            |session: &str, ts: i64, body: Value| ev("prompt", session, ts, cwd.path(), body);
        let sent = serde_json::json!({"prompt": said, "agent_sent": true});
        store.append(&prompt("s1", 60_000, sent)).unwrap();
        worker::run_once(home.path()).unwrap();
        let m = manifest(home.path()).0;
        assert!(!m.contains(&shown), "{m}");
        let typed = serde_json::json!({"prompt": said});
        store.append(&prompt("s2", 120_000, typed)).unwrap();
        worker::run_once(home.path()).unwrap();
        let m = manifest(home.path()).0;
        assert!(
            m.contains("## Owner's directives") && m.contains(&shown),
            "{m}"
        );
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

    /// Milestone 4 D6: imported records are history, not the checkout's state: the manifest's
    /// last prompt, its sessions and its count of records not yet curated come from live records
    /// only.
    #[test]
    fn the_manifest_ignores_imported_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let before = shown(home.path(), &store).unwrap();
        let prompt = serde_json::json!({"prompt": "An imported prompt."});
        let imported = Event {
            source: "oboete-v1".into(),
            ..ev("prompt", "v1", 100 * 60_000, cwd.path(), prompt)
        };
        store.append(&imported).unwrap();
        worker::run_once(home.path()).unwrap();
        assert_eq!(shown(home.path(), &store).unwrap(), before);
    }

    /// Milestone 4 D6, MUST-M9: the backlog line counts live records not yet curated, never the
    /// imported ones that wait for `recurate --source`.
    #[test]
    fn the_backlog_line_does_not_count_parked_records() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        worker::run_once(home.path()).unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let backlog = |store: &Raw| {
            let text = shown(home.path(), store).unwrap();
            let as_of = text.split("## As of\n").nth(1).unwrap();
            as_of
                .lines()
                .next()
                .unwrap()
                .split("; ")
                .nth(1)
                .unwrap()
                .to_owned()
        };
        let before = backlog(&store);
        assert!(before.ends_with("record(s) not yet curated"), "{before}");
        for i in 0..3 {
            let body = serde_json::json!({"prompt": format!("Imported {i}.")});
            let e = Event {
                source: "transcript".into(),
                ..ev("prompt", "t", (200 + i) * 60_000, cwd.path(), body)
            };
            store.append(&e).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        assert_eq!(backlog(&store), before);
    }

    const DAY: i64 = 86_400_000;

    fn said(cwd: &Path, ts: i64, text: &str) -> Event {
        ev("prompt", "s3", ts, cwd, serde_json::json!({"prompt": text}))
    }

    /// `op` as `speaker` said it.
    fn by(mut op: (crate::raw::OpKind, Value), speaker: &str) -> (crate::raw::OpKind, Value) {
        op.1["speaker"] = speaker.into();
        op
    }

    /// The lines under `text`'s heading `title`.
    fn lines_of(text: &str, title: &str) -> Vec<String> {
        text.split(&format!("## {title}\n"))
            .nth(1)
            .map(|s| {
                s.split("\n## ")
                    .next()
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A home whose checkout (r, main) has `session`'s records and the claims `ops` makes, applied,
    /// then a last prompt `prompt` when given (the words SessionStart ranks by). What SessionStart
    /// shows.
    fn start(
        claims: impl FnOnce(&mut Raw, &Path) -> Vec<(crate::raw::OpKind, Value)>,
        prompt: Option<&str>,
    ) -> (tempfile::TempDir, tempfile::TempDir, String) {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let ops = claims(&mut store, cwd.path());
        store.append_ops(&ops).unwrap();
        if let Some(p) = prompt {
            store.append(&said(cwd.path(), 100 * DAY, p)).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        (home, cwd, text)
    }

    /// Spec 3.4, owner decision 31 (#295 row 1): an earlier decision that a curator link from a
    /// later decision of the owner ended is delivered after it, each with its date.
    #[test]
    fn an_earlier_decision_ended_by_a_later_owner_decision_is_delivered_after_it_with_both_dates() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, spaces]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            [
                "- 1970-01-04 decision: Use spaces instead.",
                "- 1970-01-02 decision, superseded by the 1970-01-04 decision above: Use tabs.",
            ]
        );
    }

    /// D1: only a curator link from a later claim the owner backs, decided or done, leaves the
    /// earlier decision delivered: the user's own words, or a proposal the user accepted (Codex on
    /// #303). One from a proposal still open, a retracted claim, an earlier claim or a claim the
    /// owner does not back still ends it.
    #[test]
    fn a_link_from_a_proposal_a_retracted_claim_or_an_earlier_claim_delivers_nothing_new() {
        let cases = [
            ("proposed", "assistant proposal", 1),
            ("retracted", "user", 1),
            ("decided", "user", -1),
            ("decided", "assistant", 1),
            ("decided", "assistant proposal", 1),
        ];
        let (_h, _c, text) = start(
            |store, cwd| {
                let mut ops = Vec::new();
                for (i, (status, speaker, after)) in (0_i64..).zip(cases) {
                    let day = 10 * (i + 1);
                    let (old, uid) = claimed(
                        store,
                        said(cwd, day * DAY, &format!("Old rule {i}.")),
                        "decision",
                        "decided",
                        vec![],
                    );
                    let (new, _) = claimed(
                        store,
                        said(cwd, (day + after) * DAY, &format!("New rule {i}.")),
                        "decision",
                        status,
                        vec![uid],
                    );
                    ops.extend([old, by(new, speaker)]);
                }
                ops
            },
            None,
        );
        let shown = lines_of(&text, "Decisions and open items").join("\n");
        for ended in 0..4 {
            assert!(!shown.contains(&format!("Old rule {ended}.")), "{shown}");
        }
        assert!(
            shown.contains("New rule 2.") && shown.contains("New rule 3."),
            "{shown}"
        );
        // The accepted proposal is the owner's later decision: the earlier one follows it.
        assert!(
            shown.contains(
                "- 1970-02-21 decision: New rule 4.\n\
                 - 1970-02-20 decision, superseded by the 1970-02-21 decision above: Old rule 4."
            ),
            "{shown}"
        );
    }

    /// #261, #302 item 3: a restatement quotes the same words of the same event, so its link is
    /// not from a later claim, and it ends the claim it restates as before.
    #[test]
    fn a_restatement_ends_a_claim_as_before() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (old, uid) = claimed(
                    store,
                    said(cwd, DAY, "Keep the cache small."),
                    "decision",
                    "decided",
                    vec![],
                );
                let mut again = old.clone();
                again.1["id"] = "r1".into();
                again.1["kind"] = "preference".into();
                again.1["supersedes"] = serde_json::json!([uid]);
                vec![old, again]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 preference: Keep the cache small."]
        );
    }

    /// Review Focus 1 (#295 row 2): a pair takes two places. With one place left, the earlier
    /// decision and the later one that ended it go to the index together, the later first, and the
    /// next claim that fits takes the place. The earlier decision never stands alone.
    #[test]
    fn a_picked_earlier_claim_brings_its_later_claim_and_the_pair_takes_two_places() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (early, old) = claimed(
                    store,
                    said(cwd, DAY, "Zebra builds use the old runner."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (later, _) = claimed(
                    store,
                    said(cwd, 30 * DAY, "Use the new runner."),
                    "decision",
                    "decided",
                    vec![old],
                );
                let (short, _) = claimed(
                    store,
                    said(cwd, 5 * DAY, "Keep logs short."),
                    "decision",
                    "decided",
                    vec![],
                );
                let mut ops = vec![early, later, short];
                for day in 11..20 {
                    let text = format!("Zebra rule {day}.");
                    ops.push(
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0,
                    );
                }
                ops
            },
            // "zebra" is the only word it shares with them.
            Some("What about zebra?"),
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10, "{bodies:?}");
        assert!(
            bodies.iter().take(9).all(|l| l.contains("Zebra rule")),
            "{bodies:?}"
        );
        assert_eq!(bodies[9], "- 1970-01-06 decision: Keep logs short.");
        let more = lines_of(&text, "More from memory");
        assert_eq!(more.len(), 3, "{more:?}");
        assert!(
            more[1].ends_with("1970-01-31 decision: Use the new runner."),
            "{more:?}"
        );
        let later_id = more[1][2..14].to_owned();
        assert!(
            more[2].ends_with(&format!(
                "1970-01-02 decision, superseded by {later_id}: Zebra builds use the old runner."
            )),
            "{more:?}"
        );
    }

    /// Spec 3.4's switch (D2): once curator links end decisions, the earlier decision leaves
    /// delivery as it does from the tips.
    #[test]
    fn with_links_ending_decisions_only_the_later_claim_is_delivered() {
        let (home, _c, _text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, spaces]
            },
            None,
        );
        let k = rusqlite::Connection::open(home.path().join("knowledge.db")).unwrap();
        let bodies = |links_end: bool| -> Vec<String> {
            k.prepare(&crate::claims::delivered("a.repo = ?1", links_end))
                .unwrap()
                .query_map(["r"], |r| r.get(5))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let mut delivered = bodies(false);
        delivered.sort();
        assert_eq!(delivered, ["Use spaces instead.", "Use tabs."]);
        assert_eq!(bodies(true), ["Use spaces instead."]);
    }

    /// Row 54-6: a decision and its retraction in different chunks: SessionStart never goes back
    /// to before the retraction.
    #[test]
    fn a_retraction_in_another_chunk_never_brings_the_decision_back() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (ship, uid) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Ship the beta on Friday."),
            "decision",
            "decided",
            vec![],
        );
        store.append_ops(&[ship]).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 decision: Ship the beta on Friday."]
        );
        let (retract, _) = claimed(
            &mut store,
            said(
                cwd.path(),
                2 * DAY,
                "Do not ship the beta on Friday after all.",
            ),
            "decision",
            "retracted",
            vec![uid],
        );
        store.append_ops(&[retract]).unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(!text.contains("## Decisions and open items"), "{text}");
        assert!(!text.contains("## More from memory"), "{text}");
    }

    /// Spec 4.4: the owner's global preferences first, then about 10 claims with their bodies, and
    /// the rest one line each after the tools that find more, last.
    #[test]
    fn global_preferences_come_first_and_about_ten_claims_have_bodies() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let ops: Vec<_> = (1..=12)
            .map(|day| {
                let text = format!("Rule {day:02}.");
                claimed(
                    &mut store,
                    said(cwd.path(), day * DAY, &text),
                    "decision",
                    "decided",
                    vec![],
                )
                .0
            })
            .collect();
        store.append_ops(&ops).unwrap();
        crate::claims::pref_add(home.path(), "Answer in Japanese.").unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert!(text.starts_with("## Global preferences\n"), "{text}");
        let prefs = lines_of(&text, "Global preferences");
        assert_eq!(prefs.len(), 1);
        assert!(
            prefs[0].ends_with(" preference: Answer in Japanese."),
            "{prefs:?}"
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10);
        assert_eq!(bodies[0], "- 1970-01-13 decision: Rule 12.");
        assert_eq!(bodies[9], "- 1970-01-04 decision: Rule 03.");
        let more = lines_of(&text, "More from memory");
        assert_eq!(more[0], TOOLS);
        assert_eq!(more.len(), 3);
        assert!(
            more[1].ends_with(" 1970-01-03 decision: Rule 02."),
            "{more:?}"
        );
        assert!(
            more[2].ends_with(" 1970-01-02 decision: Rule 01."),
            "{more:?}"
        );
        assert!(text.ends_with(&format!("{}\n", more[2])));
    }

    /// Spec 4.4: claims are chosen by the words they share with the manifest's last prompt and
    /// files, then by recency, and listed newest first whatever chose them.
    #[test]
    fn the_chosen_claims_are_listed_newest_first_whatever_chose_them() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let mut ops = vec![
                    claimed(
                        store,
                        said(cwd, DAY, "Walrus names stay short."),
                        "decision",
                        "decided",
                        vec![],
                    )
                    .0,
                ];
                for day in 2..=11 {
                    let text = format!("Rule {day:02}.");
                    ops.push(
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0,
                    );
                }
                ops
            },
            Some("Rename the walrus module"),
        );
        let bodies = lines_of(&text, "Decisions and open items");
        assert_eq!(bodies.len(), 10);
        assert_eq!(bodies[0], "- 1970-01-12 decision: Rule 11.");
        assert_eq!(bodies[9], "- 1970-01-02 decision: Walrus names stay short.");
        let more = lines_of(&text, "More from memory");
        assert!(
            more[1].ends_with(" 1970-01-03 decision: Rule 02."),
            "{more:?}"
        );
    }

    /// Spec 4.4: a pair is listed as one unit, dated by its later claim: a claim dated between the
    /// two never separates them.
    #[test]
    fn a_claim_dated_between_a_pair_never_separates_it() {
        let (_h, _c, text) = start(
            |store, cwd| {
                let (tabs, old) = claimed(
                    store,
                    said(cwd, DAY, "Use tabs."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (width, _) = claimed(
                    store,
                    said(cwd, 2 * DAY, "Keep lines under 100."),
                    "decision",
                    "decided",
                    vec![],
                );
                let (spaces, _) = claimed(
                    store,
                    said(cwd, 3 * DAY, "Use spaces instead."),
                    "decision",
                    "decided",
                    vec![old],
                );
                vec![tabs, width, spaces]
            },
            None,
        );
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            [
                "- 1970-01-04 decision: Use spaces instead.",
                "- 1970-01-02 decision, superseded by the 1970-01-04 decision above: Use tabs.",
                "- 1970-01-03 decision: Keep lines under 100.",
            ]
        );
    }

    /// D4: a checkout with no manifest to show (here a branch with no records) still gets the
    /// repository's delivered claims.
    #[test]
    fn a_checkout_with_claims_and_no_manifest_row_still_gets_them() {
        let (home, _c, _text) = start(
            |store, cwd| {
                vec![
                    claimed(
                        store,
                        said(cwd, DAY, "Use tabs."),
                        "decision",
                        "decided",
                        vec![],
                    )
                    .0,
                ]
            },
            None,
        );
        let store = raw::open(home.path()).unwrap();
        let rules = crate::capture::Settings::load(home.path()).unwrap().rules;
        let text = text(home.path(), &store, "r", "feature", "none", rules.version())
            .unwrap()
            .unwrap();
        assert!(!text.contains("## As of"), "{text}");
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-02 decision: Use tabs."]
        );
    }

    /// D3, #302 item 2: what raw holds and the worker has not applied yet is never shown: a claim
    /// the owner retracted or corrected, and one that quotes a removed record, with the claim a
    /// retracted one ended and a digest that cites one. Once the worker applies them, what they say
    /// is shown.
    #[test]
    fn an_owner_retraction_the_worker_has_not_applied_is_not_shown() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        session(home.path(), cwd.path());
        let mut store = raw::open(home.path()).unwrap();
        let (tabs, old) = claimed(
            &mut store,
            said(cwd.path(), DAY, "Use tabs."),
            "decision",
            "decided",
            vec![],
        );
        let (width, width_uid) = claimed(
            &mut store,
            said(cwd.path(), 2 * DAY, "Keep lines under 100."),
            "decision",
            "decided",
            vec![],
        );
        let (spaces, spaces_uid) = claimed(
            &mut store,
            said(cwd.path(), 3 * DAY, "Use spaces instead."),
            "decision",
            "decided",
            vec![old],
        );
        let (pin, _) = claimed(
            &mut store,
            said(cwd.path(), 4 * DAY, "Pin the toolchain."),
            "decision",
            "decided",
            vec![],
        );
        let anchor = |op: &(crate::raw::OpKind, Value)| op.1["evidence"][0].clone();
        let (spaces_at, width_at, pin_at) = (anchor(&spaces), anchor(&width), anchor(&pin));
        store
            .append_ops(&[
                tabs,
                width,
                spaces,
                pin,
                digest("r", &[("Lines stay short.", &[&width_uid])]),
            ])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(lines_of(&text, "Decisions and open items").len(), 4);
        assert_eq!(
            lines_of(&text, "Digest of the last session"),
            ["- Lines stay short."]
        );
        let correct = |uid: &str, at: &Value, status: Value, body: Value| {
            let op = serde_json::json!({"uid": uid, "status": status, "body": body,
                "anchor": {"device": at["device"], "seq": at["seq"]}});
            (crate::raw::OpKind::Correction, op)
        };
        store
            .append_ops(&[
                correct(&spaces_uid, &spaces_at, "retracted".into(), Value::Null),
                correct(
                    &width_uid,
                    &width_at,
                    Value::Null,
                    "Keep lines under 80.".into(),
                ),
            ])
            .unwrap();
        let device = store.device().to_owned();
        store
            .append_tombstone(Target::Record {
                device,
                seq: pin_at["seq"].as_i64().unwrap(),
            })
            .unwrap();
        let text = shown(home.path(), &store).unwrap_or_default();
        assert!(!text.contains("## Decisions and open items"), "{text}");
        assert!(!text.contains("## Digest of the last session"), "{text}");
        assert!(!text.contains("## More from memory"), "{text}");
        worker::run_once(home.path()).unwrap();
        let text = shown(home.path(), &store).unwrap();
        assert_eq!(
            lines_of(&text, "Decisions and open items"),
            ["- 1970-01-03 decision: Keep lines under 80."]
        );
    }

    /// Spec 4.9's drop order under an agent's size cap: the hook cuts from the end, so the index
    /// goes first and the manifest's own sections stay.
    #[test]
    fn the_index_goes_first_under_the_size_cap() {
        let (_h, _c, text) = start(
            |store, cwd| {
                (1..=14)
                    .map(|day| {
                        let text = format!("Rule {day:02}.");
                        claimed(
                            store,
                            said(cwd, day * DAY, &text),
                            "decision",
                            "decided",
                            vec![],
                        )
                        .0
                    })
                    .collect()
            },
            None,
        );
        let at = text.find("## More from memory").unwrap();
        let cut = crate::manifest::cut(&text, at + 1);
        assert!(!cut.contains("## More from memory"), "{cut}");
        assert!(
            cut.contains("## Files touched") && cut.contains("## As of"),
            "{cut}"
        );
    }
}
