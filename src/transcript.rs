//! Agent transcripts as replay fixtures (docs/milestone-1-plan.md Task 4; spec 7.4, 8.4 item 1).
//! `oboete transcript <path> --agent claude|codex` prints one `{seq, agent, event, session, ts,
//! payload}` line per hook event the transcript implies: the format `oboete replay` reads, so a
//! transcript replays through today's hooks and, later, feeds the transcript import. Nothing is
//! redacted or stripped here: the hook path does that, and it is part of what a replay measures.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Claude Code records that only the transcript has: no prompt hook ever saw them. A local slash
/// command (/compact, /effort) is stored name tag first; the live hook never got one (2026-09-26:
/// the store had none of the 7 typed that week, but it had the skill command typed with them).
const TRANSCRIPT_ONLY: [&str; 6] = [
    "<command-name>",
    "<local-command-",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
    "[Request interrupted",
];

/// Harness context Codex sends as user messages; not typed prompts.
const CODEX_CONTEXT: [&str; 5] = [
    "<environment_context>",
    "<user_instructions>",
    "# AGENTS.md",
    "<permissions",
    "<INSTRUCTIONS>",
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

/// A tool call waiting for its result: (id, name, input, ts, subagent id).
type Pending = (String, String, Value, String, Option<String>);

struct Emitter<W: Write> {
    out: W,
    agent: &'static str,
    session: String,
    path: String,
    cwd: Option<String>,
    started: bool,
    stats: Stats,
    pending: Vec<Pending>,
    /// The current turn's last assistant text and its time, sent as `Stop` when the turn ends.
    last_text: Option<(String, String)>,
    last_ts: String,
}

impl<W: Write> Emitter<W> {
    fn ignore(&mut self, kind: String) {
        *self.stats.ignored.entry(kind).or_default() += 1;
    }

    fn write(&mut self, event: &str, ts: &str, mut payload: Value) -> Result<()> {
        self.stats.events += 1;
        payload["session_id"] = json!(self.session);
        payload["transcript_path"] = json!(self.path);
        payload["cwd"] = json!(self.cwd.as_deref().unwrap_or("."));
        payload["hook_event_name"] = json!(event);
        let line = json!({"seq": self.stats.events, "agent": self.agent, "event": event,
                          "session": self.session, "ts": ts, "payload": payload});
        writeln!(self.out, "{line}")?;
        Ok(())
    }

    fn emit(&mut self, event: &str, ts: &str, payload: Value) -> Result<()> {
        if !self.started {
            self.started = true;
            self.write("SessionStart", ts, json!({"source": "startup"}))?;
        }
        self.write(event, ts, payload)
    }

    fn stop(&mut self) -> Result<()> {
        match self.last_text.take() {
            Some((ts, text)) => self.emit("Stop", &ts, json!({"last_assistant_message": text})),
            None => Ok(()),
        }
    }

    fn prompt(&mut self, ts: &str, text: &str) -> Result<()> {
        self.stop()?;
        self.emit("UserPromptSubmit", ts, json!({"prompt": text}))
    }

    fn tool_use(&mut self, id: &str, name: &str, input: Value, ts: &str, agent_id: Option<&str>) {
        let entry = (
            id.into(),
            name.into(),
            input,
            ts.into(),
            agent_id.map(Into::into),
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
        let (_, name, mut input, _, agent_id) = self.pending.remove(i);
        if let Some(a) = answers {
            input["answers"] = a.clone();
        }
        let mut p = json!({"tool_name": name, "tool_input": input, "tool_response": response});
        if let Some(a) = agent_id {
            p["agent_id"] = json!(a);
        }
        match error {
            Some(e) => {
                p["error"] = json!(e);
                self.emit("PostToolUseFailure", ts, p)
            }
            None => self.emit("PostToolUse", ts, p),
        }
    }

    /// End of one file: calls that never got a result, then the turn's last text.
    fn finish(&mut self) -> Result<()> {
        for (_, name, input, ts, agent_id) in std::mem::take(&mut self.pending) {
            let mut p = json!({"tool_name": name, "tool_input": input, "tool_response": Value::Null, "interrupted": true});
            if let Some(a) = agent_id {
                p["agent_id"] = json!(a);
            }
            self.emit("PostToolUse", &ts, p)?;
        }
        self.stop()
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

fn claude_line<W: Write>(e: &mut Emitter<W>, v: &Value, agent_id: Option<&str>) -> Result<()> {
    // Older Claude Code wrote subagent turns inline, marked isSidechain; newer writes them to
    // <session>/subagents/, read with their file's agent id.
    let agent_id = if v["isSidechain"] == true {
        Some(v["agentId"].as_str().unwrap_or("sidechain"))
    } else {
        agent_id
    };
    if e.cwd.is_none() {
        e.cwd = v["cwd"].as_str().map(Into::into);
    }
    let ts = v["timestamp"].as_str().unwrap_or_default().to_string();
    let content = &v["message"]["content"];
    match v["type"].as_str() {
        Some("user") if v["isCompactSummary"] == true => e.emit(
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
            if agent_id.is_some()
                || t.is_empty()
                || TRANSCRIPT_ONLY.iter().any(|p| t.starts_with(p))
            {
                return Ok(());
            }
            // A skill command, stored message tag first, reaches the hook as typed.
            if t.starts_with("<command-message>") {
                return match command_text(t) {
                    Some(command) => e.prompt(&ts, &command),
                    None => Ok(()),
                };
            }
            e.prompt(&ts, &text)
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
                            e.last_text = Some((ts.clone(), t.to_string()));
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

fn codex_line<W: Write>(e: &mut Emitter<W>, v: &Value) -> Result<()> {
    let ts = v["timestamp"].as_str().unwrap_or_default().to_string();
    let p = &v["payload"];
    let call_id = p["call_id"].as_str().unwrap_or_default();
    match (v["type"].as_str(), p["type"].as_str()) {
        (Some("session_meta"), _) => {
            if !e.started
                && let Some(id) = p["id"].as_str()
            {
                e.session = id.to_string();
            }
            if e.cwd.is_none() {
                e.cwd = p["cwd"].as_str().map(Into::into);
            }
            Ok(())
        }
        (Some("turn_context"), _) => {
            if e.cwd.is_none() {
                e.cwd = p["cwd"].as_str().map(Into::into);
            }
            Ok(())
        }
        (Some("response_item"), Some("message")) => {
            let text = p["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["text"].as_str())
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
                    e.last_text = Some((ts, text));
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
                e.last_text = Some((ts, t.to_string()));
            }
            e.stop()
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

fn read_file<W: Write>(e: &mut Emitter<W>, path: &Path, agent_id: Option<&str>) -> Result<()> {
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
        if let Some(t) = v["timestamp"].as_str() {
            e.last_ts = t.to_string();
        }
        match e.agent {
            "claude" => claude_line(e, &v, agent_id)?,
            _ => codex_line(e, &v)?,
        }
    }
    e.finish()
}

pub fn convert(path: &Path, agent: &str, out: impl Write) -> Result<Stats> {
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
        out,
        agent,
        session: stem.to_string(),
        path: path.display().to_string(),
        cwd: None,
        started: false,
        stats: Stats::default(),
        pending: Vec::new(),
        last_text: None,
        last_ts: String::new(),
    };
    read_file(&mut e, path, None)?;
    let subagents = path.with_extension("").join("subagents");
    if agent == "claude" && subagents.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&subagents)?
            .filter_map(|d| d.ok().map(|d| d.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect();
        files.sort();
        for f in files {
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let id = stem.strip_prefix("agent-").unwrap_or(stem).to_string();
            read_file(&mut e, &f, Some(&id))?;
        }
    }
    if e.started {
        let ts = e.last_ts.clone();
        e.write("SessionEnd", &ts, json!({"reason": "transcript_end"}))?;
    }
    Ok(e.stats)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn claude_prompts_tools_answers_compaction_and_stops() {
        let (v, _) = events(CLAUDE, "claude");
        assert_eq!(
            names(&v),
            [
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "Stop",
                "UserPromptSubmit",
                "PostToolUse",
                "PostToolUseFailure",
                "PostCompact",
                "PostToolUse",
                "Stop",
                "UserPromptSubmit",
                "PostToolUse",
                "PostToolUse",
                "SessionEnd"
            ]
        );
        for (i, e) in v.iter().enumerate() {
            assert_eq!(e["seq"], i as u64 + 1);
            assert_eq!(e["session"], "claude-basic");
            assert_eq!(e["payload"]["cwd"], "/work/app");
            assert_eq!(e["payload"]["hook_event_name"], e["event"]);
        }
        assert_eq!(v[1]["payload"]["prompt"], "キャッシュの方針を決めたい");
        assert_eq!(v[1]["ts"], "2026-09-01T00:00:00.000Z");
        assert_eq!(v[2]["payload"]["tool_name"], "Read");
        assert_eq!(
            v[3]["payload"]["last_assistant_message"],
            "SQLite にします。"
        );
        assert_eq!(v[4]["payload"]["prompt"], "どちらが良い？");
        assert_eq!(
            v[5]["payload"]["tool_input"]["answers"]["どちらにしますか?"],
            "A にする"
        );
        assert_eq!(v[6]["payload"]["error"], "error: 2 tests failed");
        assert!(
            v[7]["payload"]["compact_summary"]
                .as_str()
                .unwrap()
                .contains("SQLite")
        );
        // The inline subagent: its tool call only, never its task as a prompt or its text as a Stop.
        assert_eq!(v[8]["payload"]["tool_name"], "Glob");
        assert_eq!(v[8]["payload"]["agent_id"], "b2");
        assert_eq!(
            v[9]["payload"]["last_assistant_message"],
            "テストを直します。"
        );
        // Command output and a local command are no prompt; a skill command is, as typed.
        assert_eq!(v[10]["payload"]["prompt"], "/graphify src");
        assert_eq!(v[12]["payload"]["tool_name"], "Grep");
        assert_eq!(v[12]["payload"]["agent_id"], "a1");
    }

    #[test]
    fn a_tool_call_without_result_is_emitted_at_the_end() {
        let (v, _) = events(CLAUDE, "claude");
        assert_eq!(v[11]["payload"]["tool_name"], "Edit");
        assert_eq!(v[11]["payload"]["interrupted"], true);
        assert!(v[11]["payload"]["tool_response"].is_null());
    }

    #[test]
    fn unknown_and_broken_lines_are_skipped() {
        let (v, stats) = events(CLAUDE, "claude");
        let ignored = [
            ("atis-latch".to_string(), 1),
            ("permission-mode".to_string(), 1),
        ]
        .into();
        assert_eq!(
            stats,
            Stats {
                lines: 24,
                skipped: 1,
                events: 14,
                ignored
            }
        );
        assert_eq!(v.len(), 14);
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
                "SessionEnd"
            ]
        );
        for e in &v {
            assert_eq!(e["session"], "22222222-2222-4222-8222-222222222222");
            assert_eq!(e["payload"]["cwd"], "/work/svc");
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
    }

    #[test]
    fn a_parsed_transcript_replays_through_the_hook_path() {
        let home = std::env::temp_dir().join(format!("oboete-transcript-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let conn = crate::db::open(&home).unwrap();
        let (v, _) = events(CLAUDE, "claude");
        for e in &v {
            crate::hook::handle(&conn, "claude", e["event"].as_str().unwrap(), &e["payload"])
                .unwrap();
        }
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert!(count("SELECT count(*) FROM events WHERE session_id = 'claude-basic'") >= 8);
        // The developer's answer reaches the store, where observe reads it (src/observe.rs `answers`).
        assert_eq!(
            count(
                "SELECT count(*) FROM events WHERE session_id = 'claude-basic' AND payload LIKE '%A にする%'"
            ),
            1
        );
        drop(conn);
        std::fs::remove_dir_all(&home).ok();
    }
}
