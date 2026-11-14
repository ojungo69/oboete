//! Design B capture (docs/milestone-2-plan.md Tasks 2 and 3; spec 2.1-2.4): one hook payload
//! becomes the events appended to `raw.db`, each with the ledger of what redaction masked. Text is
//! kept whole up to `MAX_FIELD_BYTES`, head and tail above it; every stored string loses its
//! `<private>`-style blocks and is scanned in full; images and other base64 content become a
//! marker; git fields are read from files, never from `git`.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::hook::{compact, field, is_envelope, str_field, strip_blocks, without_blocks};
use crate::raw::Event;
use crate::{redact, repo};

/// Bytes a stored string keeps before only its head and tail are kept (spec 2.4, plan D4): the
/// largest of 64, 128 and 256 KB whose hook p95 on the slowest machine stays within the line.
/// None does: Windows is at 25.5 ms from 64 KB on, over the iMac's 1 KB floor of 25.2 ms
/// (docs/milestone-2.md, Task 12), so it is the smallest candidate.
pub const MAX_FIELD_BYTES: usize = 64 * 1024;

/// Set on the hooks replay spawns (M14, D4): the cap a candidate size is measured at, so a
/// candidate above `MAX_FIELD_BYTES` is written whole, as it would be under that cap. Read only
/// within 1 to 256 KB, the candidates' range.
pub const FIELD_CAP_ENV: &str = "OBOETE_FIELD_CAP";

fn field_cap(v: Option<&str>) -> usize {
    v.and_then(|v| v.parse().ok())
        .filter(|n| (1024..=256 * 1024).contains(n))
        .unwrap_or(MAX_FIELD_BYTES)
}

/// What `tool_output = "head-tail"` keeps of each tool output: its first and last halves. v1's
/// hook kept 8,000 characters (src/hook.rs `MAX_FIELD`). (Claude; overrulable)
pub const HEAD_TAIL_BYTES: usize = 8 * 1024;

/// Set by `oboete replay` on the hooks it spawns: what they record is stamped `replay` (D9).
pub const REPLAY_ENV: &str = "OBOETE_REPLAY";

/// The user's capture settings (spec 1.5): `[redaction]` and `[capture]` in config.toml.
#[derive(Clone)]
pub struct Settings {
    pub rules: redact::Rules,
    pub store_prompts: bool,
    pub tool_output: crate::config::ToolOutput,
    /// Who wrote the events: `hook`, or `replay` for a fixture (D9: only a hook says the owner is
    /// at work).
    pub source: &'static str,
}

impl Default for Settings {
    fn default() -> Self {
        let c = crate::config::Capture::default();
        Self {
            rules: redact::Rules::default(),
            store_prompts: c.store_prompts,
            tool_output: c.tool_output,
            source: "hook",
        }
    }
}

impl Settings {
    /// An error when either table is wrong: a hook then records nothing rather than store text
    /// under rules the user did not get (MUST-M16's marker and doctor say so).
    pub fn load(home: &Path) -> anyhow::Result<Self> {
        let c = crate::config::load_capture(home)?;
        Ok(Self {
            rules: redact::Rules::new(&c.redaction)?,
            store_prompts: c.capture.store_prompts,
            tool_output: c.capture.tool_output,
            // `oboete replay`'s spawned samples are hooks run by the replay, not by the owner.
            source: if std::env::var_os(REPLAY_ENV).is_some() {
                "replay"
            } else {
                "hook"
            },
        })
    }
}

/// Whether a prompt is wholly private: text that `<private>` blocks leave nothing of, which is
/// recorded as no event (`shaped`), so its turn is marked by the session's private-turn flag
/// instead (`deliverables::PrivateTurn`).
pub(crate) fn wholly_private_prompt(payload: &Value) -> bool {
    let typed = str_field(payload, &["prompt"]).unwrap_or("");
    !typed.trim().is_empty() && strip_blocks(typed, true).is_empty()
}

/// One event as captured, with the ledger rows of what its redaction masked: (field, finding).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Captured {
    pub event: Event,
    pub ledger: Vec<(String, redact::Finding)>,
}

/// The hook payload's mark of a prompt another agent sent (#273): set by the hook adapter from
/// a Codex rollout, or by the transcript parser, and never by an agent.
pub(crate) const AGENT_SENT: &str = "oboete_agent_sent";

/// The hook payload's Claude Code entry point (docs/my-work-model.md O2): set by the hook adapter
/// from `CLAUDE_CODE_ENTRYPOINT`, or by the transcript parser from the line, and never by an agent.
pub(crate) const ENTRYPOINT: &str = "oboete_entrypoint";

/// The hook payload's start of a tool's call, in ms (#692): set by the hook from the call's
/// PreToolUse, or by the transcript parser from the call's line, and never by an agent.
pub(crate) const STARTED: &str = "oboete_started";

/// The events one hook call of a ported agent records: none for events that carry nothing, nor
/// in a checkout whose `.oboete.toml` stops recording (slice 4a, `checkout_records`).
pub fn events(
    agent: &str,
    event: &str,
    payload: &Value,
    ts: i64,
    settings: &Settings,
) -> Vec<Captured> {
    events_read(agent, event, payload, ts, settings)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// `events`, each with its repository and folder as the hook read them, before the gate: the hook
/// compares them with what is never recorded, stored as given (slice 4a; owner, 2026-10-13). They
/// are never stored.
pub fn events_read(
    agent: &str,
    event: &str,
    payload: &Value,
    ts: i64,
    settings: &Settings,
) -> Vec<(Captured, (String, Option<String>))> {
    shaped(event, payload, settings)
        .and_then(|(kind, body)| {
            let place = place(payload);
            let read = (place.repo.clone(), place.cwd.clone());
            checkout_records(place.git.top.as_deref()).then(|| {
                (
                    labelled(agent, kind, body, payload, ts, place, settings),
                    read,
                )
            })
        })
        .into_iter()
        .collect()
}

/// Slice 4a (spec 6.1, A28): whether the checkout at `top` lets its events be recorded, as its
/// `.oboete.toml` says (`repo_capture`); one that cannot be read stops them too, said on stderr.
fn checkout_records(top: Option<&Path>) -> bool {
    repo_capture(top).unwrap_or_else(|e| {
        eprintln!("oboete: {e:#}: nothing in this checkout is recorded until it can be read");
        false
    })
}

/// The most of a checkout's `.oboete.toml` that is read: it holds one key (Claude; overrulable).
const CHECKOUT_TOML_LIMIT: u64 = 64 * 1024;

/// The `capture` key of `.oboete.toml` at the checkout's `top`, which may only restrict: true
/// with no such file or key. An error for a file that cannot be read, is not TOML, or whose
/// `capture` is not true or false; never its text, which may hold anything.
pub fn repo_capture(top: Option<&Path>) -> anyhow::Result<bool> {
    use std::io::Read;
    let Some(path) = top.map(|t| t.join(".oboete.toml")) else {
        return Ok(true);
    };
    // The repository's file, read only as a regular file of at most `CHECKOUT_TOML_LIMIT` bytes:
    // one that names a device or a pipe, or a large one, stops recording rather than holding the
    // hook (push security review of #489). Opened without waiting on a pipe, and the handle it
    // opened is what is checked: a name swapped meanwhile is not read (Codex on #489).
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => anyhow::bail!("{} cannot be read ({})", path.display(), e.kind()),
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    let mut text = String::new();
    match file.take(CHECKOUT_TOML_LIMIT + 1).read_to_string(&mut text) {
        Ok(n) if n as u64 > CHECKOUT_TOML_LIMIT => {
            anyhow::bail!("{} is over {CHECKOUT_TOML_LIMIT} bytes", path.display())
        }
        Ok(_) => {}
        Err(e) => anyhow::bail!("{} cannot be read ({})", path.display(), e.kind()),
    }
    let Ok(table) = text.parse::<toml::Table>() else {
        anyhow::bail!("{} is not valid TOML", path.display());
    };
    match table.get("capture") {
        None => Ok(true),
        Some(toml::Value::Boolean(on)) => Ok(*on),
        Some(_) => anyhow::bail!("{}: capture is neither true nor false", path.display()),
    }
}

/// Slice 4a: `list` as an event labelled now holds its values: each repository through the gate
/// as a `repo` label, each folder as a `cwd` label, so a rule added after an exclusion masks both
/// alike.
pub fn never_recorded(
    list: &crate::raw::CaptureExclusions,
    settings: &Settings,
) -> crate::raw::CaptureExclusions {
    let mut gate = Gate::new(settings);
    let cap = gate.cap;
    let mut label = |field: &str, s: &str| gate.text(field, &without_blocks(s, false), cap);
    crate::raw::CaptureExclusions {
        repos: list.repos.iter().map(|r| label("repo", r)).collect(),
        folders: list.folders.iter().map(|f| label("cwd", f)).collect(),
        patterns: list.patterns.iter().map(|p| label("cwd", p)).collect(),
    }
}

/// Parity row 14: whether claude-mem's exclusion `pattern` names the working folder `cwd`, as its
/// `project-filter.ts` reads one: `*` is any run of characters within one part of a path, `**`
/// any run across parts, `?` one character within a part, a leading `~` the home folder, and `\`
/// reads as `/`; it names the whole path or, for a pattern that does not start at a root (`/`,
/// `C:/`), the end of one in whole parts: `work/secret` names `/home/u/work/secret` and
/// `secret/**` names `/work/secret/src`, where claude-mem tries the last part alone, which no
/// pattern with a `/` names (Codex on #563). `dir/**` names `dir` too. A pattern with no `/`
/// also names every folder inside one it names, where claude-mem reads the last part alone: a
/// session started below an excluded project is not recorded either. Letter case is ignored
/// where the platform ignores it, as for a folder (`raw::folded`).
pub fn pattern_covers(pattern: &str, cwd: &str) -> bool {
    let slashed = |s: &str| crate::raw::folded(&s.replace('\\', "/")).into_owned();
    let home = slashed(&crate::config::home_dir().to_string_lossy());
    let pattern = expanded(slashed(pattern.trim()), &home);
    if pattern.is_empty() {
        return false;
    }
    let path = slashed(cwd);
    let path = unslashed(&path);
    let named = |glob: &str, s: &str| {
        globbed(
            &glob.chars().collect::<Vec<_>>(),
            &s.chars().collect::<Vec<_>>(),
        )
    };
    // No check of the last part alone: the end of the path holds it, and a pattern from a root
    // holds a `/` that no part holds.
    let rooted = matches!(
        pattern.as_bytes(),
        [b'/', ..] | [b'A'..=b'Z' | b'a'..=b'z', b':', b'/', ..]
    );
    let ends = |glob: &str| named(glob, path) || (!rooted && named(&format!("**/{glob}"), path));
    ends(&pattern)
        || (!pattern.contains('/') && path.split('/').any(|part| named(&pattern, part)))
        || pattern.strip_suffix("/**").is_some_and(ends)
}

/// `path` without trailing separators, but a root keeps its one: `/`, `C:/` (Codex on #563).
fn unslashed(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.is_empty() || trimmed.ends_with(':') {
        true => &path[..(trimmed.len() + 1).min(path.len())],
        false => trimmed,
    }
}

/// A leading `~` of `pattern` as claude-mem expands it, `home` written with `/`: a home that is a
/// root keeps its separator, so `~` names that root alone (Codex on #563).
fn expanded(pattern: String, home: &str) -> String {
    match pattern.strip_prefix('~') {
        Some("") => unslashed(home).to_owned(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.trim_end_matches('/')),
        _ => pattern,
    }
}

/// `s` as `glob` matches it whole, as claude-mem's glob becomes a regular expression: `**` any
/// characters, a whole `**/` folder part any characters ending in `/` or none (15.0), `*` any but
/// `/`, `?` one that is not `/`, any other character itself.
fn globbed(glob: &[char], s: &[char]) -> bool {
    // `at[j]`: whether the glob read so far matches `s[..j]`; two rows, reused.
    let mut at = vec![false; s.len() + 1];
    let mut next = vec![false; s.len() + 1];
    at[0] = true;
    let mut i = 0;
    while i < glob.len() {
        next.fill(false);
        let across = glob[i] == '*' && glob.get(i + 1) == Some(&'*');
        if across && glob.get(i + 2) == Some(&'/') && (i == 0 || glob[i - 1] == '/') {
            let mut before = false;
            for j in 0..=s.len() {
                before |= j > 0 && at[j - 1];
                next[j] = at[j] || (before && s[j - 1] == '/');
            }
            std::mem::swap(&mut at, &mut next);
            i += 3;
            continue;
        }
        match glob[i] {
            '*' => {
                for j in 0..=s.len() {
                    next[j] = at[j] || (j > 0 && next[j - 1] && (across || s[j - 1] != '/'));
                }
            }
            '?' => {
                for j in 1..=s.len() {
                    next[j] = at[j - 1] && s[j - 1] != '/';
                }
            }
            c => {
                for j in 1..=s.len() {
                    next[j] = at[j - 1] && s[j - 1] == c;
                }
            }
        }
        std::mem::swap(&mut at, &mut next);
        i += if across { 2 } else { 1 };
    }
    at[s.len()]
}

/// Slice 4a: a repository or folder as `oboete capture exclude` stores it, through the same gate
/// as an event's `repo` or `cwd` label (`field`).
pub fn exclusion_label(field: &str, value: &str, settings: &Settings) -> String {
    let mut gate = Gate::new(settings);
    let cap = gate.cap;
    gate.text(field, &without_blocks(value, false), cap)
}

/// Slice 4a: whether two exclusions of `field` name one place: a folder in any spelling of letter
/// case where the platform ignores it, as `covers` reads one; a repository exactly.
fn same_place(field: &str, a: &str, b: &str) -> bool {
    if field == "cwd" {
        crate::raw::folded(a) == crate::raw::folded(b)
    } else {
        a == b
    }
}

/// Slice 4a: the entry of `stored` that names `given`'s place (`same_place`), so adding another
/// spelling of a folder keeps the one entry an undo takes back (Codex on #489).
pub fn stored_entry<'a>(
    field: &str,
    given: &str,
    stored: &'a std::collections::BTreeSet<String>,
) -> Option<&'a String> {
    stored.iter().find(|s| same_place(field, s, given))
}

/// Slice 4a: the entry of `stored` an undo of `given` takes back: the one stored as given
/// (`stored_entry`), else `given`, which takes nothing back. Refused (`ReadAlike`) when other
/// entries read as `given` under the rules now: taking one back would record what it was stored
/// for too (Codex Security Review on #491), and several cannot be told apart (Codex on #489).
pub fn undone(
    field: &str,
    given: &str,
    stored: &std::collections::BTreeSet<String>,
    settings: &Settings,
) -> anyhow::Result<String> {
    if let Some(entry) = stored_entry(field, given, stored) {
        return Ok(entry.clone());
    }
    let now = exclusion_label(field, given, settings);
    let entries: Vec<String> = stored
        .iter()
        .filter(|s| same_place(field, &exclusion_label(field, s, settings), &now))
        .cloned()
        .collect();
    anyhow::ensure!(
        entries.is_empty(),
        ReadAlike {
            given: given.to_owned(),
            entries,
        }
    );
    Ok(given.to_owned())
}

/// `undone`'s refusal: `entries`, stored under their own names, read as `given` under the rules
/// now.
#[derive(Debug)]
pub struct ReadAlike {
    pub given: String,
    pub entries: Vec<String>,
}

impl std::fmt::Display for ReadAlike {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.entries.as_slice() {
            [one] => write!(
                f,
                "{} reads now as {one}, excluded under its own name, and is not recorded while \
                 it is: undo that one by its name, or change the rule that makes them read alike",
                self.given
            ),
            many => write!(
                f,
                "{} is how {} excluded entries read now ({}): undo one by the name it was excluded \
                 under",
                self.given,
                many.len(),
                many.join(", ")
            ),
        }
    }
}

impl std::error::Error for ReadAlike {}

/// `exclusion_label` under `rules` alone, for a reader that loads no capture settings (the
/// embedding phase, search): the same scan, with no ledger kept.
pub fn label_with(value: &str, rules: &redact::Rules) -> String {
    let cap = field_cap(std::env::var(FIELD_CAP_ENV).ok().as_deref());
    redact::scan_capped(&without_blocks(value, false), cap, rules).0
}

/// Slice 4a: why nothing that happens in `cwd` is recorded: its checkout's `.oboete.toml`, or an
/// exclusion (`list`) of its repository or of a folder around it, as its events would be
/// labelled now; None when it is recorded.
pub fn not_recorded_why(
    cwd: &str,
    list: &crate::raw::CaptureExclusions,
    settings: &Settings,
) -> Option<String> {
    let place = place(&json!({ "cwd": cwd }));
    match repo_capture(place.git.top.as_deref()) {
        Ok(true) => {}
        Ok(false) => return Some("its checkout's .oboete.toml says capture = false".into()),
        Err(e) => return Some(format!("{e:#}")),
    }
    let folder = place.cwd.clone().unwrap_or_else(|| folder_path(cwd));
    let (repo, cwd) = (
        exclusion_label("repo", &place.repo, settings),
        exclusion_label("cwd", &folder, settings),
    );
    // As the hook asks it: the place as read against the list as given, the labels against the
    // list as given and as the rules now read it (security review of #489).
    let never = never_recorded(list, settings);
    if list.refuses(Some(&never), (Some(&repo), None), Some((&place.repo, None))) {
        return Some(format!(
            "its repository {repo} is excluded (oboete capture exclude)"
        ));
    }
    // Parity row 14: a pattern names it.
    if [&folder, &cwd]
        .into_iter()
        .any(|c| list.pattern_naming(c).is_some())
        || never.pattern_naming(&cwd).is_some()
    {
        return Some(format!(
            "{cwd} is named by an excluded pattern (oboete capture exclude --pattern)"
        ));
    }
    (list.covers(None, Some(&folder))
        || list.covers(None, Some(&cwd))
        || never.covers(None, Some(&cwd)))
    .then(|| format!("{cwd} is in an excluded folder (oboete capture exclude --folder)"))
}

/// The events an imported hook payload records (milestone 4 D6): `events`' kinds and bodies, under
/// `settings` (source `oboete-v1` or `transcript`), labelled with the `repo` and `cwd` its source
/// kept, through the same gate, with no git read: the checkout has moved on since. `Touch` is
/// v1's `session_repos` row: kind `touch`, body `{}`.
pub fn imported(
    agent: &str,
    event: &str,
    payload: &Value,
    ts: i64,
    repo: &str,
    cwd: Option<&str>,
    settings: &Settings,
) -> Vec<Captured> {
    let shaped = match event {
        "Touch" => Some(("touch", json!({}))),
        _ => shaped(event, payload, settings),
    };
    let place = Place {
        repo: repo.to_owned(),
        cwd: cwd.map(str::to_owned),
        git: Git::default(),
    };
    shaped
        .map(|(kind, body)| labelled(agent, kind, body, payload, ts, place, settings))
        .into_iter()
        .collect()
}

/// The kind and body of the event a hook payload records: none for events that carry nothing.
fn shaped(event: &str, payload: &Value, settings: &Settings) -> Option<(&'static str, Value)> {
    let (kind, body) = match event {
        "SessionStart" => ("start", json!({"source": payload.get("source").map(clean)})),
        "UserPromptSubmit" => {
            let typed = str_field(payload, &["prompt"]).unwrap_or("");
            let prompt = strip_blocks(typed, true);
            // Harness traffic is recorded, but never as something the developer typed.
            let kind = if is_envelope(&prompt) {
                "envelope"
            } else {
                "prompt"
            };
            // A turn whose text was all `<private>` is none, whatever `store_prompts` says: so
            // whether a turn is recorded never depends on the settings, and Task 11 can count
            // a transcript's turns under any of them.
            if prompt.is_empty() {
                return None;
            }
            // A prompt another agent sent, as the hook adapter or the transcript parser read it
            // (#273), or a review template Codex built: never the developer's words.
            let sent = |mut body: Value| {
                if payload[AGENT_SENT] == true || crate::typed::codex_review_preset(&prompt) {
                    body["agent_sent"] = json!(true);
                }
                // Where a Claude Code prompt was typed, or sent from (O2): what My Work's goals read.
                if let Some(e) = crate::typed::entrypoint(&payload[ENTRYPOINT]) {
                    body["entrypoint"] = json!(e);
                }
                body
            };
            if !settings.store_prompts {
                // The turn is still an event (Task 11 counts it), without what was typed.
                return Some((kind, sent(json!({"omitted": true}))));
            }
            (kind, sent(json!({"prompt": base64_runs(&prompt)})))
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let mut body = json!({
                "tool": without_blocks(str_field(payload, &["tool_name", "toolName", "name"]).unwrap_or("?"), false),
                "input": text(field(payload, &["tool_input", "toolInput", "args"])),
                "output": text(field(payload, &["tool_response", "toolResult", "tool_output", "error"])),
                "failed": event == "PostToolUseFailure",
            });
            // Provenance and status when the payload has them: a subagent's call, and a call
            // that never got its answer (both from `oboete transcript`, src/transcript.rs).
            for key in ["agent_id", "interrupted"] {
                if let Some(v) = payload.get(key) {
                    body[key] = clean(v);
                }
            }
            // Slice 2 (docs/subagents.md): the subagent's type beside its id, as its stop keeps it,
            // and what the facts read of a call that started one, kept before the output is cut.
            if let Some(kind) = agent_field(payload, "agent_type") {
                body["agent_type"] = kind;
            }
            if let Some(launched) = launched(payload) {
                body["launched"] = launched;
            }
            // When the call began, its record's time being when it ended (#692).
            if let Some(ms) = payload[STARTED].as_i64() {
                body["started"] = json!(ms);
            }
            // A read of stored memory keeps its call and not its output, which repeats memory
            // (claude-mem stores nothing of it); a note saved to memory keeps neither its input
            // nor its output, the note being kept once, as its document.
            let tool = body["tool"].as_str().unwrap_or("").to_owned();
            if crate::curate::memory_read(&tool, &body) {
                body["output"] = json!("");
                body["memory_read"] = json!(true);
            } else if crate::curate::memory_write(&tool, &body) {
                body["input"] = json!("");
                body["output"] = json!("");
                body["memory_write"] = json!(true);
            }
            ("tool", body)
        }
        "Stop" => {
            // Codex's Stop carries no message, and Claude Code's an empty one when the session
            // ends in a tool call: the transcript has it, as claude-mem reads it (parity row 17).
            let given = str_field(payload, &["last_assistant_message"]);
            let reply = match given.filter(|t| !t.trim().is_empty()) {
                Some(t) => t.to_string(),
                None => str_field(payload, &["transcript_path"])
                    .map(|p| crate::hook::last_assistant_in_transcript(Path::new(p)))
                    .unwrap_or_default(),
            };
            let reply = without_blocks(&reply, false);
            if reply.trim().is_empty() {
                return None;
            }
            ("reply", json!({"assistant": base64_runs(&reply)}))
        }
        // X17 (docs/subagents.md): a Claude Code subagent's stop, its id and type as claude-mem
        // keeps them (`pickAgentField`: a non-empty string of at most 128 UTF-16 units each): no
        // text, and nothing without a session or an id.
        "SubagentStop" => {
            str_field(payload, &["session_id"]).filter(|s| !s.is_empty())?;
            let mut body = json!({"agent_id": agent_field(payload, "agent_id")?});
            if let Some(kind) = agent_field(payload, "agent_type") {
                body["agent_type"] = kind;
            }
            // Slice 2: the owner's Mark finished, which the viewer sends through this path.
            if payload[OWNER_MARK] == true {
                body["marked"] = json!(true);
            }
            (crate::raw::SUBAGENT_STOP, body)
        }
        "PreCompact" => (
            "compaction",
            json!({"trigger": payload.get("trigger").map(clean)}),
        ),
        "PostCompact" => {
            match str_field(payload, &["compact_summary"]).map(|s| without_blocks(s, false)) {
                Some(s) if !s.trim().is_empty() => {
                    ("compaction", json!({"summary": base64_runs(&s)}))
                }
                _ => return None,
            }
        }
        "SessionEnd" => {
            let mut body = json!({"reason": payload.get("reason").map(clean)});
            // Task 11: the worker counts the transcript's turns against raw's.
            if let Some(path) = str_field(payload, &["transcript_path"]) {
                body["transcript"] = json!(path);
            }
            ("end", body)
        }
        _ => return None, // PreToolUse and the rest carry nothing to keep
    };
    Some((kind, body))
}

/// The session, repo and branch labels `capture` gives an event of this payload, through the
/// same gate: the checkout a hook injects for, also when it records nothing (Grok's PreToolUse).
pub fn checkout(payload: &Value, settings: &Settings) -> (String, String, Option<String>) {
    let cwd = str_field(payload, &["cwd"]).unwrap_or(".");
    let mut gate = Gate::new(settings);
    let mut label =
        |field: &str, s: &str| gate.text(field, &without_blocks(s, false), MAX_FIELD_BYTES);
    let session = label(
        "session",
        str_field(payload, &["session_id", "sessionId"]).unwrap_or("unknown"),
    );
    let repo = label("repo", &repo::key(Path::new(cwd)));
    let branch = git(Path::new(cwd))
        .branch
        .as_deref()
        .map(|b| label("branch", b));
    (session, repo, branch)
}

/// Parity row 14: a pattern as `oboete capture exclude --pattern`, the page and the import of
/// claude-mem's settings store it: trimmed, else as given; refused when empty, over
/// `PATTERN_BYTES`, or holding a control character.
pub fn pattern(given: &str) -> anyhow::Result<String> {
    let p = given.trim();
    anyhow::ensure!(
        !p.is_empty() && p.len() <= PATTERN_BYTES && !p.chars().any(char::is_control),
        "a pattern is a folder's name or a claude-mem pattern of at most 256 bytes, with no \
         control character"
    );
    Ok(p.to_owned())
}

/// The longest pattern: a folder's name is at most 255 bytes on the common file systems, and a
/// match costs the pattern's length times the path's (`globbed`), so a hook's checks stay within
/// a few milliseconds whatever the path (Codex on #563).
const PATTERN_BYTES: usize = 256;

/// The checkout `payload`'s `cwd` names.
/// Slice 4a: `path` as a folder exclusion and an event's `cwd` both hold it: absolute against the
/// current directory, with `.` and `..` taken by name and no link followed, so the same folder
/// reads alike however it was written (Codex on #489).
pub fn folder_path(path: &str) -> String {
    let Ok(absolute) = std::path::absolute(path) else {
        return path.to_owned();
    };
    let mut named = std::path::PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                named.pop();
            }
            part => named.push(part),
        }
    }
    named.to_string_lossy().into_owned()
}

fn place(payload: &Value) -> Place {
    // A relative or missing `cwd` is the directory the agent runs the hook in.
    let cwd = folder_path(str_field(payload, &["cwd"]).unwrap_or("."));
    Place {
        repo: repo::key(Path::new(&cwd)),
        git: git(Path::new(&cwd)),
        cwd: Some(cwd),
    }
}

/// Where an event was recorded, as its labels name it.
struct Place {
    repo: String,
    cwd: Option<String>,
    git: Git,
}

/// One event of `kind` with `body`, its session from `payload` and its other labels from `place`,
/// all through one gate.
fn labelled(
    agent: &str,
    kind: &str,
    body: Value,
    payload: &Value,
    ts: i64,
    place: Place,
    settings: &Settings,
) -> Captured {
    // Labels are stored text too (spec 2.2): a path or branch can carry a token.
    let mut gate = Gate::new(settings);
    let cap = gate.cap;
    let mut label = |field: &str, s: &str| gate.text(field, &without_blocks(s, false), cap);
    let session = label(
        "session",
        str_field(payload, &["session_id", "sessionId"]).unwrap_or("unknown"),
    );
    let repo = label("repo", &place.repo);
    let git = place.git;
    let branch = git.branch.as_deref().map(|b| label("branch", b));
    let head = git.head.as_deref().map(|h| label("head", h));
    let gitdir = git.gitdir.as_deref().map(|g| label("gitdir", g));
    let cwd_label = place.cwd.map(|c| label("cwd", &c));
    // One gate for the body: every string and key in it, whatever field it is.
    let body = gate.value("", body, cap).to_string();
    Captured {
        event: Event {
            agent: agent.into(),
            // A label only: an event without one is still this device's next seq.
            session,
            kind: kind.into(),
            ts,
            repo: Some(repo),
            branch,
            head,
            gitdir,
            cwd: cwd_label,
            source: settings.source.into(),
            body,
            original_bytes: gate.cut,
        },
        ledger: gate.ledger,
    }
}

/// The owner's directive `oboete pref add` records (spec 3.3; MUST-M1's owner directive): an
/// event of its own, through the same gate as every stored string, with no checkout. Without
/// `repo` it names no repository: its claim is global, in no repository's current claims
/// (milestone 4's injection reads global ones by scope). With `repo` (slice 8b, R2), a repository
/// key as records name it, it is labelled with that repository alone: its claim is that
/// repository's. `curate` never shows it in a window: its claim is appended with it.
pub fn directive(text: &str, ts: i64, settings: &Settings, repo: Option<&str>) -> Captured {
    // #743: a repository's directives a session of their own, so that excluding one repository
    // marks no other repository's rules, nor the global ones, as of an excluded session.
    let session = repo.map_or_else(|| "owner".to_owned(), |r| format!("owner:{r}"));
    let payload = json!({"session_id": session});
    let scope = if repo.is_some() { "repo" } else { "global" };
    // As a typed prompt: an opted-out part is never stored, and binary content is its marker.
    let body = json!({"text": base64_runs(&strip_blocks(text, true)), "scope": scope});
    let place = Place {
        repo: repo.unwrap_or_default().to_owned(),
        cwd: None,
        git: Git::default(),
    };
    let mut c = labelled("oboete", "directive", body, &payload, ts, place, settings);
    if repo.is_none() {
        c.event.repo = None;
    }
    c
}

/// A note an agent saves (`save_memory`): its title and text through the same gate as every
/// stored string, a closed opt-out block taken out of each first. Its caller refuses a text over
/// `MAX_FIELD_BYTES`, so the gate cuts none.
pub fn note(title: &str, text: &str, settings: &Settings) -> (String, String) {
    let mut gate = Gate::new(settings);
    let cap = gate.cap;
    (
        gate.text("title", &without_blocks(title, false), cap),
        gate.text("text", &without_blocks(text, false), cap),
    )
}

/// A work state write's list name and fields (docs/work-state.md L5) through the same gate as
/// every stored string, keys and values alike, a closed opt-out block taken out of each first (as
/// labels), each value beside its key, and a list's and a task's name settled as work state folds
/// it (`kept_beside`, #586), a task's number or boolean as its text (#589). Its ledger is not
/// kept: an op has none.
pub fn work_state(
    list: &str,
    fields: &Map<String, Value>,
    settings: &Settings,
) -> (String, Map<String, Value>) {
    let mut gate = Gate::new(settings);
    // Every value beside its key, however long: the fields are at most 2,000 characters (L3).
    gate.paired = usize::MAX;
    let cap = gate.cap;
    let list = kept_beside("list", list, &settings.rules);
    let mut stored = Map::new();
    for (k, v) in fields {
        // A closing status (`done`, `dropped`) is one of two words, no recorded text: kept as
        // sent, or a write meant to close a task or the list would leave it open where a rule
        // masks the word. The line that shows it is gated as any is (Codex on #408).
        if k == "status" && crate::work_state::closed(Some(v)) {
            stored.insert(k.clone(), v.clone());
            continue;
        }
        if let ("task", Value::String(task)) = (k.as_str(), v) {
            stored.insert(
                k.clone(),
                Value::String(kept_beside("task", task, &settings.rules)),
            );
            continue;
        }
        // A number or a boolean names a task as the text it shows as: settled as a string task
        // is, and kept as it is where that text holds (Sourcery on #589).
        if let ("task", Value::Number(_) | Value::Bool(_)) = (k.as_str(), v) {
            let text = crate::work_state::text(v);
            let kept = kept_beside("task", &text, &settings.rules);
            let leaves = |json: &str| kept_beside("task", json, &settings.rules) == json;
            stored.insert(k.clone(), as_written(v, &text, kept, leaves));
            continue;
        }
        // A number or a boolean is text where it is shown: gated as that text, and kept as it is
        // where the gate leaves it (Codex's security review of #408).
        let (v, as_text) = match v {
            Value::String(s) => (Value::String(without_blocks(s, false)), None),
            Value::Number(_) | Value::Bool(_) => {
                let text = crate::work_state::text(v);
                (Value::String(text.clone()), Some((text, v)))
            }
            v => (v.clone(), None),
        };
        let one = Map::from_iter([(without_blocks(k, false), v)]);
        let Value::Object(one) = gate.value("", Value::Object(one), cap) else {
            continue;
        };
        for (key, v) in one {
            // claude-mem's own keys name no recorded text: one a rule matches stays, or a task
            // would fold as the list's state and a status as a field (Codex on #408).
            let key = match k.as_str() {
                "task" | "status" => k.clone(),
                _ => key,
            };
            let v = match (&as_text, v) {
                (Some((text, was)), Value::String(kept)) => {
                    let named = without_blocks(k, false);
                    let leaves = |json: &str| gate.both("", &named, json, cap).0 == json;
                    as_written(was, text, kept, leaves)
                }
                (_, v) => v,
            };
            stored.insert(key, v);
        }
    }
    (list, stored)
}

/// A number or a boolean of a work state write as stored: as written where the gate leaves both
/// `text`, the text it shows as, and its JSON text (`leaves`), so the op keeps the value the tool
/// call sent, which My Work ties it by (`my_work::senders`; Codex on #593); else `kept`, the text
/// the gate saw, so no digit a rule finds in the JSON is stored (a number past a double's digits;
/// #590's review).
fn as_written(was: &Value, text: &str, kept: String, leaves: impl FnOnce(&str) -> bool) -> Value {
    // Its JSON text, not `was == text`: a `Value` equals a `str` only as a string.
    let json = was.to_string();
    if kept == text && (json == text || leaves(&json)) {
        was.clone()
    } else {
        Value::String(kept)
    }
}

/// A name of work state beside its key as `work_state` keeps it now (`Gate::both`), kept again
/// until it holds, since a rule can match what an earlier mask made: what work state tells its
/// lists and tasks apart by and shows them as, so a write before a rule and the one after it,
/// stored as the gate kept it, are one (#580, #582, #584). A name that does not settle within
/// `KEPT_PASSES` is [REDACTED] whole.
pub(crate) fn kept_beside(key: &str, value: &str, rules: &redact::Rules) -> String {
    let settings = Settings {
        rules: rules.clone(),
        ..Settings::default()
    };
    let mut name = without_blocks(value, false);
    for _ in 0..KEPT_PASSES {
        let mut gate = Gate::new(&settings);
        let cap = gate.cap;
        let next = gate.both(key, key, &name, cap).0;
        if next == name {
            return name;
        }
        name = next;
    }
    redact::MASK.to_owned()
}

/// How many times `kept_beside` applies the gate before it gives a name up as [REDACTED].
const KEPT_PASSES: usize = 8;

/// The one gate every stored string passes (spec 2.2): `redact::scan_capped` over its whole
/// length (head and tail above the cap), each finding kept with the field it is in.
struct Gate<'a> {
    rules: &'a redact::Rules,
    /// The cap of every stored string: `MAX_FIELD_BYTES`, unless replay measures another.
    cap: usize,
    /// The cap of a tool's `/output`: `cap`, or `HEAD_TAIL_BYTES` by the settings.
    output_cap: usize,
    /// The longest string value also scanned beside its key (`Gate::both`), in bytes.
    paired: usize,
    ledger: Vec<(String, redact::Finding)>,
    /// The full size of the strings that were cut, when any was.
    cut: Option<i64>,
}

impl<'a> Gate<'a> {
    fn new(settings: &'a Settings) -> Self {
        let cap = field_cap(std::env::var(FIELD_CAP_ENV).ok().as_deref());
        Self {
            rules: &settings.rules,
            cap,
            output_cap: match settings.tool_output {
                crate::config::ToolOutput::Full => cap,
                crate::config::ToolOutput::HeadTail => HEAD_TAIL_BYTES,
            },
            paired: PAIRED,
            ledger: Vec::new(),
            cut: None,
        }
    }

    fn text(&mut self, field: &str, s: &str, cap: usize) -> String {
        let (stored, found, full) = redact::scan_capped(s, cap, self.rules);
        if let Some(n) = full {
            *self.cut.get_or_insert(0) += n as i64;
        }
        self.ledger
            .extend(found.into_iter().map(|f| (field.to_owned(), f)));
        stored
    }

    /// A string `value` beside its `key`, stored as the gate keeps it: scanned alone, as every
    /// string is, and in the assignment `key = "value"`, which the rules that look for a key before
    /// a secret (gitleaks' generic-api-key) need and neither part matches alone (Codex's security
    /// review of #408). Both look at the value as written, so neither takes the context the other
    /// needs; when both hide something, differently, or a mask reaches past the value, the value
    /// is hidden whole, its findings at its start. A mask that reaches into the key hides the key
    /// too (the second of the pair).
    fn both(&mut self, field: &str, key: &str, value: &str, cap: usize) -> (String, bool) {
        let mark = self.ledger.len();
        let alone = self.text(field, value, cap);
        let head = format!("{key} = \"");
        let (masked, found) = redact::scan(&format!("{head}{value}\""), self.rules);
        // Where the first mask starts tells whether a mask reaches into the key: the text before
        // it is the pair's own, where the masked text as a whole can spell the key again (a key
        // holding the mask's text: Codex's security review of #408).
        let Some(first) = found.iter().map(|f| f.offset).min() else {
            return (alone, false);
        };
        let reached = first < head.len();
        let shown = (!reached)
            .then(|| masked[head.len()..].strip_suffix('"'))
            .flatten();
        let (kept, offset) = match shown {
            Some(v) if v == alone => return (alone, false),
            Some(v) if alone == value => (v.to_owned(), head.len()),
            _ => {
                for (_, f) in &mut self.ledger[mark..] {
                    f.offset = 0;
                }
                (redact::MASK.to_owned(), usize::MAX)
            }
        };
        self.ledger.extend(found.into_iter().map(|f| {
            let offset = f.offset.saturating_sub(offset);
            (field.to_owned(), redact::Finding { offset, ..f })
        }));
        (kept, reached)
    }

    /// Every string and key of `v`, at its JSON pointer. Blocks are gone by now: stripping here
    /// again would pair an opener left in one flattened tool field with a closer in another.
    /// A string is kept up to `cap`.
    fn value(&mut self, path: &str, v: Value, cap: usize) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(path, &s, cap)),
            Value::Array(a) => Value::Array(
                a.into_iter()
                    .enumerate()
                    .map(|(i, x)| self.value(&format!("{path}/{i}"), x, self.cap))
                    .collect(),
            ),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, x)| {
                        // The cap follows the key as capture wrote it: a user rule can mask
                        // the stored key `output` itself.
                        let cap = if path.is_empty() && k == "output" {
                            self.output_cap
                        } else {
                            self.cap
                        };
                        // The pointer is built from the stored key, so it never holds a secret.
                        let key = self.text(&format!("{path}#key"), &k, self.cap);
                        match x {
                            // Beside the key as written: a rule that masks the key itself leaves
                            // the context generic-api-key needs, and a mask the pair puts on the
                            // key hides it (Codex's security review of #408).
                            Value::String(s) if s.len() <= self.paired => {
                                let mark = self.ledger.len();
                                let (x, hidden) = self.both("", &k, &s, cap);
                                let key = if hidden { redact::MASK.to_owned() } else { key };
                                let child = format!("{path}/{}", segment(&key));
                                for (field, _) in &mut self.ledger[mark..] {
                                    field.clone_from(&child);
                                }
                                (key, Value::String(x))
                            }
                            x => {
                                let child = format!("{path}/{}", segment(&key));
                                let x = self.value(&child, x, cap);
                                (key, x)
                            }
                        }
                    })
                    .collect(),
            ),
            other => other,
        }
    }
}

/// The longest string value of a captured event scanned beside its key too (`Gate::both`):
/// gitleaks' generic-api-key takes a secret of at most 150 characters. A work state write's are
/// scanned so whatever their length.
const PAIRED: usize = 256;

/// A key as one JSON pointer segment (`~0`, `~1` escaped). A key over 128 bytes is named by
/// `~sha:` and the first 16 hex digits of its sha256 instead: thousands of findings under a huge
/// key would otherwise each copy it into the ledger. `~s` is no valid escape, so the name
/// cannot be mistaken for a key.
fn segment(key: &str) -> String {
    if key.len() > 128 {
        let sha: String = Sha256::digest(key.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return format!("~sha:{sha}");
    }
    key.replace('~', "~0").replace('/', "~1")
}

/// A tool field as text: `clean`, then flattened. `Gate` scans it with the rest of the body.
fn text(v: &Value) -> String {
    compact(&clean(v))
}

/// Slice 2: the payload key with which the viewer's Mark finished marks the stop it sends through
/// the hook's own path as the owner's. No agent sends it.
pub(crate) const OWNER_MARK: &str = "oboete_marked";

/// X17 (docs/subagents.md): a subagent's id or type as claude-mem keeps it (`pickAgentField`: a
/// non-empty string of at most 128 UTF-16 units), bounded again once cleaned: a
/// `<system-reminder>` block is taken out of it as of any text (cubic on #847), and a data URI
/// inside it becomes its longer marker (Sourcery on #847).
fn agent_field(payload: &Value, key: &str) -> Option<Value> {
    let bounded = |s: &str| !s.is_empty() && s.encode_utf16().count() <= 128;
    (payload[key].as_str())
        .filter(|s| bounded(s))
        .map(|s| clean(&json!(s)))
        .filter(|v| v.as_str().is_some_and(bounded))
}

/// Slice 2: what the facts read of a Claude Code `Agent` or `Task` call that started a subagent,
/// kept apart from its text, which may be cut: the subagent's id (the response's `agentId`), the
/// response's `status` and whether it ran async. Its task is no part of it: My Work reads the task
/// from the call's input as it is kept (cleaned and masked whole; an input over the field's cap is
/// kept as its head and tail, which no longer parses and gives no task), through the gate as its
/// rules are now before it cuts it (Codex on slice 2).
fn launched(payload: &Value) -> Option<Value> {
    let tool = str_field(payload, &["tool_name", "toolName", "name"])?;
    if !matches!(tool, "Agent" | "Task") {
        return None;
    }
    let response = field(payload, &["tool_response", "toolResult", "tool_output"]);
    let response = match response {
        Value::String(s) => serde_json::from_str::<Value>(s).ok(),
        Value::Object(_) => Some(response.clone()),
        _ => None,
    }
    .filter(Value::is_object)?;
    let id = agent_field(&response, "agentId")?;
    let status = response["status"].as_str();
    let not_async = !(response["isAsync"] == true
        || response["is_async"] == true
        || response["is_async"] == 1
        || status == Some("async_launched"));
    let mut out = json!({"agent_id": id, "async": !not_async});
    if let Some(status) = status {
        out["status"] = clean(&json!(status));
    }
    Some(out)
}

/// A payload value as stored, string by string, so a block never pairs across two fields: closed
/// `<private>`-style blocks removed without a trim (stored text keeps its whitespace, as v1's
/// `clip` did), and base64 replaced by `{kind, mime, bytes, sha256}` (spec 2.3).
fn clean(v: &Value) -> Value {
    match v {
        Value::Object(m) => binary(m).unwrap_or_else(|| {
            Value::Object(
                m.iter()
                    .map(|(k, x)| (without_blocks(k, false), clean(x)))
                    .collect(),
            )
        }),
        Value::Array(a) => Value::Array(a.iter().map(clean).collect()),
        Value::String(s) => base64_runs(&without_blocks(s, false)),
        other => other.clone(),
    }
}

/// The object shapes seen in agent payloads: a content block `{type, source: {type: base64,
/// media_type, data}}`, Claude Code's image read result `{type: <mime>, base64}`, and MCP's
/// `{type, mimeType, data}`. OpenAI's `image_url` is a data URI string (`base64_runs`).
fn binary(m: &Map<String, Value>) -> Option<Value> {
    let kind = m.get("type").and_then(Value::as_str);
    let (kind, mime, data) = if let Some(src) = m.get("source").filter(|s| s["type"] == "base64") {
        (kind?, src["media_type"].as_str()?, src["data"].as_str()?)
    } else if let Some(data) = m.get("base64").and_then(Value::as_str) {
        let mime = kind?;
        (mime.split('/').next()?, mime, data)
    } else {
        (
            kind?,
            m.get("mimeType")?.as_str()?,
            m.get("data")?.as_str()?,
        )
    };
    Some(marker(kind, mime, data))
}

/// Base64 in a string: a `data:<mime>;base64,` URI (OpenAI's `image_url`, an inline `<img>`), or
/// any run of 1,024 or more base64 characters, whatever shape carried it (the design's length check,
/// docs/research/redesign-2026-09-24/constraints-synthesis.md S2-25). A whole-string match becomes
/// the marker object; one inside other text becomes the marker's JSON.
/// ponytail: line-wrapped base64 (76 characters a line) is not caught; add when one shows up.
fn base64_runs(s: &str) -> Value {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"data:([\w.+-]+/[\w.+-]+);base64,([A-Za-z0-9+/]+=*)|[A-Za-z0-9+/_-]{1024,}={0,2}",
        )
        .expect("base64 pattern")
    });
    // A bare run counts only if it looks like encoded bytes: all three character classes, and a
    // length base64 can have. A long lowercase or hex run, or an identifier, stays text.
    let found = |c: &regex::Captures| -> Option<Value> {
        if let (Some(mime), Some(data)) = (c.get(1), c.get(2)) {
            let mime = mime.as_str();
            return Some(marker(
                mime.split('/').next().unwrap_or(mime),
                mime,
                data.as_str(),
            ));
        }
        let run = c[0].trim_end_matches('=');
        let has = |class: fn(&char) -> bool| run.chars().any(|ch| class(&ch));
        (has(char::is_ascii_uppercase)
            && has(char::is_ascii_lowercase)
            && has(char::is_ascii_digit)
            && run.len() % 4 != 1)
            .then(|| marker("binary", "application/octet-stream", &c[0]))
    };
    match RE.captures(s) {
        Some(c) if c[0].len() == s.len() => {
            found(&c).unwrap_or_else(|| Value::String(s.to_owned()))
        }
        None => Value::String(s.to_owned()),
        Some(_) => Value::String(
            RE.replace_all(s, |c: &regex::Captures| {
                found(c).map_or_else(|| c[0].to_string(), |m| m.to_string())
            })
            .into_owned(),
        ),
    }
}

/// `sha256` is of the base64 text, `bytes` the decoded size. `kind` and `mime` come from the
/// payload, so they are cleaned like any other string.
fn marker(kind: &str, mime: &str, data: &str) -> Value {
    let (kind, mime) = (without_blocks(kind, false), without_blocks(mime, false));
    let sha: String = Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({
        "kind": kind,
        "mime": mime,
        "bytes": data.trim_end_matches('=').len() * 3 / 4,
        "sha256": sha,
    })
}

#[derive(Debug, Default, PartialEq)]
pub struct Git {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub gitdir: Option<String>,
    /// The checkout's top: the directory holding its `.git` (a linked worktree's own).
    pub top: Option<std::path::PathBuf>,
}

/// Branch, HEAD SHA and the checkout's own git directory, read from files (plan D9): `.git` may
/// be a directory or, in a linked worktree, a `gitdir:` file. Refs live in the common directory.
pub fn git(cwd: &Path) -> Git {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let Some(dot_git) = start
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|g| g.exists())
    else {
        return Git::default();
    };
    let top = dot_git.parent().map(Path::to_path_buf);
    let gitdir = if dot_git.is_file() {
        match repo::gitdir(&dot_git) {
            Some(g) => g.canonicalize().unwrap_or(g),
            None => {
                return Git {
                    top,
                    ..Git::default()
                };
            }
        }
    } else {
        dot_git
    };
    let common = repo::common_dir(&gitdir).unwrap_or_else(|| gitdir.clone());
    let head = std::fs::read_to_string(gitdir.join("HEAD")).unwrap_or_default();
    let head = head.trim();
    let (branch, sha) = match head.strip_prefix("ref: ") {
        Some(name) => (
            name.strip_prefix("refs/heads/").map(str::to_owned),
            resolve(&common, name),
        ),
        None => (None, (!head.is_empty()).then(|| head.to_owned())), // detached
    };
    Git {
        branch,
        head: sha,
        gitdir: Some(gitdir.to_string_lossy().into_owned()),
        top,
    }
}

/// A ref's SHA from its loose file, else from `packed-refs`; `None` before the first commit.
fn resolve(common: &Path, name: &str) -> Option<String> {
    if let Ok(s) = std::fs::read_to_string(common.join(name)) {
        return Some(s.trim().to_owned());
    }
    std::fs::read_to_string(common.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|l| {
            let (sha, r) = l.split_once(' ')?;
            (r == name).then(|| sha.to_owned())
        })
}

#[cfg(test)]
mod tests;
