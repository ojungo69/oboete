//! Hook path: one agent event in on stdin, one row out. Must be fast and must never fail the agent.
//! Claude Code, Codex and Grok Build share one JSON dialect; Grok also sends camelCase copies and
//! runs Claude Code's hooks as a compatibility layer, which is handled in `resolve_agent`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::{config, db, inject, redact, repo};

/// Largest text kept per field. Tool outputs beyond this are clipped with a marker.
const MAX_FIELD: usize = 8_000;
/// Redaction looks this far past the clip point, so a secret straddling it is masked whole when
/// it fits; a longer block (an RSA-8192 PEM is ~6,400 chars) is cut at its BEGIN line instead.
const REDACT_OVERLAP: usize = 4_000;
/// Set on the CLIs observe spawns, so the summarizer's own session is never captured.
pub const SKIP_ENV: &str = "OBOETE_SKIP";
/// Blocks inside a prompt that are not part of what was asked, removed before anything is
/// stored, in this order: context an IDE or another memory tool puts in front of the text (it may
/// quote `<private>`), then `<private>`, the developer's opt-out (claude-mem's tag; unclosed, it
/// hides the rest, where claude-mem would keep it all).
const STRIP_BLOCKS: &[&str] = &["ide_opened_file", "hook_context", "private"];
/// Prompts that are harness traffic, not typed: background task and teammate notifications and
/// the /loop sentinel (13.5% of the 15,218 prompts claude-mem stored on this machine).
/// ponytail: fixed prefix list; add one when a new envelope shows up as a prompt card.
const ENVELOPES: &[&str] = &[
    "<task-notification",
    "<agent-message",
    "<system_notification",
    "<bash-notification",
    "<<autonomous-loop",
];

pub fn run_stdin(home: &Path, agent: &str, event: &str) -> Result<()> {
    run_io(
        home,
        agent,
        event,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

fn run_io(
    home: &Path,
    agent: &str,
    event: &str,
    mut input: impl Read,
    mut output: impl Write,
) -> Result<()> {
    let result: Result<Option<String>> = (|| {
        if std::env::var_os(SKIP_ENV).is_some() {
            return Ok(None);
        }
        let mut raw = String::new();
        input.read_to_string(&mut raw)?;
        let payload: Value = if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&raw)?
        };
        let Some(agent) = resolve_agent(agent, &payload, &grok_hooks_file()) else {
            return Ok(None);
        };
        if (agent == "agy" && agy_workspace(&payload).is_none()) || is_agent_internal(&payload) {
            return Ok(None);
        }
        std::fs::create_dir_all(home)?;
        let conn = db::open(home)?;
        let out = handle(&conn, agent, event, &payload)?;
        if matches!(event, "Stop" | "SessionEnd") && std::env::var_os("OBOETE_NO_SPAWN").is_none() {
            // agy has no SessionEnd: its last turn only becomes pending once it has settled, so
            // this observer waits out the settle window instead of finding nothing now.
            spawn_observe(home, (agent == "agy").then_some(AGY_OBSERVE_WAIT_MS));
        }
        Ok(out)
    })();
    if let Ok(Some(out)) = &result {
        writeln!(output, "{out}")?;
    } else if agent == "agy" {
        // Strict protojson: no Claude-shaped output, including skip and error paths.
        writeln!(output, "{{}}")?;
    }
    result.map(|_| ())
}

/// The hook file `oboete setup grok` writes. While it exists, Grok delivers its own events.
pub fn grok_hooks_file() -> PathBuf {
    grok_home().join("hooks").join("oboete.json")
}

/// Grok Build's config directory: `$GROK_HOME`, else `~/.grok`.
pub fn grok_home() -> PathBuf {
    std::env::var_os("GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home_dir().join(".grok"))
}

/// Grok Build runs Claude Code's hooks too, with its own camelCase payload: such an event is
/// Grok's, and a duplicate when Grok's own hook file carries our handler (`None` = drop it).
/// Cursor imports them as well; its payload names no cwd, so the session would land in the
/// hook's own directory and be handed that repository's context. Dropped until oboete has a
/// Cursor adapter (docs/research/agent-adapters-2026-09-23.md).
pub fn resolve_agent<'a>(agent: &'a str, payload: &Value, grok_hooks: &Path) -> Option<&'a str> {
    if agent == "claude" && payload.get("cursor_version").is_some() {
        return None;
    }
    if agent == "claude" && payload.get("hookEventName").is_some() {
        return if grok_delivers(grok_hooks) {
            None
        } else {
            Some("grok")
        };
    }
    Some(agent)
}

/// Grok delivers our events itself while its hook file holds our handler. The file may also
/// hold the developer's own entries (setup keeps them), which prove nothing.
fn grok_delivers(grok_hooks: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(grok_hooks) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    root["hooks"].as_object().is_some_and(|events| {
        events
            .values()
            .flat_map(|groups| groups.as_array().into_iter().flatten())
            .any(crate::setup::has_ours)
    })
}

/// Directories (under the home) where agents run housekeeping sessions of their own: Codex's
/// memory consolidation works in `~/.codex/memories`. Only these are skipped; a repository the
/// developer keeps elsewhere under `~/.codex` or `~/.claude` (a plugin, say) is real work.
const HOUSEKEEPING_DIRS: &[&str] = &[".codex/memories"];

fn is_agent_internal(payload: &Value) -> bool {
    let Some(cwd) = str_field(payload, &["cwd", "workspaceRoot"])
        .map(str::to_owned)
        .or_else(|| agy_workspace(payload))
    else {
        return false;
    };
    let home = config::home_dir();
    HOUSEKEEPING_DIRS
        .iter()
        .any(|d| Path::new(&cwd).starts_with(home.join(d)))
}

/// agy's hook cwd is its config directory, so only its explicit workspace can identify a repo.
fn agy_workspace(payload: &Value) -> Option<String> {
    let workspace = payload["workspacePaths"].as_array()?.first()?.as_str()?;
    let path = if let Some(uri) = workspace.strip_prefix("file://") {
        let uri = uri
            .strip_prefix("localhost/")
            .map_or_else(|| uri.to_string(), |path| format!("/{path}"));
        let decoded = percent_encoding::percent_decode_str(&uri)
            .decode_utf8()
            .ok()?;
        #[cfg(windows)]
        let decoded = decoded
            .strip_prefix('/')
            .filter(|p| p.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&decoded);
        decoded.to_string()
    } else {
        workspace.to_string()
    };
    Path::new(&path).is_absolute().then_some(path)
}

/// Store the event. Returns stdout JSON for context injection, or an empty object for agy.
pub fn handle(
    conn: &Connection,
    agent: &str,
    event: &str,
    payload: &Value,
) -> Result<Option<String>> {
    let workspace = (agent == "agy").then(|| agy_workspace(payload)).flatten();
    if agent == "agy" && workspace.is_none() {
        return Ok(Some("{}".into()));
    }
    let session_id = str_field(
        payload,
        &[
            "session_id",
            "sessionId",
            "conversation_id",
            "conversationId",
        ],
    )
    .unwrap_or("unknown");
    let cwd = workspace
        .as_deref()
        .or_else(|| str_field(payload, &["cwd", "workspaceRoot"]))
        .unwrap_or(".");
    let repo_key = repo::key(Path::new(cwd));
    let ts = db::now_ms();

    // agy has no prompt/output fields. Parse at most one bounded transcript tail per hook.
    let steps: Vec<Value> =
        if agent == "agy" && matches!(event, "PreInvocation" | "PostToolUse" | "Stop") {
            str_field(payload, &["transcriptPath"])
                .map(|p| transcript_tail(Path::new(p)))
                .unwrap_or_default()
                .lines()
                .rev()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        } else {
            Vec::new()
        };
    // File order is not step order. Select the newest explicit user step, then unwrap only
    // USER_REQUEST: metadata and settings changes must never become part of the prompt.
    let agy_prompt = if matches!(event, "PreInvocation" | "Stop") {
        steps
            .iter()
            .filter(|s| s["type"] == "USER_INPUT" && s["source"] == "USER_EXPLICIT")
            .max_by_key(|s| s["step_index"].as_i64())
            .and_then(|s| {
                let step = s["step_index"].as_i64().filter(|i| *i >= 0)?;
                let (_, request) = s["content"].as_str()?.split_once("<USER_REQUEST>")?;
                let (request, _) = request.split_once("</USER_REQUEST>")?;
                Some((step, request))
            })
    } else {
        None
    };
    let mut prompt = agy_prompt
        .map(|(_, p)| p)
        .or_else(|| {
            (event == "UserPromptSubmit").then(|| str_field(payload, &["prompt"]).unwrap_or(""))
        })
        .map(|p| clip(&strip_blocks(p, true)))
        .filter(|p| !p.is_empty());
    let tool_step = payload["stepIdx"].as_i64().and_then(|index| {
        steps
            .iter()
            .find(|s| s["step_index"].as_i64() == Some(index))
    });
    let event = if agent == "agy"
        && event == "PostToolUse"
        && (payload["error"].as_str().is_some_and(|s| !s.is_empty())
            || tool_step.is_some_and(|s| s["status"] == "ERROR"))
    {
        "PostToolUseFailure"
    } else {
        event
    };
    let stored = match event {
        "SessionStart" => Some(json!({"source": payload.get("source")})),
        "PostToolUse" | "PostToolUseFailure" => {
            let tool = if agent == "agy" {
                &payload["toolCall"]
            } else {
                payload
            };
            let output = if agent == "agy" {
                tool_step
                    .map(|s| {
                        s.get("content")
                            .filter(|v| v.as_str().is_some_and(|t| !t.is_empty()))
                            .unwrap_or(&s["error"])
                    })
                    .unwrap_or(&Value::Null)
            } else {
                field(
                    payload,
                    &["tool_response", "toolResult", "tool_output", "error"],
                )
            };
            Some(json!({
                "tool": str_field(tool, &["tool_name", "toolName", "name"]).unwrap_or("?"),
                "input": clip(&compact(field(tool, &["tool_input", "toolInput", "args"]))),
                "output": clip(&compact(output)),
                "failed": event == "PostToolUseFailure",
            }))
        }
        "Stop" if agent == "agy" => {
            // Only this turn's answer: a turn that ended without one must not reuse the last.
            let turn_start = agy_prompt.map_or(-1, |(step, _)| step);
            let text = steps
                .iter()
                .filter(|s| {
                    s["type"] == "PLANNER_RESPONSE"
                        && s["step_index"].as_i64().is_some_and(|i| i > turn_start)
                        && s["content"].as_str().is_some_and(|t| !t.trim().is_empty())
                })
                .max_by_key(|s| s["step_index"].as_i64())
                .and_then(|s| s["content"].as_str())
                .unwrap_or("");
            Some(json!({"assistant": clip(text)}))
        }
        "Stop" => {
            let text = match str_field(payload, &["last_assistant_message", "lastAssistantMessage"])
            {
                Some(t) => t.to_string(),
                // Codex's Stop carries no message; its rollout transcript does.
                None => str_field(payload, &["transcript_path"])
                    .map(|p| last_assistant_in_transcript(Path::new(p)))
                    .unwrap_or_default(),
            };
            (!text.trim().is_empty()).then(|| json!({"assistant": clip(&text)}))
        }
        "PostCompact" => str_field(payload, &["compact_summary"])
            .filter(|s| !s.trim().is_empty())
            .map(|s| json!({"summary": clip(s)})),
        "SessionEnd" => Some(json!({"reason": payload.get("reason")})),
        _ => None, // PreToolUse and the rest carry nothing a summary needs
    };
    // One transaction: the viewer deleting this session between the two writes would otherwise
    // leave an event that belongs to no session and never gets summarized or removed.
    let tx = conn.unchecked_transaction()?;
    db::upsert_session(&tx, session_id, agent, &repo_key, cwd, ts)?;
    if let Some((step, _)) = agy_prompt
        && !db::claim_prompt_step(&tx, session_id, step)?
    {
        prompt = None;
    }
    if event == "SessionEnd" {
        db::end_session(&tx, session_id, ts)?;
    }
    // The event feeds the summary and goes with it; the prompt itself is kept as a document.
    if let Some(p) = prompt.as_deref() {
        db::insert_event(
            &tx,
            session_id,
            "UserPromptSubmit",
            ts,
            &json!({"prompt": p}).to_string(),
        )?;
        if !is_envelope(p) {
            db::insert_prompt(&tx, session_id, ts, p)?;
        }
    }
    if let Some(v) = stored {
        db::insert_event(&tx, session_id, event, ts, &v.to_string())?;
    }
    // Claude Code and Codex read context at SessionStart (not on resume: the transcript already
    // has it; after a compaction it is gone, so `compact` gets it again). Grok ignores
    // SessionStart stdout, so its context rides on the first tool call. agy reads PreInvocation.
    // The check and marker share the write transaction so simultaneous hooks cannot inject twice.
    let inject_now = match event {
        "SessionStart" => {
            !matches!(agent, "grok" | "agy") && payload["source"].as_str() != Some("resume")
        }
        "PreToolUse" => agent == "grok" && !db::injected(&tx, session_id)?,
        "PreInvocation" => agent == "agy" && !db::injected(&tx, session_id)?,
        _ => false,
    };
    let mut out = (agent == "agy").then(|| "{}".into());
    if inject_now {
        let text = inject::context(&tx, &repo_key)?;
        if !text.is_empty() {
            db::mark_injected(&tx, session_id, ts)?;
            out = Some(if agent == "agy" {
                json!({"injectSteps": [{"ephemeralMessage": text}]})
            } else {
                json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}})
            }.to_string());
        }
    }
    tx.commit()?;
    Ok(out)
}

/// The text without the blocks in `STRIP_BLOCKS` (`<tag>` or `<tag attr…>` up to its own
/// `</tag>`, nesting counted), trimmed. An unclosed `<private>` hides the rest only when
/// `unclosed_private_hides_rest` (a typed prompt); anywhere else the tag is just text an agent
/// read or wrote, and cutting there would drop the rest of the session.
pub fn strip_blocks(s: &str, unclosed_private_hides_rest: bool) -> String {
    let mut out = s.to_string();
    for tag in STRIP_BLOCKS {
        let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
        let mut from = 0;
        while let Some(at) = out[from..].find(&open).map(|i| from + i) {
            let rest = &out[at + open.len()..];
            if !opens(rest) {
                from = at + open.len();
                continue;
            }
            match block_end(rest, &open, &close) {
                Some(end) => out.replace_range(at..at + open.len() + end, ""),
                None if unclosed_private_hides_rest && *tag == "private" => out.truncate(at),
                None => break,
            }
            from = at;
        }
    }
    out.trim().to_string()
}

/// What follows `<tag` makes it the tag (`<privateer>` is not `<private`).
fn opens(after: &str) -> bool {
    after.starts_with(|c: char| c == '>' || c.is_whitespace())
}

/// The end (past `close`) of the block whose opener comes just before `rest`, or `None` when it
/// never closes. An inner opener needs its own close first.
fn block_end(rest: &str, open: &str, close: &str) -> Option<usize> {
    let (mut depth, mut i) = (1, 0);
    while depth > 0 {
        let c = i + rest[i..].find(close)?;
        let inner = rest[i..c]
            .match_indices(open)
            .map(|(j, _)| i + j)
            .find(|&o| opens(&rest[o + open.len()..]));
        match inner {
            Some(o) => {
                depth += 1;
                i = o + open.len();
            }
            None => {
                depth -= 1;
                i = c + close.len();
            }
        }
    }
    Some(i)
}

/// A prompt the harness sent rather than the developer typed (see `ENVELOPES`).
pub fn is_envelope(prompt: &str) -> bool {
    ENVELOPES.iter().any(|e| prompt.starts_with(e))
}

fn field<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter().find_map(|k| v.get(*k)).unwrap_or(&Value::Null)
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str))
}

fn compact(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Redact the head (plus the overlap) and keep MAX_FIELD characters of it: scanning the
/// discarded tail of a megabyte tool output would only cost hook time.
fn clip(s: &str) -> String {
    let total = s.chars().count();
    if total <= MAX_FIELD {
        return redact::redact(s);
    }
    let window: String = s.chars().take(MAX_FIELD + REDACT_OVERLAP).collect();
    let head: String = redact::redact(&window).chars().take(MAX_FIELD).collect();
    // Second pass over what is kept: gitleaks' line-scoped allowlists must judge the stored
    // line, not the wider window. Then drop a key block the rules could not match (no END).
    let mut head = redact::redact(&head);
    if let Some(begin) = head.rfind("-----BEGIN")
        && !head[begin..].contains("-----END")
    {
        head.truncate(begin);
    }
    format!("{head}\n…[clipped, {total} chars in full]")
}

/// A fixed-size tail even if the agent appends while we read. Discard the first partial line.
fn transcript_tail(path: &Path) -> String {
    const TAIL: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > TAIL {
        use std::io::Seek;
        if f.seek(std::io::SeekFrom::Start(len - TAIL)).is_err() {
            return String::new();
        }
    }
    let mut bytes = Vec::new();
    if f.take(TAIL).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    if len > TAIL {
        let Some(newline) = bytes.iter().position(|b| *b == b'\n') else {
            return String::new();
        };
        bytes.drain(..=newline);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Last assistant `output_text` in a Codex rollout JSONL, reading only the file's tail.
fn last_assistant_in_transcript(path: &Path) -> String {
    let text = transcript_tail(path);
    let mut last = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "response_item"
            || v["payload"]["type"] != "message"
            || v["payload"]["role"] != "assistant"
        {
            continue;
        }
        if let Some(t) = v["payload"]["content"]
            .as_array()
            .and_then(|c| c.iter().rev().find_map(|x| x["text"].as_str()))
        {
            last = t.to_string();
        }
    }
    last
}

/// Detached `oboete observe` in its own process group, so the agent exiting right after
/// SessionEnd does not take it down; the lock inside observe makes duplicates harmless.
/// The observe settle window (60 s by default) plus a margin.
const AGY_OBSERVE_WAIT_MS: u64 = 65_000;

fn spawn_observe(home: &Path, wait_ms: Option<u64>) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--home").arg(home).arg("observe");
    if let Some(ms) = wait_ms {
        cmd.arg("--wait-ms").arg(ms.to_string());
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "oboete-hook-{name}-{}-{}",
            std::process::id(),
            db::now_ms()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn agy_fixture(dir: &Path) -> Value {
        let transcript = dir.join("transcript_full.jsonl");
        std::fs::write(
            &transcript,
            include_str!("testdata/agy/transcript_full.jsonl"),
        )
        .unwrap();
        let mut payloads: Value =
            serde_json::from_str(include_str!("testdata/agy/payloads.json")).unwrap();
        for payload in payloads.as_object_mut().unwrap().values_mut() {
            payload["transcriptPath"] = json!(transcript);
            payload["workspacePaths"] = json!([dir]);
        }
        payloads
    }

    #[test]
    fn agy_session_start_uses_conversation_and_workspace_without_injection() {
        let dir = tmp("agy-start");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let payload = &payloads["SessionStart"];
        assert_eq!(
            handle(&conn, "agy", "SessionStart", payload).unwrap(),
            Some("{}".into())
        );
        let id = payload["conversationId"].as_str().unwrap();
        let events = db::session_events(&conn, id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "SessionStart");
        let (agent, cwd, repo): (String, String, String) = conn
            .query_row(
                "SELECT agent, cwd, repo FROM sessions WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, "agy");
        assert_eq!(cwd, dir.to_string_lossy());
        assert_eq!(repo, repo::key(&dir));
        assert!(!db::injected(&conn, id).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_prompt_is_captured_once_even_after_observe_deletes_raw_events() {
        let dir = tmp("agy-prompt");
        let mut conn = db::open(&dir).unwrap();
        let mut payloads = agy_fixture(&dir);
        let id = payloads["PreInvocation"]["conversationId"]
            .as_str()
            .unwrap()
            .to_owned();
        // A failed prompt insert must not advance the durable cursor or leave its raw event.
        conn.execute_batch(
            "CREATE TRIGGER refuse_prompt BEFORE INSERT ON prompts
            BEGIN SELECT RAISE(ABORT, 'test insert failure'); END;",
        )
        .unwrap();
        assert!(handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"]).is_err());
        conn.execute_batch("DROP TRIGGER refuse_prompt;").unwrap();
        for invocation in 0..3 {
            payloads["PreInvocation"]["invocationNum"] = json!(invocation);
            handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"]).unwrap();
        }
        handle(&conn, "agy", "Stop", &payloads["Stop"]).unwrap();
        let expected = "Read the file hello.txt with your file viewing tool, then run the shell command 'ls /nonexistent-dir' and tell me the secret word and the error.";
        let prompts: Vec<String> = conn
            .prepare("SELECT body FROM prompts")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(prompts, [expected]);
        let events = db::session_events(&conn, &id).unwrap();
        let prompts: Vec<_> = events
            .iter()
            .filter(|e| e.event == "UserPromptSubmit")
            .collect();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&prompts[0].payload).unwrap(),
            json!({"prompt":expected})
        );
        assert_eq!(
            crate::search::search(&conn, "nonexistent-dir", None, 10)
                .unwrap()
                .len(),
            1
        );

        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: id.clone(),
                agent: "agy".into(),
                repo: repo::key(&dir),
                last_event_at: db::now_ms(),
            },
            "test",
            "",
            &[],
            i64::MAX,
        )
        .unwrap();
        assert!(db::session_events(&conn, &id).unwrap().is_empty());
        drop(conn);
        let conn = db::open(&dir).unwrap();
        handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"]).unwrap();
        handle(&conn, "agy", "Stop", &payloads["Stop"]).unwrap();
        assert!(
            db::session_events(&conn, &id)
                .unwrap()
                .iter()
                .all(|e| e.event != "UserPromptSubmit")
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stop_without_an_answer_does_not_reuse_the_previous_turns() {
        let dir = tmp("agy-noanswer");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let transcript = dir.join("transcript_full.jsonl");
        let mut text = std::fs::read_to_string(&transcript).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"step_index": 11, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
                   "content": "<USER_REQUEST>\nsecond question\n</USER_REQUEST>"})
        ));
        std::fs::write(&transcript, text).unwrap();
        handle(&conn, "agy", "Stop", &payloads["Stop"]).unwrap();
        let id = payloads["Stop"]["conversationId"].as_str().unwrap();
        let events = db::session_events(&conn, id).unwrap();
        let stop = events.iter().find(|e| e.event == "Stop").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stop.payload).unwrap(),
            json!({"assistant": ""})
        );
        assert!(events.iter().any(|e| e.payload.contains("second question")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_tool_outputs_match_step_indices_and_failures_have_both_signals() {
        let dir = tmp("agy-tool");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let mut payload = payloads["PostToolUse"].clone();
        let id = payload["conversationId"].as_str().unwrap().to_owned();
        assert_eq!(
            handle(&conn, "agy", "PostToolUse", &payload).unwrap(),
            Some("{}".into())
        );
        // Step 2 is before step 1 in the real fixture; the hook itself reports no error.
        payload["stepIdx"] = json!(2);
        handle(&conn, "agy", "PostToolUse", &payload).unwrap();
        // A hook error also fails a step that the transcript calls DONE.
        payload["stepIdx"] = json!(3);
        payload["error"] = json!("tool hook failed");
        handle(&conn, "agy", "PostToolUse", &payload).unwrap();
        let events = db::session_events(&conn, &id).unwrap();
        assert_eq!(
            events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            ["PostToolUse", "PostToolUseFailure", "PostToolUseFailure"]
        );
        let value: Value = serde_json::from_str(&events[0].payload).unwrap();
        assert_eq!(value["tool"], "run_command");
        assert_eq!(value["input"], payload["toolCall"]["args"].to_string());
        assert_eq!(
            value["output"],
            "Created At: 2026-09-24T06:55:35+09:00\nCompleted At: 2026-09-24T06:55:35+09:00\n\nThe command exited with code 2.\nOutput:\nls: cannot access '/nonexistent-dir': No such file or directory\r\n\n"
        );
        assert_eq!(value["failed"], false);
        let failure: Value = serde_json::from_str(&events[1].payload).unwrap();
        assert_eq!(failure["failed"], true);
        assert!(
            failure["output"]
                .as_str()
                .unwrap()
                .contains("Encountered error in step execution")
        );

        // Some error steps have no content at all.
        std::fs::write(
            dir.join("transcript_full.jsonl"),
            "{\"step_index\":3,\"status\":\"ERROR\",\"error\":\"permission denied\"}\n",
        )
        .unwrap();
        payload["error"] = json!("");
        handle(&conn, "agy", "PostToolUse", &payload).unwrap();
        let events = db::session_events(&conn, &id).unwrap();
        assert_eq!(events[3].event, "PostToolUseFailure");
        let failure: Value = serde_json::from_str(&events[3].payload).unwrap();
        assert_eq!(failure["output"], "permission denied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stop_recovers_unflushed_prompt_before_the_assistant_answer() {
        let dir = tmp("agy-stop");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let transcript = dir.join("transcript_full.jsonl");
        std::fs::remove_file(&transcript).unwrap();
        handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"]).unwrap();
        std::fs::write(
            &transcript,
            include_str!("testdata/agy/transcript_full.jsonl"),
        )
        .unwrap();
        assert_eq!(
            handle(&conn, "agy", "Stop", &payloads["Stop"]).unwrap(),
            Some("{}".into())
        );
        let id = payloads["Stop"]["conversationId"].as_str().unwrap();
        let events = db::session_events(&conn, id).unwrap();
        assert_eq!(
            events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            ["UserPromptSubmit", "Stop"]
        );
        let answer: Value = serde_json::from_str(&events[1].payload).unwrap();
        assert_eq!(
            answer["assistant"],
            "[hello.txt](file:///home/dev/proj/hello.txt) の確認およびコマンド実行結果は以下のとおりです。\n\n- **秘密の言葉（secret word）**: `pineapple`\n- **コマンド実行時のエラー**:\n  ```text\n  ls: cannot access '/nonexistent-dir': No such file or directory\n  ```"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_injects_context_once_at_preinvocation_in_its_own_json_shape() {
        let dir = tmp("agy-inject");
        let mut conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        assert_eq!(
            handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"]).unwrap(),
            Some("{}".into())
        );
        let id = payloads["PreInvocation"]["conversationId"]
            .as_str()
            .unwrap();
        assert!(!db::injected(&conn, id).unwrap());
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: id.into(),
                agent: "agy".into(),
                repo: repo::key(&dir),
                last_event_at: db::now_ms(),
            },
            "test",
            "earlier summary",
            &[],
            i64::MAX,
        )
        .unwrap();
        assert_eq!(
            handle(&conn, "agy", "SessionStart", &payloads["SessionStart"]).unwrap(),
            Some("{}".into())
        );
        let out = handle(&conn, "agy", "PreInvocation", &payloads["PreInvocation"])
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({
                "injectSteps": [{"ephemeralMessage": "# oboete: what happened before in this repository\n\n## Recent sessions (newest first)\n- earlier summary\n"}]
            })
        );
        assert!(db::injected(&conn, id).unwrap());
        drop(conn);
        let conn = db::open(&dir).unwrap();
        for event in ["PreInvocation", "Stop", "SessionStart", "PreInvocation"] {
            assert_eq!(
                handle(&conn, "agy", event, &payloads[event]).unwrap(),
                Some("{}".into())
            );
        }
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.clone();
                let mut payload = payloads["PreInvocation"].clone();
                payload["conversationId"] = json!("simultaneous");
                std::thread::spawn(move || {
                    handle(&db::open(&dir).unwrap(), "agy", "PreInvocation", &payload)
                        .unwrap()
                        .unwrap()
                })
            })
            .collect();
        let responses: Vec<_> = workers.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(responses.iter().filter(|s| s.as_str() != "{}").count(), 1);
        let events = db::session_events(&conn, "simultaneous").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "UserPromptSubmit");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stdout_is_an_object_even_when_input_or_storage_fails() {
        let dir = tmp("agy-fail-open");
        let payloads = agy_fixture(&dir);
        let mut output = Vec::new();
        assert!(
            run_io(
                &dir,
                "agy",
                "SessionStart",
                &b"invalid JSON"[..],
                &mut output
            )
            .is_err()
        );
        assert_eq!(output, b"{}\n");
        let blocked_home = dir.join("not-a-directory");
        std::fs::write(&blocked_home, "x").unwrap();
        output.clear();
        assert!(
            run_io(
                &blocked_home,
                "agy",
                "SessionStart",
                payloads["SessionStart"].to_string().as_bytes(),
                &mut output
            )
            .is_err()
        );
        assert_eq!(output, b"{}\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_empty_workspaces_do_not_create_storage_and_file_uris_are_accepted() {
        let dir = tmp("agy-workspaces");
        let conn = db::open(&dir).unwrap();
        let mut payloads = agy_fixture(&dir);
        let unused_home = dir.join("unused");
        for (event, payload) in payloads.as_object_mut().unwrap() {
            payload["workspacePaths"] = json!([]);
            // Neither a Claude-shaped field nor the process cwd can stand in for a workspace.
            payload["cwd"] = json!(dir);
            assert_eq!(
                handle(&conn, "agy", event, payload).unwrap(),
                Some("{}".into())
            );
            let mut output = Vec::new();
            run_io(
                &unused_home,
                "agy",
                event,
                payload.to_string().as_bytes(),
                &mut output,
            )
            .unwrap();
            assert_eq!(output, b"{}\n");
        }
        assert!(!unused_home.exists());
        for table in ["sessions", "events", "prompts", "fts"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let workspace = dir.join("workspace with spaces");
        std::fs::create_dir_all(&workspace).unwrap();
        let payload = &mut payloads["SessionStart"];
        let uri_path = workspace
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20");
        payload["workspacePaths"] = json!([format!(
            "file://{}{uri_path}",
            if cfg!(windows) { "/" } else { "" }
        )]);
        handle(&conn, "agy", "SessionStart", payload).unwrap();
        let cwd: String = conn
            .query_row("SELECT cwd FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(Path::new(&cwd), workspace);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_later_steps_use_the_shared_prompt_privacy_and_envelope_rules() {
        use std::io::Write;
        let dir = tmp("agy-prompt-rules");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let payload = &payloads["PreInvocation"];
        let transcript = dir.join("transcript_full.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        for (index, request) in [
            (11, "same request"),
            (12, "same request"),
            (13, "<private>private only</private>"),
            (14, "<task-notification>done</task-notification>"),
            (
                15,
                "<hook_context>not asked</hook_context>key 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",
            ),
        ] {
            writeln!(file, "{}", json!({"step_index":index, "type":"USER_INPUT", "source":"USER_EXPLICIT",
                "content":format!("<USER_REQUEST>{request}</USER_REQUEST><ADDITIONAL_METADATA>not asked</ADDITIONAL_METADATA>")})).unwrap();
            // Later file lines can have lower indices; implicit input is never the user's turn.
            writeln!(
                file,
                "{}",
                include_str!("testdata/agy/transcript_full.jsonl")
                    .lines()
                    .next()
                    .unwrap()
            )
            .unwrap();
            writeln!(file, "{}", json!({"step_index":99, "type":"USER_INPUT", "source":"MODEL", "content":"<USER_REQUEST>not asked</USER_REQUEST>"})).unwrap();
            for _ in 0..2 {
                handle(&conn, "agy", "PreInvocation", payload).unwrap();
            }
        }
        let prompts: Vec<String> = conn
            .prepare("SELECT body FROM prompts ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(prompts, ["same request", "same request", "key [REDACTED]"]);
        let events =
            db::session_events(&conn, payload["conversationId"].as_str().unwrap()).unwrap();
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|e| !e.payload.contains("private only")
            && !e.payload.contains("not asked")
            && !e.payload.contains("gsk_")));
        assert!(events[2].payload.contains("task-notification"));
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_reads_only_a_bounded_tail_and_uses_step_order_for_the_last_answer() {
        use std::io::Write;
        let dir = tmp("agy-tail");
        let conn = db::open(&dir).unwrap();
        let payloads = agy_fixture(&dir);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("transcript_full.jsonl"))
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":11, "type":"GENERIC", "content":"x".repeat(2 * 1024 * 1024)})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":14, "type":"PLANNER_RESPONSE", "content":"latest answer"})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":12, "type":"PLANNER_RESPONSE", "content":"earlier answer"})
        )
        .unwrap();
        writeln!(file, "{}", json!({"step_index":15, "type":"PLANNER_RESPONSE", "content":"  ", "thinking":"never captured"})).unwrap();
        // A partial final write must not hide the last complete response.
        write!(file, "{{\"step_index\":16").unwrap();
        for event in ["PreInvocation", "PostToolUse", "Stop"] {
            handle(&conn, "agy", event, &payloads[event]).unwrap();
        }
        let events =
            db::session_events(&conn, payloads["Stop"]["conversationId"].as_str().unwrap())
                .unwrap();
        assert_eq!(events.len(), 2); // The prompt and old tool output are outside the tail.
        let tool: Value = serde_json::from_str(&events[0].payload).unwrap();
        assert_eq!(tool["output"], "");
        let answer: Value = serde_json::from_str(&events[1].payload).unwrap();
        assert_eq!(answer, json!({"assistant":"latest answer"}));
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agent_housekeeping_sessions_are_ignored() {
        let home = config::home_dir();
        let inside = json!({"cwd": home.join(".codex").join("memories").to_string_lossy()});
        assert!(is_agent_internal(&inside));
        let outside = json!({"cwd": home.join("projects").join("x").to_string_lossy()});
        assert!(!is_agent_internal(&outside));
        let plugin =
            json!({"cwd": home.join(".claude").join("plugins").join("p").to_string_lossy()});
        assert!(!is_agent_internal(&plugin));
        assert!(!is_agent_internal(&json!({})));
    }

    #[test]
    fn secret_straddling_the_clip_boundary_is_masked() {
        let key = "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";
        let s = format!("{} {key} {}", "a".repeat(MAX_FIELD - 20), "b".repeat(3_000));
        let out = clip(&s);
        assert!(!out.contains("gsk_") && out.contains("[REDACTED]"));
        assert!(
            out.contains("…[clipped, 11038 chars in full]"),
            "{}",
            out.len()
        );
        // A key block too long for the overlap has no END in the window: cut at its BEGIN.
        let s = format!(
            "{} -----BEGIN RSA PRIVATE KEY-----\n{}",
            "a".repeat(MAX_FIELD - 40),
            "Q".repeat(9_000)
        );
        let out = clip(&s);
        assert!(!out.contains("BEGIN") && !out.contains("QQ") && out.contains("aaa"));
        assert_eq!(
            clip("x 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 y"),
            "x [REDACTED] y"
        );
    }

    #[test]
    fn typed_prompts_are_kept_without_private_blocks_and_harness_traffic() {
        let dir = tmp("prompts");
        let conn = db::open(&dir).unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let submit = |prompt: &str| {
            let p = json!({"session_id": "c1", "cwd": cwd, "prompt": prompt});
            assert!(
                handle(&conn, "claude", "UserPromptSubmit", &p)
                    .unwrap()
                    .is_none()
            );
        };
        submit("検索を直して <private>pw hunter2</private> お願い");
        submit(
            "<task-notification>\n<summary>Background command done</summary>\n</task-notification>\nRead the output file to retrieve the result: /tmp/x.output",
        );
        submit("<private reason=\"mine\">only this</private>");
        submit("<ide_opened_file>The user opened a.rs</ide_opened_file>\n再開して");
        submit("before <private>unclosed hunter3");
        submit("<privateer> is not the tag");
        // Nested: an inner close does not end the outer block.
        submit("x <private>a <private>b</private> hunter5</private> y");
        submit("keep <private>outer <private>inner</private> hunter6");
        // Another tool's context may quote the tag; it goes first, whole.
        submit(
            "<hook_context>- bugfix: text after an unclosed <private> tag\n</hook_context>\n\nテストを足して",
        );
        let bodies: Vec<String> = conn
            .prepare("SELECT body FROM prompts ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            bodies,
            [
                "検索を直して  お願い",
                "再開して",
                "before",
                "<privateer> is not the tag",
                "x  y",
                "keep",
                "テストを足して"
            ]
        );
        // The summarizer still reads the notification; nothing private reaches it either, and
        // a prompt that was all private leaves no event.
        let ev = db::session_events(&conn, "c1").unwrap();
        assert_eq!(ev.len(), 8);
        assert!(ev[1].payload.contains("task-notification"));
        assert!(ev.iter().all(|e| !e.payload.contains("hunter")
            && !e.payload.contains("only this")
            && !e.payload.contains("opened a.rs")));
        // Prompts are documents: searchable, readable in full.
        let hits = crate::search::search(&conn, "お願い", None, 10).unwrap();
        assert_eq!(
            (hits.len(), hits[0].doc.as_str(), hits[0].kind.as_str()),
            (1, "p1", "prompt")
        );
        // The agent moved into a nested repository: the prompt stays with its session's.
        std::fs::create_dir_all(dir.join("sub/.git")).unwrap();
        let moved = json!({"session_id": "c1", "cwd": dir.join("sub").to_string_lossy(), "prompt": "nested ask"});
        handle(&conn, "claude", "UserPromptSubmit", &moved).unwrap();
        let repo: String = conn
            .query_row(
                "SELECT repo FROM prompts WHERE body = 'nested ask'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repo, repo::key(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_compat_events_are_grok_or_dropped() {
        let grok = json!({"hookEventName": "Stop", "sessionId": "g1"});
        let missing = Path::new("/nonexistent/oboete.json");
        assert_eq!(resolve_agent("claude", &grok, missing), Some("grok"));
        let installed = tmp("grok").join("oboete.json");
        // The developer's own entries in our file do not mean Grok delivers our events.
        std::fs::write(
            &installed,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(resolve_agent("claude", &grok, &installed), Some("grok"));
        std::fs::write(&installed, r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]},{"hooks":[{"type":"command","command":"/x/oboete hook grok Stop"}]}]}}"#).unwrap();
        assert_eq!(resolve_agent("claude", &grok, &installed), None);
        assert_eq!(
            resolve_agent("claude", &json!({"session_id": "c1"}), &installed),
            Some("claude")
        );
        assert_eq!(
            resolve_agent("codex", &json!({}), &installed),
            Some("codex")
        );
        let cursor = json!({"conversation_id": "k1", "cursor_version": "2026.09.15", "workspace_roots": ["/r"]});
        assert_eq!(resolve_agent("claude", &cursor, missing), None);
    }

    #[test]
    fn codex_stop_reads_last_assistant_from_transcript_tail() {
        let dir = tmp("codex");
        let transcript = dir.join("rollout.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"first\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"q\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"done: 完了 \\\"quoted\\\"\"}]}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n",
            ),
        )
        .unwrap();
        assert_eq!(
            last_assistant_in_transcript(&transcript),
            "done: 完了 \"quoted\""
        );
        assert_eq!(last_assistant_in_transcript(Path::new("/nonexistent")), "");

        let conn = db::open(&dir).unwrap();
        let payload = json!({"session_id": "cx1", "cwd": dir.to_string_lossy(), "transcript_path": transcript.to_string_lossy()});
        handle(&conn, "codex", "Stop", &payload).unwrap();
        let ev = db::session_events(&conn, "cx1").unwrap();
        assert_eq!(ev.len(), 1);
        assert!(ev[0].payload.contains("done: 完了"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_camelcase_fields_and_resume_without_injection() {
        let dir = tmp("grokfields");
        let conn = db::open(&dir).unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let stop = json!({"sessionId": "g1", "workspaceRoot": cwd, "hookEventName": "Stop", "lastAssistantMessage": "grok said hi"});
        handle(&conn, "grok", "Stop", &stop).unwrap();
        let tool = json!({"sessionId": "g1", "workspaceRoot": cwd, "toolName": "Bash", "toolInput": {"cmd": "ls"}, "toolResult": "a b"});
        handle(&conn, "grok", "PostToolUse", &tool).unwrap();
        let ev = db::session_events(&conn, "g1").unwrap();
        assert!(ev[0].payload.contains("grok said hi"));
        assert!(ev[1].payload.contains("\"tool\":\"Bash\"") && ev[1].payload.contains("a b"));

        // Something to inject exists for this repo …
        db::apply_batch(
            &mut db::open(&dir).unwrap(),
            &db::PendingSession {
                id: "g1".into(),
                agent: "grok".into(),
                repo: repo::key(Path::new(&cwd)),
                last_event_at: db::now_ms(),
            },
            "test",
            "earlier summary",
            &[],
            i64::MAX,
        )
        .unwrap();
        let fresh = json!({"session_id": "g2", "cwd": cwd, "source": "startup"});
        assert!(
            handle(&conn, "claude", "SessionStart", &fresh)
                .unwrap()
                .is_some()
        );
        // … but a resumed session already carries it.
        let resumed = json!({"session_id": "g3", "cwd": cwd, "source": "resume"});
        assert!(
            handle(&conn, "claude", "SessionStart", &resumed)
                .unwrap()
                .is_none()
        );
        // Grok: nothing at SessionStart, once at the first tool call, never again.
        let g_start = json!({"sessionId": "g4", "workspaceRoot": cwd, "hookEventName": "SessionStart", "source": "startup"});
        assert!(
            handle(&conn, "grok", "SessionStart", &g_start)
                .unwrap()
                .is_none()
        );
        let g_tool = json!({"sessionId": "g4", "workspaceRoot": cwd, "hookEventName": "PreToolUse", "toolName": "Read"});
        let out = handle(&conn, "grok", "PreToolUse", &g_tool)
            .unwrap()
            .unwrap();
        assert!(
            out.contains("\"hookEventName\":\"PreToolUse\"") && out.contains("earlier summary")
        );
        assert!(
            handle(&conn, "grok", "PreToolUse", &g_tool)
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
