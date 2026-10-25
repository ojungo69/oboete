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
            // (#273): never the developer's words.
            let sent = |mut body: Value| {
                if payload[AGENT_SENT] == true {
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

/// One event of `kind` with `body`, its labels from `payload` and the checkout it names.
fn capture(
    agent: &str,
    kind: &str,
    body: Value,
    payload: &Value,
    ts: i64,
    settings: &Settings,
) -> Captured {
    labelled(agent, kind, body, payload, ts, place(payload), settings)
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
/// event of its own, through the same gate as every stored string, with no repository or
/// checkout: its claim is global, in no repository's current claims (milestone 4's injection
/// reads global ones by scope). `curate` never shows it in a window: its claim is appended with
/// it.
pub fn directive(text: &str, ts: i64, settings: &Settings) -> Captured {
    let payload = json!({"session_id": "owner"});
    // As a typed prompt: an opted-out part is never stored, and binary content is its marker.
    let body = json!({"text": base64_runs(&strip_blocks(text, true)), "scope": "global"});
    let mut c = capture("oboete", "directive", body, &payload, ts, settings);
    c.event.repo = None;
    c.event.cwd = None;
    (c.event.branch, c.event.head, c.event.gitdir) = (None, None, None);
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
mod tests {
    use super::*;

    /// claude-mem 13.35's `isRecursiveMemoryTool`: a read of stored memory keeps its call and not
    /// its output; another server's tool of the same name keeps both.
    #[test]
    fn a_memory_read_is_stored_without_its_output() {
        let s = Settings::load(tempfile::tempdir().unwrap().path()).unwrap();
        let call = |tool: &str| {
            let payload = json!({"session_id": "s", "cwd": "/tmp", "tool_name": tool,
                "tool_input": {"query": "tabs"}, "tool_response": "Use tabs (decided)."});
            let c = &events("claude", "PostToolUse", &payload, 1, &s)[0];
            serde_json::from_str::<Value>(&c.event.body).unwrap()
        };
        let read = call("mcp__oboete__search");
        assert_eq!(
            (&read["output"], &read["memory_read"]),
            (&json!(""), &json!(true))
        );
        assert!(read["input"].as_str().unwrap().contains("tabs"));
        let other = call("mcp__notion__search");
        assert!(other["output"].as_str().unwrap().contains("Use tabs"));
        assert_eq!(other.get("memory_read"), None);
        // A note saved to memory keeps neither: it is kept once, as its document.
        let saved = call("mcp__oboete__save_memory");
        assert_eq!(
            (&saved["input"], &saved["output"], &saved["memory_write"]),
            (&json!(""), &json!(""), &json!(true))
        );
    }

    /// Slice 4a (A28): a checkout's `.oboete.toml` may only stop recording: its `capture` key alone
    /// is read, and a file that cannot be read, is not TOML or holds another `capture` stops it
    /// too, its text never in the error.
    #[test]
    fn a_checkouts_oboete_toml_may_only_stop_recording() {
        let top = tempfile::tempdir().unwrap();
        let file = top.path().join(".oboete.toml");
        let read = |text: Option<&str>| {
            let _ = std::fs::remove_file(&file);
            if let Some(text) = text {
                std::fs::write(&file, text).unwrap();
            }
            repo_capture(Some(top.path()))
        };
        assert!(repo_capture(None).unwrap());
        for (text, on) in [
            (None, true),
            (Some("capture = false\n"), false),
            (Some("capture = true\n"), true),
            (Some("[sync]\nother = 1\n"), true),
        ] {
            assert_eq!(read(text).unwrap(), on, "{text:?}");
        }
        for text in ["capture = \"no-4a\"\n", "capture = [\n"] {
            let e = read(Some(text)).unwrap_err().to_string();
            assert!(e.contains(".oboete.toml") && !e.contains("no-4a"), "{e}");
        }
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(repo_capture(Some(top.path())).is_err());
    }

    /// Security review of #489: why a directory is not recorded is asked as the hook asks it, so a
    /// folder stored as given covers a directory below it under a rule that takes its separator.
    #[test]
    fn why_a_directory_is_not_recorded_reads_its_folder_as_the_hook_does() {
        let top = tempfile::tempdir().unwrap();
        let folder = top.path().join("private");
        let below = folder.join("sub");
        std::fs::create_dir_all(&below).unwrap();
        let mut list = crate::raw::CaptureExclusions::default();
        list.folders.insert(folder_path(&folder.to_string_lossy()));
        let settings = Settings {
            rules: redact::Rules::new(
                &crate::config::parse_capture(Some(
                    "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
                ))
                .unwrap()
                .redaction,
            )
            .unwrap(),
            ..Settings::default()
        };
        let why = not_recorded_why(&below.to_string_lossy(), &list, &settings);
        assert!(why.is_some_and(|w| w.contains("excluded folder")));
    }

    /// Codex on #489: an undo finds a folder stored in another spelling of letter case where the
    /// platform ignores it, as `covers` reads one, and only there; an add finds it the same way.
    #[test]
    fn an_undo_finds_a_folder_in_another_spelling_only_where_case_is_ignored() {
        let (stored, given) = if cfg!(windows) {
            (r"C:\Work\Private", r"c:\work\private")
        } else {
            ("/Users/x/Work/Private", "/users/x/work/private")
        };
        let list = std::collections::BTreeSet::from([stored.to_owned()]);
        let found = undone("cwd", given, &list, &Settings::default()).unwrap();
        let want = if cfg!(any(windows, target_os = "macos")) {
            stored
        } else {
            given
        };
        assert_eq!(found, want);
        // An exclusion added in another spelling keeps that one entry, as the CLI's add takes it.
        let kept = stored_entry("cwd", given, &list).map(String::as_str);
        assert_eq!(kept, (want == stored).then_some(stored));
        assert_eq!(stored_entry("repo", given, &list), None);
    }

    /// Push security review of #489: a checkout's `.oboete.toml` past the limit, or one that names
    /// a device (a link the repository holds), stops recording at once rather than holding the
    /// hook while it reads.
    #[test]
    fn a_checkout_toml_that_is_no_small_file_stops_recording() {
        let top = tempfile::tempdir().unwrap();
        let file = top.path().join(".oboete.toml");
        let big = format!(
            "capture = true\n#{}\n",
            "x".repeat(CHECKOUT_TOML_LIMIT as usize)
        );
        std::fs::write(&file, big).unwrap();
        let e = repo_capture(Some(top.path())).unwrap_err().to_string();
        assert!(e.contains("over"), "{e}");
        std::fs::remove_file(&file).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/dev/zero", &file).unwrap();
            let e = repo_capture(Some(top.path())).unwrap_err().to_string();
            assert!(e.contains("not a regular file"), "{e}");
            // A pipe no one writes to is opened without waiting, and refused (Codex on #489).
            std::fs::remove_file(&file).unwrap();
            let made = std::process::Command::new("mkfifo").arg(&file).status();
            assert!(made.unwrap().success());
            let e = repo_capture(Some(top.path())).unwrap_err().to_string();
            assert!(e.contains("not a regular file"), "{e}");
        }
    }

    /// Slice 4a: a checkout whose `.oboete.toml` stops recording, or cannot be read, gets no event,
    /// whatever its kind; `capture = true` records as before.
    #[test]
    fn a_checkout_that_stops_recording_gets_no_event() {
        let top = tempfile::tempdir().unwrap();
        std::fs::create_dir(top.path().join(".git")).unwrap();
        let cwd = top.path().join("src");
        std::fs::create_dir(&cwd).unwrap();
        let payload = json!({"session_id": "s", "cwd": cwd, "prompt": "canary-4a"});
        let n = || {
            events(
                "claude",
                "UserPromptSubmit",
                &payload,
                7,
                &Settings::default(),
            )
            .len()
        };
        assert_eq!(n(), 1);
        for (text, want) in [
            ("capture = false\n", 0),
            ("capture =\n", 0),
            ("capture = true\n", 1),
        ] {
            std::fs::write(top.path().join(".oboete.toml"), text).unwrap();
            assert_eq!(n(), want, "{text}");
        }
    }

    /// Parity row 14: claude-mem's patterns name folders as its `project-filter.ts` reads them,
    /// and a name with no `/` names the folders inside one so named too.
    #[test]
    fn a_pattern_names_folders_as_claude_mems_does() {
        for (pattern, cwd, named) in [
            ("secret", "/work/secret", true),
            ("secret", "/work/secret/src", true),
            ("secret", "/work/secrets", false),
            ("sec*", "/work/secret", true),
            ("sec?et", "/work/secret", true),
            ("sec?et", "/work/sec/et", false),
            ("/work/*/private", "/work/a/private", true),
            ("/work/*/private", "/work/a/b/private", false),
            ("/work/**/private", "/work/a/b/private", true),
            // A whole `**/` folder part also names no folder, as claude-mem 15.0 reads it.
            ("/work/**/private", "/work/private", true),
            ("/**/private", "/private", true),
            ("/work/**/private", "/work/privacy", false),
            ("/work/**", "/work", true),
            ("/work/**", "/work/a/b", true),
            ("/work/**", "/other", false),
            (r"\work\secret", "/work/secret/", true),
            ("/", "/", true),
            ("C:/", "C:/", true),
            ("/", "/work", false),
            ("C:/", "C:/work", false),
            ("  secret ", "/work/secret", true),
            ("", "/work/secret", false),
            // A pattern that does not start at a root names the end of a path (Codex on #563).
            ("secret/**", "/work/secret", true),
            ("secret/**", "/work/secret/src", true),
            ("secret/**", "/work/secrets", false),
            ("a/b/**", "/x/a/b", true),
            ("a/b/**", "/x/a/b/c", true),
            ("a/b", "/x/a/b", true),
            ("a/b", "/x/a/b/c", false),
            ("a/b", "/xa/b", false),
            ("*/private", "/work/a/private", true),
            ("/a/b", "/x/a/b", false),
            ("C:/a/b", "/x/C:/a/b", false),
            ("/server/share", "//server/share", false),
            ("/**", "C:/", false),
            ("/**", "C:/work", false),
        ] {
            assert_eq!(pattern_covers(pattern, cwd), named, "{pattern:?} {cwd}");
        }
        let home = crate::config::home_dir()
            .to_string_lossy()
            .replace('\\', "/");
        let home = home.trim_end_matches('/');
        assert!(pattern_covers("~/secret", &format!("{home}/secret")));
        assert!(pattern_covers("~/**", &format!("{home}/a/b")));
        assert!(!pattern_covers("~/secret", "/elsewhere/secret"));
        // A home that is a root: `~` names that root alone (Codex on #563).
        for (pattern, home, want) in [
            ("~", "/", "/"),
            ("~", "C:/", "C:/"),
            ("~", "/home/u/", "/home/u"),
            ("~/x", "/", "/x"),
            ("~/x", "C:/", "C:/x"),
            ("~x", "/home/u", "~x"),
        ] {
            assert_eq!(expanded(pattern.into(), home), want, "{pattern} {home}");
        }
        assert!(pattern("  a b ").is_ok_and(|p| p == "a b"));
        assert!(pattern(" ").is_err() && pattern("a\nb").is_err());
        assert!(pattern(&"a".repeat(256)).is_ok() && pattern(&"a".repeat(257)).is_err());
    }

    /// Codex on #489: a folder reads alike however it was written: `.` and `..` by name, a
    /// trailing separator dropped, a relative path against the current directory.
    #[test]
    fn a_folder_reads_alike_however_it_was_written() {
        let here = std::env::current_dir().unwrap();
        let private = here
            .join("x")
            .join("private")
            .to_string_lossy()
            .into_owned();
        let sep = std::path::MAIN_SEPARATOR;
        for written in [
            here.join("x").join("open").join("..").join("private"),
            here.join("x").join(".").join("private"),
        ] {
            assert_eq!(folder_path(&written.to_string_lossy()), private);
        }
        assert_eq!(folder_path(&format!("{private}{sep}")), private);
        assert_eq!(folder_path("x/open/../private"), private);
    }

    /// Slice 4a: why a directory is not recorded, as doctor and the work state say it: its
    /// checkout's `.oboete.toml`, its repository excluded, or a folder around it; nothing when it is
    /// recorded.
    #[test]
    fn why_a_directory_is_not_recorded() {
        let top = tempfile::tempdir().unwrap();
        std::fs::create_dir(top.path().join(".git")).unwrap();
        let dir = top.path().join("deep");
        std::fs::create_dir(&dir).unwrap();
        let (dir, settings) = (dir.to_string_lossy().into_owned(), Settings::default());
        let mut list = crate::raw::CaptureExclusions::default();
        assert_eq!(not_recorded_why(&dir, &list, &settings), None);
        std::fs::write(top.path().join(".oboete.toml"), "capture = false\n").unwrap();
        let why = not_recorded_why(&dir, &list, &settings).unwrap();
        assert!(why.contains("capture = false"), "{why}");
        std::fs::remove_file(top.path().join(".oboete.toml")).unwrap();
        list.folders
            .insert(top.path().to_string_lossy().into_owned());
        let why = not_recorded_why(&dir, &list, &settings).unwrap();
        assert!(why.contains("excluded folder"), "{why}");
        list.folders.clear();
        list.patterns.insert("deep".into());
        let why = not_recorded_why(&dir, &list, &settings).unwrap();
        assert!(why.contains("excluded pattern"), "{why}");
        list.patterns.clear();
        list.repos.insert(repo::key(top.path()));
        let why = not_recorded_why(&dir, &list, &settings).unwrap();
        assert!(why.contains("its repository"), "{why}");
    }

    /// Slice 4a: an exclusion's values as events labelled now hold them, through today's rules: a
    /// rule added after a folder was excluded masks it as it masks the cwd of an event in it.
    #[test]
    fn exclusions_are_labelled_as_events_are_now() {
        let home = tempfile::tempdir().unwrap();
        let folder = "/w/acme-482915/private";
        let mut list = crate::raw::CaptureExclusions::default();
        list.folders.insert(folder.into());
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = Settings::load(home.path()).unwrap();
        let cwd = exclusion_label("cwd", &format!("{folder}/sub"), &settings);
        assert!(!cwd.contains("acme-482915"));
        assert!(!list.covers(None, Some(&cwd)));
        assert!(never_recorded(&list, &settings).covers(None, Some(&cwd)));
    }

    fn one(event: &str, payload: Value) -> Event {
        let mut v = events("claude", event, &payload, 7, &Settings::default());
        assert_eq!(v.len(), 1, "{event}: {v:?}");
        v.remove(0).event
    }

    fn body(e: &Event) -> Value {
        serde_json::from_str(&e.body).unwrap()
    }

    #[test]
    fn replay_can_measure_a_cap_candidate_but_not_outside_their_range() {
        assert_eq!(field_cap(None), MAX_FIELD_BYTES);
        assert_eq!(field_cap(Some("262144")), 256 * 1024);
        assert_eq!(field_cap(Some("131072")), 128 * 1024);
        for v in ["262145", "1023", "0", "-1", "lots"] {
            assert_eq!(field_cap(Some(v)), MAX_FIELD_BYTES, "{v}");
        }
    }

    #[test]
    fn a_long_tool_output_is_kept_whole_and_redacted_past_the_old_window() {
        let key = &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split, as in import.rs, so secret scanners pass it
        // 50 KB: past v1's 12,000-character window, within `MAX_FIELD_BYTES`.
        let out = "word ".repeat(10_000) + " Authorization: Bearer " + key;
        let e = one(
            "PostToolUse",
            json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                   "tool_input": {"command": "ls"}, "tool_response": {"stdout": out}}),
        );
        assert_eq!(e.kind, "tool");
        let b = body(&e);
        assert!(
            b["output"]
                .as_str()
                .unwrap()
                .contains(&"word ".repeat(10_000))
        );
        assert!(
            !e.body.contains(key),
            "a secret past today's 12,000-character window"
        );
        assert_eq!(b["input"], r#"{"command":"ls"}"#);
        assert_eq!(b["failed"], false);
        assert_eq!(
            (e.session.as_str(), e.ts, e.source.as_str()),
            ("s", 7, "hook")
        );
    }

    #[test]
    fn prompts_lose_private_blocks_and_envelopes_are_not_typed_prompts() {
        let e = one(
            "UserPromptSubmit",
            json!({"prompt": "keep <private>secret plan</private> this"}),
        );
        assert_eq!(
            (e.kind.as_str(), body(&e)["prompt"].as_str()),
            ("prompt", Some("keep  this"))
        );
        let e = one(
            "UserPromptSubmit",
            json!({"prompt": "shown <private>and the rest is hidden"}),
        );
        assert_eq!(body(&e)["prompt"], "shown");
        assert!(
            events(
                "claude",
                "UserPromptSubmit",
                &json!({"prompt": "<private>x"}),
                0,
                &Settings::default()
            )
            .is_empty()
        );
        for envelope in [
            "<task-notification>done</task-notification>",
            "Another Claude session sent a message: <teammate-message from=\"a\">hi</teammate-message>",
        ] {
            let e = one("UserPromptSubmit", json!({"prompt": envelope}));
            assert_eq!(e.kind, "envelope", "{envelope}");
        }
        assert_eq!(
            one("UserPromptSubmit", json!({"prompt": "hi"})).session,
            "unknown"
        );
    }

    #[test]
    fn a_block_never_pairs_across_two_fields() {
        let out = json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "<private>gone</private>!"});
        let e = one(
            "PostToolUse",
            json!({"tool_name": "t", "tool_input": {}, "tool_response": out}),
        );
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            stored,
            json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "!"})
        );
    }

    #[test]
    fn the_ledger_names_the_field_and_the_mask_never_the_value() {
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {"command": format!("curl -H 'Authorization: Bearer {key}'")},
            "tool_response": {"stdout": "ok"}, "reason": {format!("Bearer {key}"): 1}});
        let v = events("claude", "PostToolUse", &payload, 0, &Settings::default());
        let c = &v[0];
        assert!(!format!("{c:?}").contains(&key), "{c:?}");
        let (field, f) = &c.ledger[0];
        assert_eq!((field.as_str(), f.length), ("/input", key.len()));
        let input = body(&c.event)["input"].as_str().unwrap().to_owned();
        assert_eq!(
            &input[f.offset..f.offset + "[REDACTED]".len()],
            "[REDACTED]"
        );
        // A key names its object, from the stored key: the pointer never holds the secret.
        let end = events(
            "claude",
            "SessionEnd",
            &json!({"reason": {format!("Bearer {key}"): 1}}),
            0,
            &Settings::default(),
        );
        assert!(
            end[0]
                .ledger
                .iter()
                .any(|(field, _)| field == "/reason#key"),
            "{:?}",
            end[0].ledger
        );
        assert!(!format!("{:?}", end[0]).contains(&key));
    }

    /// Codex's security review of #408: a rule that looks for a key before a secret (gitleaks'
    /// generic-api-key) matches a string beside its key, the ledger offset in the stored string.
    #[test]
    fn a_value_is_scanned_with_its_key() {
        let secret = format!("R8m2V5p9{}", "Q1s4H7c0N6x3");
        let payload = json!({"reason": {"api_key": format!("  {secret}"), "note": "violet"}});
        let v = events("claude", "SessionEnd", &payload, 0, &Settings::default());
        assert!(!format!("{:?}", v[0]).contains(&secret), "{:?}", v[0]);
        let (field, f) = &v[0].ledger[0];
        assert_eq!((field.as_str(), f.offset), ("/reason/api_key", 2));
        assert_eq!(body(&v[0].event)["reason"]["api_key"], "  [REDACTED]");
        assert_eq!(body(&v[0].event)["reason"]["note"], "violet");
        // A mask that reaches past the value masks the value whole, and one that reaches into the
        // key the key too, its ledger pointer with it.
        let s = with("[[redaction.extra_rules]]\nid = \"v\"\nregex = 'note = \"violet\"'\n");
        let v = events("claude", "SessionEnd", &payload, 0, &s);
        assert!(body(&v[0].event)["reason"].get("note").is_none());
        assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
        assert!(
            v[0].ledger
                .iter()
                .any(|(field, f)| field == "/reason/[REDACTED]" && f.offset == 0)
        );
        // Each scan looks at the value as written: a rule anchored to the whole value still finds
        // what follows the secret that generic-api-key masks beside its key, and the value two
        // different masks hide is hidden whole.
        let s = with(
            "[[redaction.extra_rules]]\nid = \"t\"\nregex = '^[A-Za-z0-9]{20} (teal-[0-9]{4})$'\n\
             secret_group = 1\n",
        );
        let payload = json!({"reason": {"api_key": format!("{secret} teal-1234")}});
        let v = events("claude", "SessionEnd", &payload, 0, &s);
        assert_eq!(body(&v[0].event)["reason"]["api_key"], "[REDACTED]");
        assert!(v[0].ledger.len() >= 2 && v[0].ledger.iter().all(|(_, f)| f.offset == 0));
        // A mask the pair puts on the key hides the key, its pointer in the ledger too.
        let s = with(
            "[[redaction.extra_rules]]\nid = \"p\"\nregex = '^(teal-[0-9]{4}) = \"private\"$'\n\
             secret_group = 1\n",
        );
        let payload = json!({"reason": {"teal-1234": "private"}});
        let v = events("claude", "SessionEnd", &payload, 0, &s);
        assert!(!format!("{:?}", v[0]).contains("teal-1234"), "{:?}", v[0]);
        assert!(body(&v[0].event)["reason"].get("[REDACTED]").is_some());
        // Where the mask starts tells whether it reaches the key: the masked pair can spell the
        // key again when the key holds the mask's own text (Codex's security review of #408).
        let s = with(
            "[[redaction.extra_rules]]\nid = \"s\"\n\
             regex = '^(\\[REDACTED\\]teal-[0-9]{4} = \")teal-[0-9]{4} = \"private\"$'\n\
             secret_group = 1\n",
        );
        let payload = json!({"reason": {"[REDACTED]teal-1234": "teal-1234 = \"private"}});
        let v = events("claude", "SessionEnd", &payload, 0, &s);
        assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
        // A rule that masks the key leaves the value's scan beside the key as written.
        let s = with("[[redaction.extra_rules]]\nid = \"k\"\nregex = '^api_key$'\n");
        let payload = json!({"reason": {"api_key": secret.clone()}});
        let v = events("claude", "SessionEnd", &payload, 0, &s);
        assert!(!format!("{:?}", v[0]).contains(&secret), "{:?}", v[0]);
        assert_eq!(body(&v[0].event)["reason"]["[REDACTED]"], "[REDACTED]");
    }

    /// Codex's security review of #408: a number or a boolean is gated as the text it shows as,
    /// and kept as it is where the gate leaves that text.
    #[test]
    fn a_work_state_number_is_gated_as_its_text() {
        let fields = json!({"authorization": 7_319_462_805u64, "pr": 7, "draft": true});
        let (_, stored) = work_state("l", fields.as_object().unwrap(), &Settings::default());
        let stored = Value::Object(stored);
        assert!(!stored.to_string().contains("7319462805"), "{stored}");
        assert_eq!((&stored["pr"], &stored["draft"]), (&json!(7), &json!(true)));
    }

    /// Codex on #408: claude-mem's keys stay where a rule matches them, their values gated, and a
    /// closing status stays where a rule matches the word.
    #[test]
    fn a_work_state_write_keeps_task_and_status() {
        let s = with(
            "[[redaction.extra_rules]]\nid = \"k\"\nregex = '^(?:task|status|note)$'\n\n\
             [[redaction.extra_rules]]\nid = \"v\"\nregex = 'teal-[0-9]{4}|(?i)done|wip'\n",
        );
        let fields = json!({"task": "teal-1234", "status": "Done", "note": "x"});
        let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
        assert_eq!(
            Value::Object(stored),
            json!({"task": "[REDACTED]", "status": "Done", "[REDACTED]": "x"})
        );
        let open = json!({"task": "t", "status": "wip"});
        let (_, stored) = work_state("l", open.as_object().unwrap(), &s);
        assert_eq!(stored["status"], "[REDACTED]");
    }

    /// A list's and a task's name are stored settled, as the fold reads them (`kept_beside`), so a
    /// write and its stored form fold as one whatever the passes (Sourcery on #585).
    #[test]
    fn a_work_state_name_is_stored_settled() {
        for key in ["list", "task"] {
            let s = with(&format!(
                "[[redaction.extra_rules]]\nid = \"a\"\nregex = '^\\[REDACTED\\] (teal-[0-9]{{4}})'\n\
                 secret_group = 1\n\n[[redaction.extra_rules]]\nid = \"k\"\n\
                 regex = '{key} = \"(alpha)'\nsecret_group = 1\n"
            ));
            let fields = json!({"task": "alpha teal-1234"});
            let (list, stored) = work_state("alpha teal-1234", fields.as_object().unwrap(), &s);
            let name = if key == "list" {
                list
            } else {
                stored["task"].as_str().unwrap().to_owned()
            };
            assert_eq!(name, "[REDACTED] [REDACTED]", "{key}");
            assert_eq!(kept_beside(key, &name, &s.rules), name, "{key}");
        }
    }

    /// A number or a boolean names a task as the text it shows as: stored as that text settles, as
    /// a string task is, and kept as it is where the gate leaves it (Sourcery on #589).
    #[test]
    fn a_numeric_or_boolean_task_is_stored_settled() {
        for (task, head, tail) in [(json!(1234), "12", "34"), (json!(true), "tr", "ue")] {
            let s = with(&format!(
                "[[redaction.extra_rules]]\nid = \"a\"\nregex = '^\\[REDACTED\\]({tail})'\n\
                 secret_group = 1\n\n[[redaction.extra_rules]]\nid = \"k\"\n\
                 regex = 'task = \"({head})'\nsecret_group = 1\n"
            ));
            let fields = json!({ "task": task });
            let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
            assert_eq!(stored["task"], "[REDACTED][REDACTED]", "{task}");
            assert_eq!(
                kept_beside("task", "[REDACTED][REDACTED]", &s.rules),
                "[REDACTED][REDACTED]"
            );
            let (_, kept) = work_state("l", fields.as_object().unwrap(), &with(""));
            assert_eq!(kept["task"], task);
        }
    }

    /// A number a rule finds in its JSON is stored as the text it shows as, the text the gate saw,
    /// a task's and any field's, so no digit the gate masks is kept (#590's review); every other
    /// number is kept as the tool call sent it, which My Work ties the write by (Codex on #593).
    #[test]
    fn a_number_is_stored_as_sent_unless_a_rule_finds_its_digits() {
        let s = with("[[redaction.extra_rules]]\nid = \"n\"\nregex = '1234567890123456789'\n");
        let fields =
            json!({"task": 1_234_567_890_123_456_789_u64, "n": 1_234_567_890_123_456_789_u64});
        let (_, stored) = work_state("l", fields.as_object().unwrap(), &s);
        let op = serde_json::to_string(&stored).unwrap();
        assert!(!op.contains("1234567890123456789"), "{op}");
        assert_eq!(stored["task"], "1234567890123456800");
        assert_eq!(stored["n"], "1234567890123456800");
        let sent = json!({"task": 100.0, "w": 100.0, "big": 9_007_199_254_740_993_u64,
            "e": 1e21, "m": 5, "f": 0.5, "b": false});
        let (_, stored) = work_state("l", sent.as_object().unwrap(), &s);
        assert_eq!(&Value::Object(stored), &sent);
    }

    /// A list's name is gated beside its key `list`, as a field's value is beside its own: a rule
    /// that needs the key masks it in the op.
    #[test]
    fn a_work_state_list_name_is_gated_beside_its_key() {
        let s = with("[[redaction.extra_rules]]\nid = \"l\"\nregex = 'list = \"canary-[0-9]{4}'\n");
        let fields = json!({"task": "rotate"});
        let (list, _) = work_state("canary-4827", fields.as_object().unwrap(), &s);
        assert_eq!(list, crate::redact::MASK);
    }

    #[test]
    fn secrets_behind_json_escapes_in_tool_fields_are_masked() {
        // A tool field is stored as flattened JSON: its quotes and line breaks become `\"`, `\n`.
        let b64 = format!("dXNyOnE5Wng4{}", "bUwydkI0blI3dFl3");
        let weak = format!("MTExMTExMTE6{}", "MTExMTExMTE="); // 11111111:11111111
        let short = format!("YT{}", "pi"); // a:b
        let pass = format!("q9Zx8mL2{}", "vB4nR7tY1wK3");
        let api = format!("k7Fq2Lp9{}", "Xw3Rt8Vn5Bz1Mc6D");
        let cases = [
            // Bash stdout holding JSON (an API's answer, a config file)
            (
                json!({"command": "cat cfg.json"}),
                json!({"stdout": format!("{{\"Authorization\":\"Basic {b64}\"}}"), "stderr": ""}),
                &b64,
            ),
            // A weak credential after "Authorization: Basic" is still one
            (
                json!({"command": "cat headers.txt"}),
                json!({"stdout": format!("Authorization: Basic {weak}\n")}),
                &weak,
            ),
            (
                json!({"command": "cat headers.txt"}),
                json!({"stdout": format!("Authorization: Basic {short}\n")}),
                &short,
            ),
            // curl's -u and -H in double quotes
            (
                json!({"command": format!("curl -u \"admin:{pass}\" https://x.invalid/")}),
                json!({"stdout": "ok"}),
                &pass,
            ),
            (
                json!({"command": format!("curl -H \"X-Api-Key: {api}\" https://x.invalid/")}),
                json!({"stdout": "ok"}),
                &api,
            ),
            // A key and its value side by side: why tool fields stay flattened
            (
                json!({"api_key": api.clone(), "q": "x"}),
                json!({"stdout": "ok"}),
                &api,
            ),
        ];
        for (input, output, secret) in cases {
            let payload = json!({"session_id": "s", "tool_name": "Bash",
                "tool_input": input, "tool_response": output});
            let c = &events("claude", "PostToolUse", &payload, 0, &Settings::default())[0];
            assert!(
                !format!("{c:?}").contains(secret.as_str()),
                "{}",
                c.event.body
            );
            assert!(!c.ledger.is_empty());
            let b = body(&c.event);
            for (field, f) in &c.ledger {
                let text = b[&field[1..]].as_str().unwrap();
                assert_eq!(&text[f.offset..f.offset + "[REDACTED]".len()], "[REDACTED]");
            }
        }
    }

    #[test]
    fn a_huge_key_is_named_by_its_hash_in_the_ledger() {
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let big = "k".repeat(250_000);
        let inner: Map<String, Value> = (0..50).map(|i| (format!("n{i}"), json!(key))).collect();
        let e = &events(
            "claude",
            "SessionEnd",
            &json!({"reason": {big.clone(): inner}}),
            0,
            &Settings::default(),
        )[0];
        assert_eq!(e.ledger.len(), 50);
        for (field, _) in &e.ledger {
            assert!(
                field.starts_with("/reason/~sha:") && field.len() < 64,
                "{}",
                &field[..60.min(field.len())]
            );
        }
        assert_eq!(segment("a/b~c"), "a~1b~0c");
    }

    fn with(toml: &str) -> Settings {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), toml).unwrap();
        Settings::load(home.path()).unwrap()
    }

    #[test]
    fn with_prompts_off_the_turn_is_recorded_without_its_text() {
        let s = with("[capture]\nstore_prompts = false\n");
        let payload = json!({"session_id": "s", "prompt": "the zebra plan"});
        let v = events("claude", "UserPromptSubmit", &payload, 0, &s);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].event.kind, "prompt");
        assert_eq!(body(&v[0].event), json!({"omitted": true}));
        assert!(!format!("{:?}", v[0]).contains("zebra"));
        // A turn that was all private is none, as with prompts on.
        let private = json!({"session_id": "s", "prompt": "<private>zebra</private>"});
        assert!(events("claude", "UserPromptSubmit", &private, 0, &s).is_empty());
        let on = events(
            "claude",
            "UserPromptSubmit",
            &private,
            0,
            &Settings::default(),
        );
        assert!(on.is_empty());
    }

    #[test]
    fn head_tail_cuts_a_tool_output_and_leaves_its_input_whole() {
        let s = with("[capture]\ntool_output = \"head-tail\"\n");
        let big = "a".repeat(HEAD_TAIL_BYTES) + &"b".repeat(HEAD_TAIL_BYTES);
        let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {"cmd": big}, "tool_response": big});
        let e = &events("claude", "PostToolUse", &payload, 0, &s)[0].event;
        let b = body(e);
        let out = b["output"].as_str().unwrap();
        assert!(out.len() < HEAD_TAIL_BYTES + 100, "{}", out.len());
        assert!(out.starts_with('a') && out.ends_with('b') && out.contains("[cut: "));
        assert!(b["input"].as_str().unwrap().contains(&big));
    }

    #[test]
    fn head_tail_holds_when_a_rule_masks_the_key_output() {
        let s = with(
            "[capture]\ntool_output = \"head-tail\"\n[[redaction.extra_rules]]\nid = \"o\"\nregex = \"output\"\n",
        );
        let big = "a".repeat(HEAD_TAIL_BYTES) + &"b".repeat(HEAD_TAIL_BYTES);
        let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {}, "tool_response": big});
        let b = body(&events("claude", "PostToolUse", &payload, 0, &s)[0].event);
        let (key, out) = b
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, v)| v.as_str().is_some_and(|v| v.starts_with('a')))
            .unwrap();
        assert_ne!(key, "output");
        assert!(out.as_str().unwrap().len() < HEAD_TAIL_BYTES + 100);
    }

    #[test]
    fn an_oversized_field_keeps_head_and_tail_and_its_full_size() {
        let out = "a".repeat(MAX_FIELD_BYTES) + &"b".repeat(MAX_FIELD_BYTES);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": out}),
        );
        let stored = body(&e)["output"].as_str().unwrap().to_owned();
        assert!(stored.len() < MAX_FIELD_BYTES + 100, "{}", stored.len());
        assert!(stored.starts_with('a') && stored.ends_with('b'));
        assert!(stored.contains(&format!("[cut: {} bytes in full]", out.len())));
        assert_eq!(e.original_bytes, Some(out.len() as i64));
        let small = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": "x"}),
        );
        assert_eq!(small.original_bytes, None);
    }

    #[test]
    fn a_tool_call_keeps_its_subagent_and_interruption() {
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": null,
                   "agent_id": "a1", "interrupted": true}),
        );
        assert_eq!(
            (&body(&e)["agent_id"], &body(&e)["interrupted"]),
            (&json!("a1"), &json!(true))
        );
        let plain = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}}),
        );
        assert!(
            body(&plain).get("agent_id").is_none() && body(&plain).get("interrupted").is_none()
        );
    }

    #[test]
    fn stored_text_keeps_its_whitespace() {
        let out = "\n    indented line\n\t";
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Read", "tool_input": " a ", "tool_response": out}),
        );
        assert_eq!(
            (body(&e)["output"].as_str(), body(&e)["input"].as_str()),
            (Some(out), Some(" a "))
        );
        let e = one(
            "Stop",
            json!({"last_assistant_message": "  code:\n    x = 1\n"}),
        );
        assert_eq!(body(&e)["assistant"], "  code:\n    x = 1\n");
    }

    #[test]
    fn private_blocks_leave_every_text_field() {
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "echo <private>zqx-in</private>"},
                   "tool_response": {"stdout": "a <private>zqx-out</private> b"}}),
        );
        assert!(
            !e.body.contains("zqx") && !e.body.contains("private"),
            "{}",
            e.body
        );
        let e = one(
            "Stop",
            json!({"last_assistant_message": "ok <private>reply</private>"}),
        );
        assert_eq!(body(&e)["assistant"], "ok "); // whitespace is kept, the block is not
        let e = one(
            "PostCompact",
            json!({"compact_summary": "sum <private>mary</private>"}),
        );
        assert_eq!(body(&e)["summary"], "sum ");
        let only_private = json!({"last_assistant_message": "<private>all</private>"});
        assert!(events("claude", "Stop", &only_private, 0, &Settings::default()).is_empty());
    }

    #[test]
    fn every_string_in_the_body_passes_the_gate() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let bearer = format!("Authorization: Bearer {token} <private>zqx-opt-out</private>");
        for (event, payload) in [
            (
                "PostToolUse",
                json!({"tool_name": bearer, "tool_input": {}, "tool_response": ""}),
            ),
            ("SessionStart", json!({"source": bearer})),
            ("PreCompact", json!({"trigger": {"nested": [bearer]}})),
            ("SessionEnd", json!({"reason": bearer})),
            ("UserPromptSubmit", json!({"prompt": bearer})),
            ("Stop", json!({"last_assistant_message": bearer})),
            ("PostCompact", json!({"compact_summary": bearer})),
        ] {
            let e = one(event, payload);
            assert!(
                !e.body.contains(&token)
                    && e.body.contains("[REDACTED]")
                    && !e.body.contains("zqx"),
                "{event}: {}",
                e.body
            );
        }
    }

    #[test]
    fn labels_are_redacted_too() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join(format!("Bearer {token}"));
        std::fs::create_dir(&cwd).unwrap();
        let e = one(
            "SessionStart",
            json!({"session_id": format!("Authorization: Bearer {token}"), "cwd": cwd}),
        );
        for field in [Some(&e.session), e.cwd.as_ref(), e.repo.as_ref()] {
            let field = field.unwrap();
            assert!(
                !field.contains(&token) && field.contains("[REDACTED]"),
                "{field}"
            );
        }
    }

    #[test]
    fn images_become_markers_in_each_shape_seen() {
        let data = "iVBORw0KGgo=";
        let out = json!([
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
            {"type": "image/png", "base64": data, "dimensions": {}},
            {"type": "image", "mimeType": "image/jpeg", "data": data},
            {"type": "text", "text": "caption"},
        ]);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Read", "tool_input": {}, "tool_response": out}),
        );
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert!(!e.body.contains(data));
        for (i, mime) in ["image/png", "image/png", "image/jpeg"].iter().enumerate() {
            assert_eq!(stored[i]["kind"], "image");
            assert_eq!(stored[i]["mime"], *mime);
            assert_eq!(stored[i]["bytes"], 8);
            assert_eq!(stored[i]["sha256"].as_str().unwrap().len(), 64);
        }
        assert_eq!(stored[3]["text"], "caption");
        let tagged =
            json!({"type": "image", "mimeType": "image/<private>zqx</private>png", "data": data});
        let e = one(
            "PostToolUse",
            json!({"tool_name": "t", "tool_input": {}, "tool_response": tagged}),
        );
        assert!(!e.body.contains("zqx"), "{}", e.body);
    }

    #[test]
    fn long_base64_in_any_shape_becomes_a_marker() {
        let blob = "iVBORw0KGgoAAAANSUhEUg".repeat(50); // 1,100 characters
        let e = one(
            "PostToolUse",
            json!({"tool_name": "mcp__x", "tool_input": {},
                   "tool_response": {"encoding": "base64", "content": blob, "note": format!("saved {blob}.")}}),
        );
        assert!(!e.body.contains("SUhEUgiVBOR"), "{}", e.body);
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(stored["content"]["kind"], "binary");
        assert_eq!(stored["content"]["bytes"], 825);
        assert!(
            stored["note"]
                .as_str()
                .unwrap()
                .starts_with("saved {\"kind\":\"binary\"")
        );
        // Text that only looks like it stays: short runs (a hash, a key id), runs missing a character
        // class (a long lowercase or hex run), and a length base64 cannot have (1,025).
        for kept in [
            "a1B2".repeat(64),
            "x".repeat(2_000),
            "0f".repeat(600),
            "a1B".repeat(341) + "cd",
        ] {
            let e = one(
                "PostToolUse",
                json!({"tool_name": "x", "tool_input": {}, "tool_response": kept}),
            );
            assert!(e.body.contains(&kept), "{}", kept.len());
        }
        // Prompts, replies and summaries get the same pass as tool fields.
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        for (event, payload) in [
            ("UserPromptSubmit", json!({"prompt": format!("see {uri}")})),
            ("Stop", json!({"last_assistant_message": blob})),
            (
                "PostCompact",
                json!({"compact_summary": format!("{uri} and {blob}")}),
            ),
        ] {
            let e = one(event, payload);
            assert!(!e.body.contains("iVBORw0KGgo"), "{event}: {}", e.body);
        }
    }

    #[test]
    fn data_uris_become_markers_whole_or_inside_text() {
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        let out = json!([
            {"type": "input_image", "image_url": uri},
            {"type": "image_url", "image_url": {"url": uri}},
            format!("<img src=\"{uri}\"> after"),
        ]);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "view_image", "tool_input": {}, "tool_response": out}),
        );
        assert!(!e.body.contains("iVBORw0KGgo"), "{}", e.body);
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(stored[0]["image_url"]["mime"], "image/png");
        assert_eq!(stored[1]["image_url"]["url"]["bytes"], 8);
        let inline = stored[2].as_str().unwrap();
        assert!(
            inline.starts_with("<img src=\"{\"kind\":\"image\"") && inline.ends_with("\"> after"),
            "{inline}"
        );
    }

    #[test]
    fn replies_come_from_the_payload_or_the_codex_transcript() {
        let e = one("Stop", json!({"last_assistant_message": "done"}));
        assert_eq!(
            (e.kind.as_str(), body(&e)["assistant"].as_str()),
            ("reply", Some("done"))
        );
        let dir = tempfile::tempdir().unwrap();
        let rollout = dir.path().join("rollout.jsonl");
        std::fs::write(
            &rollout,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"from codex"}]}}"#,
        )
        .unwrap();
        let v = events(
            "codex",
            "Stop",
            &json!({"transcript_path": rollout}),
            0,
            &Settings::default(),
        );
        assert_eq!(body(&v[0].event)["assistant"], "from codex");
        assert!(
            events(
                "claude",
                "Stop",
                &json!({"last_assistant_message": " "}),
                0,
                &Settings::default()
            )
            .is_empty()
        );
        // Claude Code's empty message at a session that ended in a tool call: its transcript's
        // last text, never a subagent's (parity row 17).
        let transcript = dir.path().join("session.jsonl");
        let lines = [
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "text", "text": "Tests pass."}, {"type": "text", "text": "Pushed."}]}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "name": "Bash", "input": {}}]}}),
            json!({"type": "assistant", "isSidechain": true, "message": {"role": "assistant",
                "content": [{"type": "text", "text": "a subagent's"}]}}),
        ];
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&transcript, text.join("\n")).unwrap();
        let e = one(
            "Stop",
            json!({"last_assistant_message": "", "transcript_path": transcript}),
        );
        assert_eq!(body(&e)["assistant"], "Tests pass.\nPushed.");
        assert!(
            events(
                "claude",
                "PreToolUse",
                &json!({"tool_name": "Bash"}),
                0,
                &Settings::default()
            )
            .is_empty()
        );
    }

    /// Milestone 4 D6: an imported event takes `events`' kind and body, and the labels its source
    /// kept (session, repo, cwd) through the same gate, never the checkout as it is now (its branch
    /// has moved on); v1's
    /// repository row is a `touch` record.
    #[test]
    fn an_imported_event_keeps_its_source_labels_and_reads_no_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/now\n").unwrap();
        let payload = json!({"session_id": "s1", "prompt": "keep UTC", "cwd": dir.path()});
        let live = one("UserPromptSubmit", payload.clone());
        assert_eq!(live.branch.as_deref(), Some("now"));
        let settings = Settings {
            source: "oboete-v1",
            ..Settings::default()
        };
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let repo = format!("github.com/o/r/{token}");
        let mut v = imported(
            "claude",
            "UserPromptSubmit",
            &payload,
            7,
            &repo,
            dir.path().to_str(),
            &settings,
        );
        assert_eq!(v.len(), 1);
        let Captured { event: e, ledger } = v.remove(0);
        assert_eq!((e.kind.as_str(), body(&e)), ("prompt", body(&live)));
        assert_eq!((e.branch, e.head, e.gitdir), (None, None, None));
        assert_eq!(
            (e.session.as_str(), e.cwd.as_deref(), e.source.as_str()),
            ("s1", dir.path().to_str(), "oboete-v1")
        );
        let repo = e.repo.unwrap();
        assert!(
            !repo.contains(&token) && repo.contains("[REDACTED]"),
            "{repo}"
        );
        assert_eq!(ledger[0].0, "repo");
        let touch = json!({"session_id": "s1"});
        let v = imported(
            "claude",
            "Touch",
            &touch,
            5,
            "github.com/o/r",
            None,
            &settings,
        );
        let e = &v[0].event;
        assert_eq!(
            (e.kind.as_str(), e.body.as_str(), e.repo.as_deref(), e.ts),
            ("touch", "{}", Some("github.com/o/r"), 5)
        );
        // What a hook records nothing of, an import records nothing of either.
        assert!(imported("claude", "PreToolUse", &payload, 7, "r", None, &settings).is_empty());
    }

    #[test]
    fn git_fields_come_from_files_in_a_worktree_too() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let git = |args: &[&str], cwd: &Path| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{args:?}: {ok:?}");
            String::from_utf8(ok.stdout).unwrap().trim().to_owned()
        };
        std::fs::create_dir(&main).unwrap();
        assert_eq!(super::git(&main), Git::default());
        git(&["init", "-q"], &main);
        let unborn = super::git(&main);
        assert_eq!(
            (unborn.branch.as_deref(), unborn.head),
            (Some("main"), None)
        );
        git(&["commit", "-q", "--allow-empty", "-m", "one"], &main);
        let sha = git(&["rev-parse", "HEAD"], &main);
        let wt = dir.path().join("wt");
        git(
            &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
            &main,
        );
        git(&["pack-refs", "--all"], &main); // refs now only in packed-refs
        let m = super::git(&main.join("sub-dir-that-does-not-exist"));
        assert_eq!(
            (m.branch.as_deref(), m.head.as_deref()),
            (Some("main"), Some(sha.as_str()))
        );
        let w = super::git(&wt);
        assert_eq!(
            (w.branch.as_deref(), w.head.as_deref()),
            (Some("side"), Some(sha.as_str()))
        );
        assert_ne!(w.gitdir, m.gitdir);
        assert!(w.gitdir.unwrap().contains("worktrees"));
        git(&["checkout", "-q", "--detach"], &wt);
        let d = super::git(&wt);
        assert_eq!((d.branch, d.head.as_deref()), (None, Some(sha.as_str())));
    }
}
