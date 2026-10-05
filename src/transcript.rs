//! Agent transcripts as replay fixtures (docs/milestone-1-plan.md Task 4; spec 7.4, 8.4 item 1).
//! `oboete transcript <path> --agent claude|codex` prints one `{seq, agent, event, session, ts,
//! payload}` line per hook event the transcript implies: the format `oboete replay` reads, so a
//! transcript replays through today's hooks and feeds the transcript import (spec 7.4, A60).
//! Conversion leaves text whole for replay; import uses capture's settings and redaction gate.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::capture::{self, Captured, Settings};
use crate::migrate::FailureCode;
use crate::raw::{self, Checkpoint, IMPORT_BATCH, MAX_BATCH_BYTES};

/// Claude Code records that only the transcript has: no prompt hook ever saw them.
const TRANSCRIPT_ONLY: [&str; 5] = [
    "<local-command-",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
    "[Request interrupted",
];

/// Local commands older Claude Code stored as plain text; the prompt hook never got them.
const LOCAL_COMMANDS: [&str; 13] = [
    "/compact",
    "/clear",
    "/effort",
    "/model",
    "/plugin",
    "/exit",
    "/mcp",
    "/login",
    "/resume",
    "/config",
    "/reload-plugins",
    "/reload-skills",
    "/advisor",
];

/// Harness context Codex sends as user messages; not typed prompts. Newer rollouts say so in
/// `content_item_kinds`; this list is for records without it.
const CODEX_CONTEXT: [&str; 9] = [
    "<environment_context>",
    "<user_instructions>",
    "# AGENTS.md",
    "<permissions",
    "<INSTRUCTIONS>",
    "<recommended_plugins>",
    "<skill>",
    "<codex_internal_context",
    "<hook_prompt",
];

#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    pub lines: u64,
    /// Lines that are not JSON.
    pub skipped: u64,
    pub events: u64,
    /// Record types this parser does not read, by type: a new format shows up here.
    pub ignored: std::collections::BTreeMap<String, u64>,
}

/// A tool call waiting for its result: (id, name, input, ts, subagent id, cwd when made).
type Pending = (
    String,
    String,
    Value,
    String,
    Option<String>,
    Option<String>,
);

#[derive(serde::Serialize)]
// The fields in the order `convert` printed them before this struct (`json!` keeps its keys'
// order here: serde_json's `preserve_order`), so a fixture converted again keeps its bytes.
struct Line {
    seq: u64,
    agent: &'static str,
    event: String,
    session: String,
    ts: String,
    payload: Value,
    #[serde(skip)]
    synthetic: bool,
    #[serde(skip)]
    native_session_known: bool,
}

struct Emitter {
    agent: &'static str,
    session: String,
    path: String,
    cwd: Option<String>,
    started: bool,
    /// The session's own id was read (the file name is only the fallback). A forked Codex
    /// rollout carries its parent's `session_meta` after its own.
    meta_seen: bool,
    native_session_known: bool,
    /// Whether another agent sent a Codex rollout's prompts, from its own `session_meta` (#273).
    agent_sent: Option<bool>,
    /// Prompts the prompt hook got when they were queued, not yet delivered as user records.
    queued: Vec<String>,
    stats: Stats,
    pending: Vec<Pending>,
    /// The current turn's last assistant text, sent as `Stop` when the turn ends, and the
    /// directory it was said in (the record that ends the turn may already be in another).
    last_text: Option<String>,
    turn_cwd: Option<String>,
    last_ts: String,
    /// Lines without their `seq`, keyed by time: subagent files are read after the main file,
    /// and `oboete replay` takes the lines in order, so they are sorted before they are written.
    lines: Vec<(String, Line)>,
}

impl Emitter {
    fn ignore(&mut self, kind: String) {
        *self.stats.ignored.entry(kind).or_default() += 1;
    }

    fn write(&mut self, event: &str, ts: &str, mut payload: Value) -> Result<()> {
        self.stats.events += 1;
        payload["session_id"] = json!(self.session);
        payload["transcript_path"] = json!(self.path);
        payload["cwd"] = json!(self.cwd.as_deref().unwrap_or("."));
        payload["hook_event_name"] = json!(event);
        // A record without a time keeps the place of the one before it.
        let ts = match (ts, self.lines.last()) {
            ("", Some((prev, _))) => prev.clone(),
            _ => ts.to_string(),
        };
        let line = Line {
            seq: 0,
            agent: self.agent,
            event: event.to_owned(),
            session: self.session.clone(),
            ts: ts.clone(),
            payload,
            synthetic: false,
            native_session_known: self.native_session_known,
        };
        // SessionStart sorts first whatever its time.
        let key = if event == "SessionStart" {
            String::new()
        } else {
            ts
        };
        self.lines.push((key, line));
        Ok(())
    }

    /// Every line in time order (a stable sort: equal times keep the order they were read in),
    /// numbered, then SessionEnd.
    fn flush(mut self) -> Result<(Vec<Line>, Stats)> {
        if self.started {
            let ts = self.last_ts.clone();
            self.write("SessionEnd", &ts, json!({"reason": "transcript_end"}))?;
        }
        let end = self.lines.len().saturating_sub(1);
        self.lines[..end].sort_by(|a, b| a.0.cmp(&b.0));
        let lines = self
            .lines
            .into_iter()
            .enumerate()
            .map(|(i, (_, mut line))| {
                line.seq = i as u64 + 1;
                line
            })
            .collect();
        Ok((lines, self.stats))
    }

    fn emit(&mut self, event: &str, ts: &str, payload: Value) -> Result<()> {
        if !self.started {
            self.started = true;
            self.write("SessionStart", ts, json!({"source": "startup"}))?;
        }
        self.write(event, ts, payload)
    }

    /// The turn ended at `ts` (the record that ended it), after its last text.
    fn said(&mut self, text: String) {
        self.last_text = Some(text);
        self.turn_cwd.clone_from(&self.cwd);
    }

    fn stop(&mut self, ts: &str) -> Result<()> {
        let Some(text) = self.last_text.take() else {
            return Ok(());
        };
        let now = std::mem::replace(&mut self.cwd, self.turn_cwd.take());
        let result = self.emit("Stop", ts, json!({"last_assistant_message": text}));
        self.cwd = now;
        result
    }

    fn prompt(&mut self, ts: &str, text: &str) -> Result<()> {
        self.stop(ts)?;
        let mut payload = json!({"prompt": text});
        if let Some(sent) = self.agent_sent {
            payload[crate::capture::AGENT_SENT] = json!(sent);
        }
        self.emit("UserPromptSubmit", ts, payload)
    }

    fn tool_use(&mut self, id: &str, name: &str, input: Value, ts: &str, agent_id: Option<&str>) {
        let entry = (
            id.into(),
            name.into(),
            input,
            ts.into(),
            agent_id.map(Into::into),
            self.cwd.clone(),
        );
        self.pending.push(entry);
    }

    fn tool_result(
        &mut self,
        ts: &str,
        id: &str,
        response: Value,
        error: Option<String>,
        answers: Option<&Value>,
    ) -> Result<()> {
        // A result whose call is not in this file (a resumed session) has nothing to pair with.
        let Some(i) = self.pending.iter().position(|p| p.0 == id) else {
            return Ok(());
        };
        let (_, name, mut input, _, agent_id, _) = self.pending.remove(i);
        if let Some(a) = answers {
            input["answers"] = a.clone();
        }
        let mut p = json!({"tool_name": name, "tool_input": input});
        if let Some(a) = agent_id {
            p["agent_id"] = json!(a);
        }
        // A live failure carries `error` and no `tool_response`; the hook reads the latter first.
        match error {
            Some(e) => {
                p["error"] = json!(e);
                self.emit("PostToolUseFailure", ts, p)
            }
            None => {
                p["tool_response"] = response;
                self.emit("PostToolUse", ts, p)
            }
        }
    }

    /// Calls that never got a result, as interrupted, at their own time. `main_only`: at an
    /// interruption of the session, only its own calls; subagent calls wait for their file's end.
    fn interrupt(&mut self, main_only: bool) -> Result<()> {
        let (gone, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| !main_only || p.4.is_none());
        self.pending = kept;
        for (_, name, input, ts, agent_id, cwd) in gone {
            let mut p = json!({"tool_name": name, "tool_input": input, "tool_response": Value::Null, "interrupted": true});
            if let Some(a) = agent_id {
                p["agent_id"] = json!(a);
            }
            // In the directory the call was made in: a subagent's end comes after its scope.
            let now = std::mem::replace(&mut self.cwd, cwd);
            let result = self.emit("PostToolUse", &ts, p);
            self.cwd = now;
            result?;
        }
        Ok(())
    }

    /// End of one file: calls that never got a result, then the turn's last text.
    fn finish(&mut self) -> Result<()> {
        let first = self.lines.len();
        self.interrupt(false)?;
        let ts = self.last_ts.clone();
        self.stop(&ts)?;
        for (_, line) in &mut self.lines[first..] {
            line.synthetic = true;
        }
        Ok(())
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|i| i["type"] == "text")
            .filter_map(|i| i["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A slash command as the developer typed it: the transcript stores it as tags.
fn command_text(s: &str) -> Option<String> {
    let tag = |name: &str| {
        let (_, rest) = s.split_once(&format!("<{name}>"))?;
        let (value, _) = rest.split_once(&format!("</{name}>"))?;
        Some(value.trim().to_string())
    };
    let name = tag("command-name")?;
    Some(match tag("command-args").filter(|a| !a.is_empty()) {
        Some(args) => format!("{name} {args}"),
        None => name,
    })
}

fn claude_line(e: &mut Emitter, v: &Value, agent_id: Option<&str>) -> Result<()> {
    // Older Claude Code wrote subagent turns inline, marked isSidechain; newer writes them to
    // <session>/subagents/, read with their file's agent id.
    let agent_id = if v["isSidechain"] == true {
        Some(v["agentId"].as_str().unwrap_or("sidechain"))
    } else {
        agent_id
    };
    // Hooks key the repository from each event's cwd. A main-session record moves the session's
    // directory; a subagent's events carry its own (it may run in a worktree, and its file is read
    // after the main one) without moving the session's.
    let cwd = v["cwd"].as_str().map(String::from);
    if agent_id.is_none() {
        // The id the hooks got, even when the file was copied under another name.
        if !e.meta_seen
            && let Some(id) = v["sessionId"].as_str()
        {
            e.session = id.to_string();
            e.native_session_known = !id.is_empty();
            e.meta_seen = true;
        }
        e.cwd = cwd.or(e.cwd.take());
        return claude_record(e, v, None);
    }
    let session = e.cwd.clone();
    e.cwd = cwd.or(session.clone());
    let result = claude_record(e, v, agent_id);
    e.cwd = session;
    result
}

fn claude_record(e: &mut Emitter, v: &Value, agent_id: Option<&str>) -> Result<()> {
    let ts = v["timestamp"].as_str().unwrap_or_default().to_string();
    let content = &v["message"]["content"];
    match v["type"].as_str() {
        // A subagent's compacted context is its own, not the session's.
        Some("user") if v["isCompactSummary"] == true && agent_id.is_none() => e.emit(
            "PostCompact",
            &ts,
            json!({"trigger": "auto", "compact_summary": text_of(content)}),
        ),
        Some("user") if v["isMeta"] == true => Ok(()),
        Some("user") => {
            for item in content.as_array().into_iter().flatten() {
                if item["type"] == "tool_result" {
                    let id = item["tool_use_id"].as_str().unwrap_or_default();
                    let full = &v["toolUseResult"];
                    let response = if full.is_null() {
                        item["content"].clone()
                    } else {
                        full.clone()
                    };
                    let error = (item["is_error"] == true).then(|| text_of(&item["content"]));
                    e.tool_result(&ts, id, response, error, full.get("answers"))?;
                }
            }
            let text = text_of(content);
            let t = text.trim_start();
            // The developer stopped the turn: its unanswered calls end here, not at the file's end.
            if agent_id.is_none() && t.starts_with("[Request interrupted") {
                e.interrupt(true)?;
            }
            if agent_id.is_some()
                || t.is_empty()
                || TRANSCRIPT_ONLY
                    .iter()
                    .chain(&LOCAL_COMMANDS)
                    .any(|p| t.starts_with(p))
            {
                return Ok(());
            }
            // The delivery of a prompt already sent when it was queued.
            // Measured on the dev set: it comes after the turn ended (mid-turn ones are attachments).
            if let Some(i) = e.queued.iter().position(|q| q == text.trim()) {
                e.queued.remove(i);
                return e.stop(&ts);
            }
            // A skill command (message tag first) and /goal reach the prompt hook as typed; other
            // local commands (/compact, /effort: name tag first) never do. Measured 2026-09-26:
            // claude-mem's copy has skill commands and /goal (42 sessions) among its prompts, but
            // none of the 245 /effort or 135 /compact in the transcripts.
            if t.starts_with("<command-message>") || t.starts_with("<command-name>") {
                let hooked =
                    t.starts_with("<command-message>") || t.starts_with("<command-name>/goal<");
                return match command_text(t).filter(|_| hooked) {
                    Some(command) => e.prompt(&ts, &command),
                    None => Ok(()),
                };
            }
            e.prompt(&ts, &text)
        }
        // A prompt typed while a turn runs reaches the prompt hook when it is queued (claude-mem
        // has them). Harness traffic is queued too; it is read where it is delivered.
        Some("queue-operation") if v["operation"] == "enqueue" => {
            let text = text_of(&v["content"]);
            let t = text.trim();
            // Only the known harness forms; a queued prompt may itself start with markup.
            if t.is_empty()
                || crate::hook::is_envelope(t)
                || t.starts_with("Another Claude session sent a message")
                || t.starts_with("<command-")
                || TRANSCRIPT_ONLY
                    .iter()
                    .chain(&LOCAL_COMMANDS)
                    .any(|p| t.starts_with(p))
            {
                return Ok(());
            }
            // The turn that runs goes on: its Stop comes where it ends, not here.
            e.queued.push(t.to_string());
            e.emit("UserPromptSubmit", &ts, json!({"prompt": text}))
        }
        // Where the Stop hook ran (a turn with stop hooks), or the turn's end.
        Some("system")
            if agent_id.is_none()
                && matches!(
                    v["subtype"].as_str(),
                    Some("stop_hook_summary" | "turn_duration")
                ) =>
        {
            e.stop(&ts)
        }
        Some("assistant") => {
            for item in content.as_array().into_iter().flatten() {
                match item["type"].as_str() {
                    Some("tool_use") => e.tool_use(
                        item["id"].as_str().unwrap_or_default(),
                        item["name"].as_str().unwrap_or("?"),
                        item["input"].clone(),
                        &ts,
                        agent_id,
                    ),
                    Some("text") if agent_id.is_none() => {
                        let t = item["text"].as_str().unwrap_or_default();
                        if !t.trim().is_empty() {
                            e.said(t.to_string());
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        other => {
            e.ignore(other.unwrap_or("<none>").to_string());
            Ok(())
        }
    }
}

/// Whether another agent sent a Codex session's prompts, from its `session_meta` payload (#273):
/// a `codex exec` run, a session Claude Code's Codex plugin started, Codex as an MCP server, or a
/// sub-agent's thread. The owner's TUI and VS Code sessions are `codex-tui`, and an originator not
/// listed here stays the user's.
pub(crate) fn codex_agent_sent(meta: &Value) -> bool {
    // A sub-agent's thread is the one `source` object seen: its shape, not any object, so a
    // new object form of the owner's sessions stays the user's.
    meta["source"].get("subagent").is_some()
        || matches!(meta["source"].as_str(), Some("exec" | "mcp"))
        || matches!(
            meta["originator"].as_str(),
            Some("codex_exec" | "Claude Code")
        )
}

fn codex_line(e: &mut Emitter, v: &Value) -> Result<()> {
    let ts = v["timestamp"].as_str().unwrap_or_default().to_string();
    let p = &v["payload"];
    let call_id = p["call_id"].as_str().unwrap_or_default();
    match (v["type"].as_str(), p["type"].as_str()) {
        (Some("session_meta"), _) => {
            if !e.meta_seen
                && let Some(id) = p["id"].as_str()
            {
                e.session = id.to_string();
                e.native_session_known = !id.is_empty();
            }
            if !e.meta_seen {
                e.agent_sent = Some(codex_agent_sent(p));
            }
            e.meta_seen = true;
            if e.cwd.is_none() {
                e.cwd = p["cwd"].as_str().map(Into::into);
            }
            Ok(())
        }
        // A resumed rollout can continue in another directory.
        (Some("turn_context"), _) => {
            if let Some(cwd) = p["cwd"].as_str() {
                e.cwd = Some(cwd.into());
            }
            Ok(())
        }
        (Some("response_item"), Some("message")) => {
            let items: Vec<&Value> = p["content"].as_array().into_iter().flatten().collect();
            let kinds = p["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
                .as_array()
                .filter(|k| k.len() == items.len());
            // Only what the developer typed ("user.text"), when the rollout says which is which.
            let text = items
                .iter()
                .enumerate()
                .filter(|(i, _)| kinds.is_none_or(|k| k[*i] == "user.text"))
                .filter_map(|(_, c)| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                return Ok(());
            }
            match p["role"].as_str() {
                Some("user")
                    if !CODEX_CONTEXT
                        .iter()
                        .any(|c| text.trim_start().starts_with(c)) =>
                {
                    e.prompt(&ts, &text)
                }
                Some("assistant") => {
                    e.said(text);
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        (Some("response_item"), Some("function_call")) => {
            let args = p["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_else(|| json!({"arguments": p["arguments"]}));
            e.tool_use(call_id, p["name"].as_str().unwrap_or("?"), args, &ts, None);
            Ok(())
        }
        (Some("response_item"), Some("custom_tool_call")) => {
            e.tool_use(
                call_id,
                p["name"].as_str().unwrap_or("?"),
                json!({"input": p["input"]}),
                &ts,
                None,
            );
            Ok(())
        }
        (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
            e.tool_result(&ts, call_id, p["output"].clone(), None, None)
        }
        (Some("event_msg"), Some("task_complete")) => {
            if let Some(t) = p["last_agent_message"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
            {
                e.said(t.to_string());
            }
            e.stop(&ts)
        }
        // An aborted turn sends no Stop, and its unanswered calls end here.
        (Some("event_msg"), Some("turn_aborted")) => {
            e.last_text = None;
            e.interrupt(true)
        }
        (Some("compacted"), _) => match p["message"].as_str().filter(|s| !s.trim().is_empty()) {
            Some(s) => e.emit(
                "PostCompact",
                &ts,
                json!({"trigger": "auto", "compact_summary": s}),
            ),
            None => Ok(()),
        },
        (kind, sub) => {
            e.ignore(format!(
                "{}:{}",
                kind.unwrap_or("<none>"),
                sub.unwrap_or("")
            ));
            Ok(())
        }
    }
}

fn read_file(e: &mut Emitter, path: &Path, agent_id: Option<&str>) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    // Split on bytes: one line of broken UTF-8 must not end the file.
    for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        let line = String::from_utf8_lossy(&line);
        if line.trim().is_empty() {
            continue;
        }
        e.stats.lines += 1;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            e.stats.skipped += 1;
            continue;
        };
        // SessionEnd takes the main file's last time; subagent files are read after it.
        if agent_id.is_none()
            && let Some(t) = v["timestamp"].as_str()
        {
            e.last_ts = t.to_string();
        }
        match e.agent {
            "claude" => claude_line(e, &v, agent_id)?,
            _ => codex_line(e, &v)?,
        }
    }
    e.finish()
}

fn jsonl_under(dir: &Path, prefix: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if entry.file_type()?.is_dir() {
            jsonl_under(&p, prefix, out)?;
        } else if p.extension().is_some_and(|x| x == "jsonl")
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
        {
            out.push(p);
        }
    }
    Ok(())
}

fn subagent_files(path: &Path, agent: &str) -> Result<Vec<PathBuf>> {
    let dir = path.with_extension("").join("subagents");
    let mut files = Vec::new();
    if agent == "claude" && dir.is_dir() {
        // A workflow's journal.jsonl sits beside its agents; it is no transcript.
        jsonl_under(&dir, "agent-", &mut files)?;
        files.sort();
    }
    Ok(files)
}

fn parse(path: &Path, agent: &str, subagents: &[PathBuf]) -> Result<(Vec<Line>, Stats)> {
    let agent: &'static str = match agent {
        "claude" => "claude",
        "codex" => "codex",
        other => bail!("no transcript parser for {other}: claude and codex have one"),
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    let mut e = Emitter {
        agent,
        session: stem.to_string(),
        path: path.display().to_string(),
        cwd: None,
        started: false,
        meta_seen: false,
        native_session_known: false,
        agent_sent: None,
        queued: Vec::new(),
        stats: Stats::default(),
        pending: Vec::new(),
        last_text: None,
        turn_cwd: None,
        last_ts: String::new(),
        lines: Vec::new(),
    };
    read_file(&mut e, path, None)?;
    for f in subagents {
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let id = stem.strip_prefix("agent-").unwrap_or(stem).to_string();
        read_file(&mut e, f, Some(&id))?;
    }
    e.flush()
}

pub fn convert(path: &Path, agent: &str, mut out: impl Write) -> Result<Stats> {
    let (lines, stats) = parse(path, agent, &subagent_files(path, agent)?)?;
    for line in lines {
        serde_json::to_writer(&mut out, &line)?;
        writeln!(out)?;
    }
    Ok(stats)
}

/// What each agent's transcripts contributed, or would contribute in a preview (spec 7.4).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ImportStats {
    pub agents: BTreeMap<String, AgentStats>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct AgentStats {
    pub files: u64,
    pub sessions: u64,
    /// Records appended, after `append_imported` leaves denied records out; preview counts the
    /// records capture would produce without opening a destination store.
    pub events: u64,
    pub cut: u64,
    pub seen: u64,
    pub housekeeping: u64,
    /// Sessions whose identifier the redaction gate masks or clips: never imported.
    pub masked: u64,
    pub waiting: u64,
    /// Files whose stored prefix or cross-source forget correspondence cannot be verified.
    pub refused: u64,
    /// Bytes of captured bodies selected for import, before the store's deny-list check.
    pub bytes: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Preview {
    pub key: String,
    pub candidates: ImportStats,
    pub v1: Option<crate::migrate::Preview>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub stats: ImportStats,
    pub v1: Option<crate::migrate::Outcome>,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub outcome: Box<Outcome>,
    pub code: crate::migrate::FailureCode,
    pub cause: anyhow::Error,
}

#[derive(Clone, Debug)]
pub(crate) enum Committed {
    V1(crate::migrate::Committed),
    Transcripts {
        agent: String,
        events: u64,
        bytes: u64,
    },
}

#[derive(serde::Serialize)]
struct BoundFile {
    agent: String,
    path: PathBuf,
    digest: String,
    parsed: String,
}

struct PreviewPlan {
    preview: Preview,
    files: Vec<BoundFile>,
    config: Option<Vec<u8>>,
    destination: String,
}

/// Bind the events actually parsed, including native-identity flags, rather than trusting
/// that a later reread of a path still describes the bytes the parser consumed.
fn parsed_digest(lines: &[Line]) -> Result<String> {
    let mut hash = Sha256::new();
    for line in lines {
        hash.update(serde_json::to_vec(&(
            line,
            line.synthetic,
            line.native_session_known,
        ))?);
        hash.update(b"\n");
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn check_bound(file: &BoundFile) -> Result<()> {
    let current =
        source_digest(&file.path, &file.agent).map_err(|_| anyhow::anyhow!(FailureCode::Stale))?;
    anyhow::ensure!(current == file.digest, FailureCode::Stale);
    Ok(())
}

fn source_digest(path: &Path, agent: &str) -> Result<String> {
    let files = subagent_files(path, agent)?;
    let entries = std::iter::once(path)
        .chain(files.iter().map(PathBuf::as_path))
        .map(|p| {
            Ok((
                p.canonicalize()?,
                crate::db::store_file(p),
                crate::migrate::file_version(p)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(crate::forget::hash(&serde_json::to_vec(&entries)?))
}

fn bound_files(roots: &[(&str, &Path)]) -> Result<Vec<BoundFile>> {
    let mut files = Vec::new();
    for (agent, root) in roots {
        anyhow::ensure!(
            matches!(*agent, "claude" | "codex"),
            "unsupported transcript agent"
        );
        for path in transcript_files(root, agent)? {
            files.push(BoundFile {
                agent: (*agent).to_owned(),
                digest: source_digest(&path, agent)?,
                parsed: String::new(),
                path,
            });
        }
    }
    Ok(files)
}

pub(crate) fn preview(home: &Path, roots: &[(&str, &Path)]) -> Result<Preview> {
    Ok(preview_plan(home, roots)?.preview)
}

pub(crate) fn run(
    home: &Path,
    roots: &[(&str, &Path)],
    expected: Option<&str>,
    committed: &mut impl FnMut(&Committed),
) -> std::result::Result<Outcome, Failure> {
    run_with_output(home, roots, expected, committed, &mut std::io::sink())
}

fn run_with_output(
    home: &Path,
    roots: &[(&str, &Path)],
    expected: Option<&str>,
    committed: &mut impl FnMut(&Committed),
    out: &mut impl Write,
) -> std::result::Result<Outcome, Failure> {
    let mut outcome = Outcome::default();
    let mut code = FailureCode::InvalidSource;
    let result = (|| {
        let v1 = home.join("oboete.db");
        crate::migrate::check_source(home, &v1)?;
        code = FailureCode::Busy;
        let _lock = crate::import::lock(home)?;
        let mut plan = if let Some(expected) = expected {
            code = FailureCode::Stale;
            let plan = preview_plan(home, roots)?;
            anyhow::ensure!(plan.preview.key == expected, FailureCode::Stale);
            Some(plan)
        } else {
            None
        };
        let before_config = plan.as_ref().map(|p| p.config.clone());
        let mut raw = None;
        if v1.try_exists()? {
            let expected_v1 = plan
                .as_ref()
                .and_then(|p| p.preview.v1.as_ref())
                .map(|p| p.key.as_str());
            if plan.is_some() {
                anyhow::ensure!(expected_v1.is_some(), FailureCode::Stale);
            }
            let migrated =
                crate::migrate::run_holding(home, &v1, &mut raw, expected_v1, &mut |c| {
                    committed(&Committed::V1(c.clone()))
                });
            match migrated {
                Ok(migrated) => {
                    outcome.v1 = Some(migrated);
                    if let Some(settings) =
                        &outcome.v1.as_ref().context("v1 outcome absent")?.settings
                    {
                        for line in crate::migrate::settings_lines(&settings.missing) {
                            writeln!(out, "{line}")?;
                        }
                    }
                    writeln!(
                        out,
                        "v1 migration: {}",
                        serde_json::to_string(
                            &outcome.v1.as_ref().context("v1 outcome absent")?.stats
                        )?
                    )?;
                }
                Err(failed) => {
                    outcome.v1 = Some(*failed.outcome);
                    code = failed.code;
                    if let Some(settings) =
                        &outcome.v1.as_ref().context("v1 outcome absent")?.settings
                    {
                        for line in crate::migrate::settings_lines(&settings.missing) {
                            writeln!(out, "{line}")?;
                        }
                    }
                    return Err(failed.cause);
                }
            }
        } else if plan.as_ref().is_some_and(|p| p.preview.v1.is_some()) {
            anyhow::bail!(FailureCode::Stale);
        }
        // Recheck after acquiring the same lock every settings writer uses; hold the selected
        // capture settings steady until the last transcript batch has committed.
        let _config = crate::settings::config_lock(home)?;
        code = FailureCode::Stale;
        if let Some(before) = before_config {
            anyhow::ensure!(
                crate::migrate::config_bytes(home)? == before,
                FailureCode::Stale
            );
        }
        code = FailureCode::InvalidConfig;
        Settings::load(home)?;
        code = FailureCode::Stale;
        if raw.is_none() {
            let destination = if plan.is_some() {
                raw::read_only(home)?
            } else {
                None
            };
            if let Some(plan) = &plan {
                anyhow::ensure!(
                    plan.destination == crate::migrate::preview_key(home, &"destination")?,
                    FailureCode::Stale
                );
            }
            crate::migrate::check_source(home, &v1)?;
            code = FailureCode::Failed;
            raw = Some(match destination {
                Some(held) => held.into_writer(home)?,
                None => raw::open(home)?,
            });
        }
        code = FailureCode::Failed;
        crate::forget::reconcile_or_say(home, raw.as_mut().context("transcript raw absent")?)?;
        import_files(
            home,
            roots,
            &mut raw,
            &mut outcome.stats,
            out,
            committed,
            plan.as_mut().map(|p| p.files.as_mut_slice()),
        )
    })();
    match result {
        Ok(()) => Ok(outcome),
        Err(cause) => Err(Failure {
            outcome: Box::new(outcome),
            code: crate::migrate::failure_code(&cause, code),
            cause,
        }),
    }
}

fn preview_plan(home: &Path, roots: &[(&str, &Path)]) -> Result<PreviewPlan> {
    let destination = crate::migrate::preview_key(home, &"destination")?;
    let config = crate::migrate::config_bytes(home)?;
    let path = home.join("oboete.db");
    let v1 = path
        .try_exists()?
        .then(|| crate::migrate::preview(home, &path))
        .transpose()?;
    let mut files = bound_files(roots)?;
    let mut candidates = ImportStats::default();
    import_files(
        home,
        roots,
        &mut None,
        &mut candidates,
        &mut std::io::sink(),
        &mut |_| {},
        Some(&mut files),
    )?;
    anyhow::ensure!(
        crate::migrate::config_bytes(home)? == config,
        "transcript settings changed during preview"
    );
    let key = crate::migrate::preview_key(
        home,
        &(
            "transcripts",
            roots,
            &files,
            config.as_deref().map(crate::forget::hash),
            &v1,
            &candidates,
        ),
    )?;
    anyhow::ensure!(
        destination == crate::migrate::preview_key(home, &"destination")?,
        FailureCode::Stale
    );
    Ok(PreviewPlan {
        preview: Preview {
            key,
            candidates,
            v1,
        },
        files,
        config,
        destination,
    })
}

/// Import stable transcripts locally, with the time cut and transactional checkpoints of A60
/// and D6. Preview reads only the transcripts, settings and v1 store, and creates nothing.
pub fn import(
    home: &Path,
    roots: &[(&str, &Path)],
    yes: bool,
    out: &mut impl Write,
) -> Result<ImportStats> {
    if yes {
        return run_with_output(home, roots, None, &mut |_| {}, out)
            .map(|outcome| outcome.stats)
            .map_err(|failed| {
                if failed.code == FailureCode::Refused {
                    // The typed code is internal; preserve the native CLI's refusal text.
                    anyhow::anyhow!(failed.cause.to_string())
                } else {
                    failed.cause
                }
            });
    }
    let mut stats = ImportStats::default();
    import_files(home, roots, &mut None, &mut stats, out, &mut |_| {}, None)?;
    Ok(stats)
}

fn import_files(
    home: &Path,
    roots: &[(&str, &Path)],
    raw: &mut Option<raw::Raw>,
    stats: &mut ImportStats,
    out: &mut impl Write,
    committed: &mut impl FnMut(&Committed),
    mut files: Option<&mut [BoundFile]>,
) -> Result<()> {
    let yes = raw.is_some();
    let v1 = home.join("oboete.db");
    let cut = match raw.as_ref() {
        Some(raw) => raw.earliest_by_session()?,
        None => preview_cut(&v1)?,
    };
    let mut checkpoints = match raw.as_ref() {
        Some(raw) => raw.migration_checkpoints("transcript:")?,
        None => HashMap::new(),
    };
    let settings = Settings {
        source: "transcript",
        ..Settings::load(home)?
    };
    let clock = rusqlite::Connection::open_in_memory()?;
    let mut sessions = HashSet::new();
    for (agent, root) in roots {
        let stats = stats.agents.entry((*agent).to_owned()).or_default();
        let paths = match files.as_deref() {
            Some(files) => files
                .iter()
                .filter(|file| file.agent == *agent)
                .map(|file| file.path.clone())
                .collect(),
            None => transcript_files(root, agent)?,
        };
        for path in paths {
            let mut bound = files.as_deref_mut().and_then(|files| {
                files
                    .iter_mut()
                    .find(|file| file.agent == *agent && file.path == path)
            });
            if let Some(bound) = bound.as_deref() {
                check_bound(bound)?;
            }
            stats.files += 1;
            if yes && bound.as_deref().is_some_and(|file| file.parsed.is_empty()) {
                stats.waiting += 1;
                continue;
            }
            let Some(mut lines) = stable_lines(&path, agent)? else {
                if yes && bound.is_some() {
                    anyhow::bail!(FailureCode::Stale);
                }
                stats.waiting += 1;
                continue;
            };
            if let Some(bound) = bound.as_deref_mut() {
                check_bound(bound)?;
                let parsed = parsed_digest(&lines)?;
                if bound.parsed.is_empty() {
                    bound.parsed = parsed;
                } else {
                    anyhow::ensure!(bound.parsed == parsed, FailureCode::Stale);
                }
            }
            // EOF's interrupted calls, Stop and SessionEnd may be replaced when the file grows.
            // Number only settled events, so none of those synthetic lines moves the checkpoint.
            lines.retain(|line| !line.synthetic && line.event != "SessionEnd");
            for (i, line) in lines.iter_mut().enumerate() {
                line.seq = i as u64 + 1;
            }
            let Some(first) = lines.first() else {
                continue;
            };
            let session = &first.session;
            let new_session = sessions.insert(((*agent).to_owned(), session.clone()));
            stats.sessions += u64::from(new_session);
            if lines
                .iter()
                .any(|line| crate::hook::is_agent_internal(agent, &line.payload))
            {
                stats.housekeeping += u64::from(new_session);
                continue;
            }
            let recorded = cut.get(&((*agent).to_owned(), session.clone())).copied();
            let earliest = match raw.as_ref() {
                Some(raw) => match raw.transcript_cut(agent, session, recorded) {
                    Ok(cut) => cut,
                    Err(e) => {
                        stats.refused += 1;
                        writeln!(
                            out,
                            "refused {}: {e:#}",
                            crate::redact::outbound_with(
                                &path.display().to_string(),
                                &settings.rules
                            )
                        )?;
                        continue;
                    }
                },
                None => recorded,
            };
            // Masked or clipped identifiers cannot serve as checkpoints without collisions,
            // and keeping the original would bypass both the redaction gate and the time cut:
            // such a session is left out, counted. A touch always gives one record to look at.
            let labelled =
                capture::imported(agent, "Touch", &first.payload, 0, "", None, &settings);
            if labelled.first().map(|c| c.event.session.as_str()) != Some(session.as_str()) {
                stats.masked += u64::from(new_session);
                continue;
            }
            if !new_session {
                stats.seen += lines.len() as u64;
                continue;
            }
            let key = format!("transcript:{agent}:{session}");
            let seen = checkpoints.get(&key).map_or(0, |c| c.through);
            let mut prefix = Sha256::new();
            if let Some(previous) = checkpoints.get(&key) {
                for line in lines.iter().take_while(|line| line.seq <= seen as u64) {
                    hash_line(&mut prefix, line)?;
                }
                let fingerprint = format!("{:x}", prefix.clone().finalize());
                if previous.prefix.as_deref() != Some(fingerprint.as_str()) {
                    stats.refused += 1;
                    writeln!(
                        out,
                        "refused {}: its imported prefix changed or cannot be verified",
                        crate::redact::outbound_with(&path.display().to_string(), &settings.rules)
                    )?;
                    continue;
                }
            }
            let mut checkpoint = Checkpoint {
                key: key.clone(),
                through: seen,
                row: None,
                prefix: checkpoints.get(&key).and_then(|c| c.prefix.clone()),
            };
            let namespace = session.clone();
            let namespace_known = first.native_session_known;
            let fingerprints: Vec<String> = lines
                .iter()
                .map(|line| {
                    let mut native = Sha256::new();
                    hash_line(&mut native, line)?;
                    Ok(format!("{:x}", native.finalize()))
                })
                .collect::<Result<_>>()?;
            let mut counts = HashMap::<&str, usize>::new();
            for fingerprint in &fingerprints {
                *counts.entry(fingerprint).or_default() += 1;
            }
            let mut occurrences = HashMap::<String, u64>::new();
            let mut batch = Vec::<(Captured, raw::ImportIdentity)>::new();
            let mut bytes = 0;
            for (line, event_hash) in lines.into_iter().zip(&fingerprints) {
                let occurrence = occurrences.entry(event_hash.clone()).or_default();
                let identity = format!("{event_hash}:{occurrence}");
                *occurrence += 1;
                let through = i64::try_from(line.seq)?;
                if through <= seen {
                    stats.seen += 1;
                    continue;
                }
                hash_line(&mut prefix, &line)?;
                let fingerprint = Some(format!("{:x}", prefix.clone().finalize()));
                let ts = crate::replay::fixture_ms(&clock, &json!(line.ts))
                    .context("a transcript event has no valid timestamp")?;
                if earliest.is_some_and(|cut| ts >= cut) {
                    stats.cut += 1;
                    checkpoint.through = through;
                    checkpoint.prefix = fingerprint;
                    continue;
                }
                // The parser writes "." where the transcript named no directory: that is this
                // process's, not the session's, so such a line gets no repository.
                let cwd = line.payload["cwd"].as_str().filter(|c| *c != ".");
                let repo = cwd.map_or_else(String::new, |c| crate::repo::key(Path::new(c)));
                let captured =
                    capture::imported(agent, &line.event, &line.payload, ts, &repo, cwd, &settings);
                let size: usize = captured.iter().map(|c| c.event.body.len()).sum();
                if batch.len() + captured.len() > IMPORT_BATCH || bytes + size > MAX_BATCH_BYTES {
                    if let Some(bound) = bound.as_deref() {
                        check_bound(bound)?;
                    }
                    append_batch(raw, &mut batch, &checkpoint, &settings, stats)?;
                    if raw.is_some() {
                        committed(&Committed::Transcripts {
                            agent: (*agent).to_owned(),
                            events: stats.events,
                            bytes: stats.bytes,
                        });
                    }
                    bytes = 0;
                }
                batch.extend(captured.into_iter().enumerate().map(|(i, c)| {
                    (
                        c,
                        raw::ImportIdentity {
                            origin: crate::forget::origin(&key, &format!("{identity}:{i}")),
                            session: crate::forget::session(agent, &line.session),
                            ambiguous: (counts[event_hash.as_str()] > 1
                                || !line.native_session_known)
                                .then(|| {
                                    crate::forget::origin(&key, &format!("{event_hash}:0:{i}"))
                                }),
                            unverified: !namespace_known
                                || !line.native_session_known
                                || line.session != namespace,
                        },
                    )
                }));
                bytes += size;
                checkpoint.through = through;
                checkpoint.prefix = fingerprint;
            }
            if checkpoint.through > seen {
                if let Some(bound) = bound.as_deref() {
                    check_bound(bound)?;
                }
                append_batch(raw, &mut batch, &checkpoint, &settings, stats)?;
                if raw.is_some() {
                    committed(&Committed::Transcripts {
                        agent: (*agent).to_owned(),
                        events: stats.events,
                        bytes: stats.bytes,
                    });
                }
                checkpoints.insert(key, checkpoint);
            }
        }
    }
    if !yes {
        writeln!(
            out,
            "Run oboete import transcripts --yes with the same --home and --agent to import."
        )?;
    }
    writeln!(out, "{}", serde_json::to_string(&stats)?)?;
    let refused: u64 = stats.agents.values().map(|agent| agent.refused).sum();
    if refused != 0 {
        return Err(anyhow::anyhow!(FailureCode::Refused).context(format!(
            "{refused} transcript file(s) refused: imported prefixes or cross-source forget identities cannot be verified; \
             review the refusal reasons above; those files were not imported")));
    }
    Ok(())
}

fn hash_line(prefix: &mut Sha256, line: &Line) -> Result<()> {
    // A copied transcript keeps its event contents; its file path is not its identity.
    let mut payload = line.payload.clone();
    if let Some(fields) = payload.as_object_mut() {
        fields.remove("transcript_path");
    }
    prefix.update(serde_json::to_vec(&(
        line.agent,
        &line.event,
        &line.session,
        &line.ts,
        payload,
    ))?);
    prefix.update(b"\n");
    Ok(())
}

fn append_batch(
    raw: &mut Option<raw::Raw>,
    batch: &mut Vec<(Captured, raw::ImportIdentity)>,
    checkpoint: &Checkpoint,
    settings: &Settings,
    stats: &mut AgentStats,
) -> Result<()> {
    let (records, origins): (Vec<_>, Vec<_>) = batch.drain(..).unzip();
    stats.events += match raw.as_mut() {
        Some(raw) => raw
            .append_imported_origins(
                &records,
                &origins,
                settings.rules.version(),
                Some(checkpoint),
            )?
            .len() as u64,
        None => records.len() as u64,
    };
    stats.bytes += records
        .iter()
        .map(|c| c.event.body.len() as u64)
        .sum::<u64>();
    Ok(())
}

fn preview_cut(v1: &Path) -> Result<HashMap<(String, String), i64>> {
    if !v1.exists() {
        return Ok(HashMap::new());
    }
    crate::migrate::with_preview_v1(v1, |conn, _| {
        let mut st = conn.prepare(
            "SELECT s.agent, e.session_id, MIN(e.ts) FROM events e
             JOIN sessions s ON s.id = e.session_id GROUP BY s.agent, e.session_id",
        )?;
        let rows = st.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    })
}

fn transcript_files(root: &Path, agent: &str) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("read transcript root"),
    };
    let mut files = Vec::new();
    if agent == "codex" {
        jsonl_under(root, "rollout-", &mut files)?;
    } else {
        for project in entries {
            let project = project?;
            if project.file_type()?.is_dir() {
                for entry in std::fs::read_dir(project.path())? {
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_file()
                        && path.extension().is_some_and(|e| e == "jsonl")
                    {
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn stamps(
    path: &Path,
    subagents: &[PathBuf],
) -> Result<Vec<(PathBuf, u64, std::time::SystemTime)>> {
    std::iter::once(path)
        .chain(subagents.iter().map(PathBuf::as_path))
        .map(|path| {
            let m = std::fs::metadata(path)?;
            Ok((path.to_owned(), m.len(), m.modified()?))
        })
        .collect()
}

fn stable_lines(path: &Path, agent: &str) -> Result<Option<Vec<Line>>> {
    let subagents = subagent_files(path, agent)?;
    let before = stamps(path, &subagents)?;
    let parsed = parse(path, agent, &subagents);
    #[cfg(test)]
    AFTER_PARSE.with_borrow_mut(|hook| {
        if let Some(hook) = hook.take() {
            hook();
        }
    });
    // Check the file set too: a new or removed subagent changes the session's event order.
    let after = subagent_files(path, agent).and_then(|files| stamps(path, &files));
    if after.as_ref().ok() != Some(&before) {
        return Ok(None);
    }
    Ok(Some(parsed?.0))
}

#[cfg(test)]
thread_local! {
    static AFTER_PARSE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
mod tests {
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
        let shown = preview(&home, &roots).unwrap();
        assert_eq!(
            (
                shown.candidates.agents["codex"].events,
                shown.candidates.agents["codex"].waiting
            ),
            (0, 1)
        );
        assert!(!home.exists());
        let failed = run(&home, &roots, Some(&shown.key), &mut |_| {
            panic!("no commits")
        })
        .unwrap_err();
        assert_eq!(failed.code, FailureCode::Stale);
        assert!(!raw::path(&home).exists());
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
        assert_eq!(result.code, crate::migrate::FailureCode::Refused);
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

    #[test]
    fn transcript_consent_binds_content_file_set_scope_config_and_destination() {
        for change in ["content", "file_set", "scope", "config", "destination"] {
            let dir = tempfile::tempdir().unwrap();
            let (_, root) = fixtures(dir.path());
            let home = dir.path().join("home");
            let roots = [("codex", root.as_path())];
            let shown = preview(&home, &roots).unwrap();
            match change {
                "content" => {
                    let path = root.join("2026/09/02/rollout-basic.jsonl");
                    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                    let times = std::fs::FileTimes::new()
                        .set_modified(file.metadata().unwrap().modified().unwrap());
                    let text = std::fs::read_to_string(&path)
                        .unwrap()
                        .replace("Try again with 100ms", "Try again with 200ms");
                    std::fs::write(path, text).unwrap();
                    file.set_times(times).unwrap();
                }
                "file_set" => {
                    std::fs::copy(CODEX, root.join("rollout-new.jsonl")).unwrap();
                }
                "config" => {
                    std::fs::create_dir_all(&home).unwrap();
                    std::fs::write(home.join("config.toml"), "# new\n").unwrap();
                }
                "destination" => {
                    std::fs::create_dir_all(&home).unwrap();
                    drop(raw::open(&home).unwrap());
                }
                _ => {}
            }
            let selected = if change == "scope" {
                vec![("claude", root.as_path())]
            } else {
                roots.to_vec()
            };
            let failed = run(&home, &selected, Some(&shown.key), &mut |_| {
                panic!("no committed effects")
            })
            .unwrap_err();
            assert_eq!(failed.code, FailureCode::Stale, "{change}: {failed:?}");
            assert!(failed.outcome.stats.agents.is_empty());
            if change != "destination" {
                assert!(!raw::path(&home).exists());
            }
        }
    }

    #[test]
    fn confirmed_transcript_stops_when_next_bound_file_changes_after_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        let (_, root) = fixtures(dir.path());
        let next = root.join("2026/09/02/rollout-new.jsonl");
        let text = std::fs::read_to_string(CODEX)
            .unwrap()
            .replace(CODEX_SESSION, "33333333-3333-4333-8333-333333333333");
        std::fs::write(&next, &text).unwrap();
        let home = dir.path().join("home");
        let roots = [("codex", root.as_path())];
        let shown = preview(&home, &roots).unwrap();
        let failed = run(&home, &roots, Some(&shown.key), &mut |c| {
            if matches!(c, Committed::Transcripts { .. }) {
                std::fs::write(
                    &next,
                    text.replace("Try again with 100ms", "Try again with 200ms"),
                )
                .unwrap();
            }
        })
        .unwrap_err();
        assert_eq!(failed.code, FailureCode::Stale);
        assert_eq!(failed.outcome.stats.agents["codex"].events, 8);
        assert_eq!(records(&home).len(), 8);
        let shown = preview(&home, &roots).unwrap();
        let resumed = run(&home, &roots, Some(&shown.key), &mut |_| {}).unwrap();
        assert_eq!(resumed.stats.agents["codex"].events, 8);
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
        let dir =
            std::env::temp_dir().join(format!("oboete-transcript-sub-{}", std::process::id()));
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
        // In time order: subagent files are read last but sorted into place.
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
                "PostToolUse",
                "Stop",
                "UserPromptSubmit",
                "UserPromptSubmit",
                "UserPromptSubmit",
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
        let ts: Vec<&str> = v[1..].iter().map(|e| e["ts"].as_str().unwrap()).collect();
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
        assert_eq!(v[11]["payload"]["tool_name"], "Glob");
        assert_eq!(v[11]["payload"]["agent_id"], "b2");
        assert_eq!(
            v[12]["payload"]["last_assistant_message"],
            "テストを直します。"
        );
        // Command output and a local command are no prompt; a skill command is, as typed.
        assert_eq!(v[13]["payload"]["prompt"], "/graphify src");
        assert_eq!(v[14]["payload"]["prompt"], "/goal finish the cache");
        // A plain-text local command is no prompt; a queued prompt is sent once, when queued.
        assert_eq!(v[15]["payload"]["prompt"], "テストも直して");
        assert_eq!(v[15]["ts"], "2026-09-01T00:00:20.000Z");
        // A queued prompt that starts with markup is still a prompt.
        assert_eq!(v[17]["payload"]["prompt"], "<div>見出し</div> を直して");
        // SessionEnd takes the main file's last time, not a subagent file's.
        assert_eq!(v[18]["ts"], "2026-09-01T00:00:24.000Z");
    }

    #[test]
    fn calls_without_result_end_at_the_interruption() {
        let (v, _) = events(CLAUDE, "claude");
        // Ended by the interruption at 00:00:23, placed at the time each call was made.
        assert_eq!(v[10]["payload"]["tool_name"], "Edit");
        assert_eq!(v[10]["payload"]["interrupted"], true);
        assert!(v[10]["payload"]["tool_response"].is_null());
        assert_eq!(v[16]["payload"]["tool_name"], "Bash");
        assert_eq!(v[16]["ts"], "2026-09-01T00:00:22.000Z");
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
}
