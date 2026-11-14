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
use crate::preview::FailureCode;
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
// order here: serde_json's `preserve_order`), so a line converted again keeps its bytes. Where
// an interrupted call's line stands among the others is another matter (`interrupt`, #950).
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
    /// Prompts the prompt hook got when they were queued, not yet delivered as user records, each
    /// with where its line is in `lines`.
    queued: Vec<(String, usize)>,
    /// The entry point the session's lines last named (O2): a queued prompt's line names none.
    entrypoint: Option<String>,
    stats: Stats,
    pending: Vec<Pending>,
    /// The current turn's last assistant text, sent as `Stop` when the turn ends, and the
    /// directory it was said in (the record that ends the turn may already be in another).
    last_text: Option<String>,
    turn_cwd: Option<String>,
    last_ts: String,
    /// Lines without their `seq`, keyed by the place each sorts at: its time, or for a call the
    /// session's interruption ended, that interruption's (`interrupt`). Subagent files are read
    /// after the main file, and `oboete replay` takes the lines in order, so they are sorted
    /// before they are written.
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
        // A record without a time keeps the time and the place of the one before it. They are
        // two things for a call the session's interruption ended (`interrupt`).
        let (ts, place) = match (ts, self.lines.last()) {
            ("", Some((place, before))) => (before.ts.clone(), place.clone()),
            _ => (ts.to_string(), ts.to_string()),
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
            place
        };
        self.lines.push((key, line));
        Ok(())
    }

    /// Every line in the order of its place (a stable sort: equal places keep the order they
    /// were read in), numbered, then SessionEnd.
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

    /// A prompt at `ts`; `line`, a Claude Code transcript's, says where it came from (O2): its
    /// `entrypoint`, and an `origin` or a `turnOrigin` other than a person's typing (a task's
    /// notification, a scheduled turn, the Agent SDK's).
    fn prompt(&mut self, ts: &str, text: &str, line: Option<&Value>) -> Result<()> {
        self.stop(ts)?;
        let mut payload = json!({"prompt": text});
        if let Some(sent) = self.agent_sent {
            payload[crate::capture::AGENT_SENT] = json!(sent);
        }
        if let Some(v) = line {
            mark(&mut payload, v, None);
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
        let (_, name, mut input, started, agent_id, _) = self.pending.remove(i);
        if let Some(a) = answers {
            input["answers"] = a.clone();
        }
        // When the call began, as its line's time (#692): the import reads it as it reads a line's.
        let mut p =
            json!({"tool_name": name, "tool_input": input, crate::capture::STARTED: started});
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

    /// Calls that never got a result, as interrupted, each a line with the time its call was
    /// made. At an interruption of the session (`at`, the time of the record that stopped the
    /// turn) only its own calls end, and each one's line takes that record's place among the
    /// others: a transcript imported while the call had no result yet has the lines up to there
    /// behind its checkpoint, and the call's line comes after them, not among them (#950). Its
    /// time and its bytes are what they were, so it is the line it was to a checkpoint and to a
    /// forget's denial. Subagent calls wait for their file's end (`None`), where every call left
    /// ends at its own time: those lines are synthetic, and an import leaves them out.
    fn interrupt(&mut self, at: Option<&str>) -> Result<()> {
        let (gone, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| at.is_none() || p.4.is_none());
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
            // Never before its own time: an interruption with no time of its own, or an earlier
            // one, leaves the line where it was.
            if let (Some(at), Some((place, _))) = (at, self.lines.last_mut())
                && at > place.as_str()
            {
                *place = at.to_owned();
            }
        }
        Ok(())
    }

    /// End of one file: calls that never got a result, then the turn's last text.
    fn finish(&mut self) -> Result<()> {
        let first = self.lines.len();
        self.interrupt(None)?;
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

/// O2: what a Claude Code line says of the prompt in `payload`: an `origin` or a `turnOrigin`
/// other than a person's typing (a task's notification, a scheduled turn, the Agent SDK's) makes it
/// another program's, and its `entrypoint`, or else `session`'s, is where it was sent from.
fn mark(payload: &mut Value, v: &Value, session: Option<&str>) {
    let not_typed = |origin: &Value| origin.as_str().is_some_and(|k| k != "human");
    if not_typed(&v["origin"]["kind"]) || not_typed(&v["turnOrigin"]) {
        payload[crate::capture::AGENT_SENT] = json!(true);
    }
    if let Some(e) = crate::typed::entrypoint(&v["entrypoint"]).or(session) {
        payload[crate::capture::ENTRYPOINT] = json!(e);
    }
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
        if let Some(entry) = crate::typed::entrypoint(&v["entrypoint"]) {
            e.entrypoint = Some(entry.to_owned());
        }
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
                e.interrupt(Some(&ts))?;
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
            if let Some(i) = e.queued.iter().position(|(q, _)| q == text.trim()) {
                // Its delivery says who sent it: the marks go on the prompt sent when it was
                // queued (Codex on #659).
                let (_, at) = e.queued.remove(i);
                mark(&mut e.lines[at].1.payload, v, None);
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
                    Some(command) => e.prompt(&ts, &command, Some(v)),
                    None => Ok(()),
                };
            }
            e.prompt(&ts, &text, Some(v))
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
            let mut payload = json!({"prompt": text});
            mark(&mut payload, v, e.entrypoint.as_deref());
            e.emit("UserPromptSubmit", &ts, payload)?;
            e.queued.push((t.to_string(), e.lines.len() - 1));
            Ok(())
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
                    e.prompt(&ts, &text, None)
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
            let name = p["name"].as_str().unwrap_or("?");
            // An MCP tool's name is its namespace's and its own joined by `__`, as Codex names it
            // to hooks (`join_tool_name` in codex-rs/core/src/tools/handlers/mcp.rs).
            let name = match p["namespace"].as_str() {
                Some(ns) if ns.starts_with("mcp__") => format!(
                    "{}__{}",
                    ns.trim_end_matches('_'),
                    name.trim_start_matches('_')
                ),
                _ => name.to_owned(),
            };
            e.tool_use(call_id, &name, args, &ts, None);
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
            e.interrupt(Some(&ts))
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
        entrypoint: None,
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
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub stats: ImportStats,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub outcome: Box<Outcome>,
    pub code: crate::preview::FailureCode,
    pub cause: anyhow::Error,
}

#[derive(Clone, Debug)]
pub(crate) struct Committed {
    pub agent: String,
    pub events: u64,
    pub bytes: u64,
}

#[derive(serde::Serialize)]
struct BoundFile {
    agent: String,
    path: PathBuf,
    digest: String,
    parsed: String,
    /// The session the preview read in it; none while its lines are not bound.
    session: Option<String>,
}

/// A bound file that waits keeps its session's place among the files: a later copy of the
/// session, which the preview counted as seen before, is not imported in its stead.
fn keep_place(bound: Option<&BoundFile>, sessions: &mut HashSet<(String, String)>) {
    if let Some(bound) = bound
        && let Some(session) = &bound.session
    {
        sessions.insert((bound.agent.clone(), session.clone()));
    }
}

/// What a preview bound. The viewer keeps it between a preview and its confirm (#947): the
/// confirm imports of these files the ones that are still what was bound, and no other file.
pub(crate) struct PreviewPlan {
    pub(crate) preview: Preview,
    /// The agents and their transcript folders the preview read.
    roots: Vec<(String, PathBuf)>,
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
                crate::preview::file_version(p)?,
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
                session: None,
                path,
            });
        }
    }
    Ok(files)
}

/// The import. `prepared` is a preview's plan, for a confirm of it: of the files it bound, each
/// is imported while it is still the file that was bound, the others wait for a later import,
/// and no file it did not bind is read. Without one (the CLI's `--yes`) nothing is bound.
pub(crate) fn run(
    home: &Path,
    roots: &[(&str, &Path)],
    prepared: Option<PreviewPlan>,
    committed: &mut impl FnMut(&Committed),
) -> std::result::Result<Outcome, Failure> {
    run_with_output(home, roots, prepared, committed, &mut std::io::sink())
}

fn run_with_output(
    home: &Path,
    roots: &[(&str, &Path)],
    prepared: Option<PreviewPlan>,
    committed: &mut impl FnMut(&Committed),
    out: &mut impl Write,
) -> std::result::Result<Outcome, Failure> {
    let mut outcome = Outcome::default();
    let mut code = FailureCode::Refused;
    let result = (|| {
        refuse_v1_store(home)?;
        code = FailureCode::Busy;
        let _lock = crate::import::lock(home)?;
        // A confirm is of what its preview read: the same agents' folders. What else the
        // preview's key bound is looked at again below, the settings and the destination, and
        // the files one by one as they are read.
        let mut plan = prepared;
        if let Some(plan) = &plan {
            code = FailureCode::Stale;
            let same = (plan.roots.iter())
                .map(|(agent, root)| (agent.as_str(), root.as_path()))
                .eq(roots.iter().copied());
            anyhow::ensure!(same, FailureCode::Stale);
        }
        let before_config = plan.as_ref().map(|p| p.config.clone());
        // Recheck after acquiring the same lock every settings writer uses; hold the selected
        // capture settings steady until the last transcript batch has committed.
        let _config = crate::settings::config_lock(home)?;
        code = FailureCode::Stale;
        if let Some(before) = before_config {
            anyhow::ensure!(
                crate::preview::config_bytes(home)? == before,
                FailureCode::Stale
            );
        }
        code = FailureCode::InvalidConfig;
        Settings::load(home)?;
        code = FailureCode::Stale;
        let destination = if plan.is_some() {
            raw::read_only(home)?
        } else {
            None
        };
        if let Some(plan) = &plan {
            anyhow::ensure!(
                plan.destination == crate::preview::preview_key(home, &"destination")?,
                FailureCode::Stale
            );
        }
        code = FailureCode::Failed;
        let mut raw = Some(match destination {
            Some(held) => held.into_writer(home)?,
            None => raw::open(home)?,
        });
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
            code: crate::preview::failure_code(&cause, code),
            cause,
        }),
    }
}

pub(crate) fn preview_plan(home: &Path, roots: &[(&str, &Path)]) -> Result<PreviewPlan> {
    refuse_v1_store(home)?;
    let destination = crate::preview::preview_key(home, &"destination")?;
    let config = crate::preview::config_bytes(home)?;
    let mut files = bound_files(roots)?;
    #[cfg(test)]
    if let Some(hook) = AFTER_BOUND.with_borrow_mut(Option::take) {
        hook();
    }
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
        crate::preview::config_bytes(home)? == config,
        "transcript settings changed during preview"
    );
    let key = crate::preview::preview_key(
        home,
        &(
            "transcripts",
            roots,
            &files,
            config.as_deref().map(crate::forget::hash),
            &candidates,
        ),
    )?;
    anyhow::ensure!(
        destination == crate::preview::preview_key(home, &"destination")?,
        FailureCode::Stale
    );
    Ok(PreviewPlan {
        preview: Preview { key, candidates },
        roots: (roots.iter())
            .map(|(agent, root)| ((*agent).to_owned(), root.to_path_buf()))
            .collect(),
        files,
        config,
        destination,
    })
}

/// Refuses an import while the home holds a store of oboete v1 (`<home>/oboete.db`), which this
/// build no longer migrates (decision 99, 2026-11-12): its events moved later by an older build
/// would come again beside the transcripts' copies of them. The store is not opened.
fn refuse_v1_store(home: &Path) -> Result<()> {
    anyhow::ensure!(
        !home.join("oboete.db").try_exists()?,
        "{}; nothing was imported",
        crate::setup::V1_STORE
    );
    Ok(())
}

/// Import stable transcripts locally, with the time cut and transactional checkpoints of A60
/// and D6. Preview reads only the transcripts and settings, and creates nothing.
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
    refuse_v1_store(home)?;
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
    let cut = match raw.as_ref() {
        Some(raw) => raw.earliest_by_session()?,
        None => HashMap::new(),
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
            // A file that is not the one the preview bound (a session at work has written to it
            // since) waits for a later import, and the files after it are read (#947, decision
            // 100): what a confirm stores is a part of what its preview showed, never another
            // version of a file. A preview counts such a file as waiting too, as one that
            // changes while it is read, and leaves it bound as it was with nothing parsed: it
            // authorizes no later version of the file. Once its lines are read and it is still
            // the file that was bound, a preview does not look at it again: what it counts is
            // what it bound.
            let changed = |bound: &Option<&mut BoundFile>| {
                bound.as_deref().is_some_and(|b| check_bound(b).is_err())
            };
            if changed(&bound) {
                stats.files += 1;
                stats.waiting += 1;
                keep_place(bound.as_deref(), &mut sessions);
                continue;
            }
            stats.files += 1;
            if yes && bound.as_deref().is_some_and(|file| file.parsed.is_empty()) {
                stats.waiting += 1;
                continue;
            }
            let Some(mut lines) = stable_lines(&path, agent)? else {
                stats.waiting += 1;
                keep_place(bound.as_deref(), &mut sessions);
                continue;
            };
            if changed(&bound) {
                stats.waiting += 1;
                keep_place(bound.as_deref(), &mut sessions);
                continue;
            }
            #[cfg(test)]
            if let Some(hook) = AFTER_READ.with_borrow_mut(Option::take) {
                hook();
            }
            if let Some(bound) = bound.as_deref_mut() {
                let parsed = parsed_digest(&lines)?;
                if bound.parsed.is_empty() {
                    bound.parsed = parsed;
                } else if bound.parsed != parsed {
                    // The same file by its bytes, and other lines than the preview read of it.
                    stats.waiting += 1;
                    keep_place(Some(bound), &mut sessions);
                    continue;
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
            if let Some(bound) = bound.as_deref_mut()
                && bound.session.is_none()
            {
                bound.session = Some(session.clone());
            }
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
            // A batch the store did not take for a forget's sake: the file is refused where it
            // stands, as one `transcript_cut` refuses, and the files after it are still read.
            let mut refused = None;
            // Written to while a confirm imported it (#947): nothing more of it is stored.
            let mut grew = false;
            for (mut line, event_hash) in lines.into_iter().zip(&fingerprints) {
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
                // A call's start, as the record keeps a time (#692); none when it does not read.
                if let Some(fields) = line.payload.as_object_mut()
                    && let Some(start) = fields.remove(capture::STARTED)
                    && let Some(ms) = crate::replay::fixture_ms(&clock, &start)
                {
                    fields.insert(capture::STARTED.into(), json!(ms));
                }
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
                    // A confirm leaves a file that is no longer the one it was shown where its
                    // last batch left it: the rest waits. A preview writes nothing and has the
                    // lines it read from the file it bound: it reads on, and a confirm of it
                    // finds the file changed.
                    if yes && bound.as_deref().is_some_and(|b| check_bound(b).is_err()) {
                        grew = true;
                        break;
                    }
                    if let Err(e) = append_batch(raw, &mut batch, &checkpoint, &settings, stats) {
                        refused = Some(forget_refusal(e)?);
                        break;
                    }
                    if raw.is_some() {
                        committed(&Committed {
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
            grew = grew
                || (refused.is_none()
                    && checkpoint.through > seen
                    && yes
                    && bound.as_deref().is_some_and(|b| check_bound(b).is_err()));
            if grew {
                stats.waiting += 1;
            } else if refused.is_none() && checkpoint.through > seen {
                match append_batch(raw, &mut batch, &checkpoint, &settings, stats) {
                    Ok(()) => {
                        if raw.is_some() {
                            committed(&Committed {
                                agent: (*agent).to_owned(),
                                events: stats.events,
                                bytes: stats.bytes,
                            });
                        }
                        checkpoints.insert(key, checkpoint);
                    }
                    Err(e) => refused = Some(forget_refusal(e)?),
                }
            }
            if let Some(e) = refused {
                stats.refused += 1;
                writeln!(
                    out,
                    "refused {}: {e:#}",
                    crate::redact::outbound_with(&path.display().to_string(), &settings.rules)
                )?;
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
        return Err(anyhow::anyhow!(FailureCode::Refused)).with_context(|| format!(
            "{refused} transcript file(s) refused: an imported prefix, or what a forget covers, cannot be verified; \
             review the refusal reasons above; nothing more of those files was imported"));
    }
    Ok(())
}

/// `e` when it is a batch the store refused for a forget's sake: its file is refused, and the
/// run goes on. Any other error of an append ends the run.
fn forget_refusal(e: anyhow::Error) -> Result<anyhow::Error> {
    match e.downcast_ref::<raw::ForgetRefusal>() {
        Some(_) => Ok(e),
        None => Err(e),
    }
}

fn hash_line(prefix: &mut Sha256, line: &Line) -> Result<()> {
    // A line's identity is the bytes it had when transcripts were first imported: a checkpoint's
    // prefix is made of them, and so is what a forget's denial holds of an imported record. What
    // is read of a line since is no part of it: the marks of a Claude Code line (O2; Codex on
    // #659) and a call's start (#692). A Codex rollout's mark (#273) was always part of it.
    //
    // Those fields stand in the middle of a payload, or after its last field, and `Map::remove`
    // puts the last field in the place of the one it takes out: taken out so, they left the
    // others in another order, and a line that had one lost the identity it was imported with.
    // They go with `shift_remove`, which leaves the others as they were. `transcript_path` went
    // with `remove` from the first import on, after them: that order is the identity.
    let mut payload = line.payload.clone();
    if let Some(fields) = payload.as_object_mut() {
        fields.shift_remove(crate::capture::ENTRYPOINT);
        fields.shift_remove(crate::capture::STARTED);
        if line.agent == "claude" {
            fields.shift_remove(crate::capture::AGENT_SENT);
        }
        // A copied transcript keeps its event contents; its file path is not its identity.
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
            .append_imported_under(&records, &origins, settings, Some(checkpoint))?
            .len() as u64,
        None => records.len() as u64,
    };
    stats.bytes += records
        .iter()
        .map(|c| c.event.body.len() as u64)
        .sum::<u64>();
    Ok(())
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
    if let Some(hook) = AFTER_PARSE.with_borrow_mut(Option::take) {
        hook();
    }
    // Check the file set too: a new or removed subagent changes the session's event order.
    let after = subagent_files(path, agent).and_then(|files| stamps(path, &files));
    if after.as_ref().ok() != Some(&before) {
        return Ok(None);
    }
    Ok(Some(parsed?.0))
}

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_PARSE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    /// After a preview has bound its files, before it reads the first.
    static AFTER_BOUND: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    /// After a file was read and found to be the one that was bound, before its lines are counted
    /// or written.
    static AFTER_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
mod tests;
