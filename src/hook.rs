//! Hook path: one agent event in on stdin, one row out. Must be fast and must never fail the agent.
//! Claude Code, Codex, Grok Build, and Pi share one JSON dialect; Grok also sends camelCase copies
//! and runs Claude Code's hooks as a compatibility layer, which is handled in `resolve_agent`.

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
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let payload: Value = if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw)?
        };
        let Some(agent) = resolve_agent(agent, &payload, &grok_hooks_file()) else {
            return Ok(None);
        };
        if (matches!(agent, "agy" | "cursor") && agent_workspace(agent, &payload).is_none())
            || is_agent_internal(agent, &payload)
        {
            return Ok(None);
        }
        std::fs::create_dir_all(home)?;
        let conn = db::open(home)?;
        let out = handle(&conn, agent, event, &payload)?;
        if matches!(event, "Stop" | "SessionEnd") && std::env::var_os("OBOETE_NO_SPAWN").is_none() {
            // Agents without a reliable SessionEnd need their last turn to settle first.
            spawn_observe(home, observe_wait_ms(agent));
        }
        Ok(out)
    })();
    if let Ok(Some(out)) = &result {
        writeln!(output, "{out}")?;
    } else if matches!(agent, "agy" | "cursor") {
        // Each adapter returns its own JSON shape, including skip and error paths.
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
/// Cursor imports them as well; drop those copies so only its dedicated adapter captures and
/// injects context (docs/research/agent-adapters-2026-09-23.md).
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

fn is_agent_internal(agent: &str, payload: &Value) -> bool {
    let Some(cwd) = agent_workspace(agent, payload)
        .or_else(|| str_field(payload, &["cwd", "workspaceRoot"]).map(str::to_owned))
    else {
        return false;
    };
    let home = config::home_dir();
    HOUSEKEEPING_DIRS
        .iter()
        .any(|d| Path::new(&cwd).starts_with(home.join(d)))
}

fn agent_workspace(agent: &str, payload: &Value) -> Option<String> {
    match agent {
        "agy" => agy_workspace(payload),
        "cursor" => cursor_workspace(payload, std::env::var_os("CURSOR_PROJECT_DIR").as_deref()),
        _ => None,
    }
}

/// Cursor's process cwd is the config directory, never a fallback for a missing workspace.
fn cursor_workspace(payload: &Value, project_dir: Option<&std::ffi::OsStr>) -> Option<String> {
    [
        payload["cwd"].as_str().map(Path::new),
        payload["workspace_roots"]
            .as_array()
            .and_then(|roots| roots.first())
            .and_then(Value::as_str)
            .map(Path::new),
        project_dir.map(Path::new),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_absolute())
    .map(|path| path.to_string_lossy().into_owned())
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

/// Store the event. Returns stdout JSON for context injection, or an empty object for agy/Cursor.
pub fn handle(
    conn: &Connection,
    agent: &str,
    event: &str,
    payload: &Value,
) -> Result<Option<String>> {
    let workspace = agent_workspace(agent, payload);
    if matches!(agent, "agy" | "cursor") && workspace.is_none() {
        return Ok(Some("{}".into()));
    }
    let session_id = str_field(
        payload,
        if agent == "cursor" {
            &["session_id", "conversation_id"]
        } else {
            &[
                "session_id",
                "sessionId",
                "conversation_id",
                "conversationId",
            ]
        },
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
            } else if agent == "cursor" && event == "PostToolUseFailure" {
                &payload["error_message"]
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
            let text = match str_field(
                payload,
                if agent == "cursor" {
                    &["text"]
                } else {
                    &["last_assistant_message", "lastAssistantMessage"]
                },
            ) {
                Some(t) => t.to_string(),
                None if agent == "cursor" => String::new(),
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
        "PreCompact" if agent == "cursor" => Some(json!({})),
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
    if agent == "cursor" && event == "PreCompact" {
        db::mark_compacted(&tx, session_id)?;
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
    // Cursor also restores context on the first prompt after its compaction marker.
    // The check and marker share the write transaction so simultaneous hooks cannot inject twice.
    let inject_now = match event {
        "SessionStart" => {
            agent == "cursor"
                || (!matches!(agent, "grok" | "agy")
                    && payload["source"].as_str() != Some("resume"))
        }
        "PreToolUse" => agent == "grok" && !db::injected(&tx, session_id)?,
        "PreInvocation" => agent == "agy" && !db::injected(&tx, session_id)?,
        "UserPromptSubmit" => agent == "cursor" && db::claim_reinjection(&tx, session_id)?,
        _ => false,
    };
    let mut out = matches!(agent, "agy" | "cursor").then(|| "{}".into());
    if inject_now {
        let text = inject::context(&tx, &repo_key)?;
        if !text.is_empty() {
            db::mark_injected(&tx, session_id, ts)?;
        }
        if !text.is_empty() || agent == "cursor" {
            out = Some(if agent == "agy" {
                json!({"injectSteps": [{"ephemeralMessage": text}]})
            } else if agent == "cursor" {
                cursor_injection(&text)
            } else {
                json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}})
            }.to_string());
        }
    }
    tx.commit()?;
    Ok(out)
}

fn cursor_injection(text: &str) -> Value {
    // Cursor measures JS string length and drops the whole field above 10,000 units.
    let mut units = 0;
    let text: String = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 9_500
        })
        .collect();
    json!({"additional_context": text})
}

/// The text without the blocks in `STRIP_BLOCKS` (`<tag>` or `<tag attr…>` up to its own
/// `</tag>`, nesting counted), trimmed. An unclosed `<private>` hides the rest only when
/// `unclosed_private_hides_rest` (a typed prompt); anywhere else the tag is just text an agent
/// read or wrote, and cutting there would drop the rest of the session.
pub fn strip_blocks(s: &str, unclosed_private_hides_rest: bool) -> String {
    without_blocks(s, unclosed_private_hides_rest)
        .trim()
        .to_string()
}

fn without_blocks(s: &str, unclosed_private_hides_rest: bool) -> String {
    let mut out = s.to_string();
    for tag in STRIP_BLOCKS {
        out = strip_tag(&out, tag, unclosed_private_hides_rest && *tag == "private");
    }
    out
}

/// One pass over `<tag` openers and `</tag>` closers: each closer pairs with the nearest open
/// opener, and every paired block goes (nested ones inside their outer block). An opener left
/// without a closer is kept as text, except with `hide_unclosed`, where the text stops at the
/// first one. Linear in the input, so a prompt full of stray openers cannot stall the hook.
fn strip_tag(s: &str, tag: &str, hide_unclosed: bool) -> String {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut marks: Vec<(usize, bool)> = s
        .match_indices(&open)
        .filter(|(i, _)| opens(&s[i + open.len()..]))
        .map(|(i, _)| (i, true))
        .chain(s.match_indices(&close).map(|(i, _)| (i, false)))
        .collect();
    marks.sort_unstable();
    let (mut stack, mut blocks) = (Vec::new(), Vec::new());
    for (i, is_open) in marks {
        if is_open {
            stack.push(i);
        } else if let Some(start) = stack.pop() {
            blocks.push((start, i + close.len()));
        }
    }
    // No paired block spans a leftover opener: the closer would have paired with it instead.
    let end = match stack.first() {
        Some(&first) if hide_unclosed => first,
        _ => s.len(),
    };
    blocks.sort_unstable();
    let (mut out, mut pos) = (String::with_capacity(s.len()), 0);
    for (start, stop) in blocks {
        if start >= end {
            break;
        }
        if start >= pos {
            out.push_str(&s[pos..start]);
            pos = stop;
        }
    }
    out.push_str(&s[pos.min(end)..end]);
    out
}

/// What follows `<tag` makes it the tag (`<privateer>` is not `<private`).
fn opens(after: &str) -> bool {
    after.starts_with(|c: char| c == '>' || c.is_whitespace())
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
pub fn clip(s: &str) -> String {
    // Closed `<private>`-style blocks go before the cut: a block cut in half would leave an
    // opener that the outbound gate keeps as text, with the private content right after it.
    let s = &without_blocks(s, false);
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
const NO_SESSION_END_OBSERVE_WAIT_MS: u64 = 65_000;

fn observe_wait_ms(agent: &str) -> Option<u64> {
    matches!(agent, "agy" | "opencode").then_some(NO_SESSION_END_OBSERVE_WAIT_MS)
}

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

    /// Payload shapes from the Cursor event table, with all paths kept inside the test repo.
    fn cursor_fixture(dir: &Path) -> Value {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let mut payloads: Value =
            serde_json::from_str(include_str!("testdata/cursor/payloads.json")).unwrap();
        for payload in payloads.as_object_mut().unwrap().values_mut() {
            payload["workspace_roots"] = json!([dir]);
            if payload.get("cwd").is_some() {
                payload["cwd"] = json!(dir);
            }
        }
        payloads
    }

    #[test]
    fn cursor_session_start_uses_workspace_and_prints_flat_context() {
        let dir = tmp("cursor-start");
        let payloads = cursor_fixture(&dir);
        let mut conn = db::open(&dir).unwrap();
        let ts = db::now_ms();
        let repo = repo::key(&dir);
        db::upsert_session(&conn, "prior", "cursor", &repo, dir.to_str().unwrap(), ts).unwrap();
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: "prior".into(),
                agent: "cursor".into(),
                repo: repo.clone(),
                last_event_at: ts,
            },
            "test",
            "earlier Cursor summary",
            &[],
            i64::MAX,
        )
        .unwrap();
        let mut output = Vec::new();
        run_io(
            &dir,
            "cursor",
            "SessionStart",
            payloads["SessionStart"].to_string().as_bytes(),
            &mut output,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap(),
            json!({
                "additional_context": "# oboete: what happened before in this repository\n\n## Recent sessions (newest first)\n- earlier Cursor summary\n"
            })
        );
        let stored: (String, String, String) = conn
            .query_row(
                "SELECT agent, repo, cwd FROM sessions WHERE id='cursor-session'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            stored,
            ("cursor".into(), repo, dir.to_str().unwrap().into())
        );
        assert_eq!(
            db::session_events(&conn, "cursor-session").unwrap()[0].event,
            "SessionStart"
        );
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_events_capture_prompt_tool_text_and_session_end() {
        let dir = tmp("cursor-events");
        let mut payloads = cursor_fixture(&dir);
        let conn = db::open(&dir).unwrap();
        let events = [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUseFailure",
            "Stop",
            "SessionEnd",
        ];
        for event in events {
            let payload = &mut payloads[event];
            // session_id wins; a conversation-only event still belongs to the same session.
            if event == "SessionEnd" {
                payload.as_object_mut().unwrap().remove("session_id");
                payload["sessionId"] = json!("not-a-cursor-field");
            } else {
                payload["conversation_id"] = json!("not-the-session");
            }
            let out = handle(&conn, "cursor", event, payload).unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&out).unwrap(),
                if event == "SessionStart" {
                    json!({"additional_context":""})
                } else {
                    json!({})
                }
            );
        }
        let stored = db::session_events(&conn, "cursor-session").unwrap();
        assert_eq!(
            stored.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            events
        );
        let bodies: Vec<Value> = stored
            .iter()
            .map(|e| serde_json::from_str(&e.payload).unwrap())
            .collect();
        assert_eq!(
            bodies[1],
            json!({"prompt":"Read hello.txt and explain the result."})
        );
        assert_eq!(
            bodies[2],
            json!({"tool":"Shell", "input":"{\"command\":\"cat hello.txt\"}", "output":"{\"exitCode\":0,\"stdout\":\"hello\\n\"}", "failed":false})
        );
        assert_eq!(
            bodies[3],
            json!({"tool":"Read", "input":"{\"path\":\"missing.txt\"}", "output":"File not found: missing.txt", "failed":true})
        );
        assert_eq!(
            bodies[4],
            json!({"assistant":"hello.txt contains hello; missing.txt does not exist."})
        );
        assert_eq!(bodies[5], json!({"reason":"user_close"}));
        let prompts = crate::search::search(&conn, "explain", Some(&repo::key(&dir)), 10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].body, "Read hello.txt and explain the result.");
        let ended: bool = conn
            .query_row(
                "SELECT ended_at IS NOT NULL FROM sessions WHERE id='cursor-session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ended);
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_compaction_reinjects_once_after_cleanup_even_with_concurrent_prompts() {
        let dir = tmp("cursor-compact");
        let payloads = cursor_fixture(&dir);
        let mut conn = db::open(&dir).unwrap();
        handle(&conn, "cursor", "SessionStart", &payloads["SessionStart"]).unwrap();
        assert_eq!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .unwrap(),
            Some("{}".into())
        );
        assert_eq!(
            handle(&conn, "cursor", "PreCompact", &payloads["PreCompact"]).unwrap(),
            Some("{}".into())
        );
        let events = db::session_events(&conn, "cursor-session").unwrap();
        let marker = events
            .iter()
            .find(|e| e.event == "PreCompact")
            .expect("compaction marker");
        assert_eq!(marker.payload, "{}");
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: "cursor-session".into(),
                agent: "cursor".into(),
                repo: repo::key(&dir),
                last_event_at: db::now_ms(),
            },
            "test",
            "context after compaction",
            &[],
            i64::MAX,
        )
        .unwrap();
        assert!(
            db::session_events(&conn, "cursor-session")
                .unwrap()
                .is_empty()
        );
        drop(conn);
        let conn = db::open(&dir).unwrap();
        conn.execute_batch("CREATE TRIGGER refuse_cursor_prompt BEFORE INSERT ON events WHEN NEW.event='UserPromptSubmit'
            BEGIN SELECT RAISE(ABORT, 'test insert failure'); END;").unwrap();
        assert!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .is_err()
        );
        conn.execute_batch("DROP TRIGGER refuse_cursor_prompt")
            .unwrap();
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.clone();
                let payload = payloads["UserPromptSubmit"].clone();
                std::thread::spawn(move || {
                    handle(
                        &db::open(&dir).unwrap(),
                        "cursor",
                        "UserPromptSubmit",
                        &payload,
                    )
                    .unwrap()
                    .unwrap()
                })
            })
            .collect();
        let responses: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(responses.iter().filter(|s| s.as_str() != "{}").count(), 1);
        let injected: Value =
            serde_json::from_str(responses.iter().find(|s| s.as_str() != "{}").unwrap()).unwrap();
        assert!(
            injected["additional_context"]
                .as_str()
                .unwrap()
                .contains("context after compaction")
        );
        assert_eq!(injected.as_object().unwrap().len(), 1);
        assert_eq!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .unwrap(),
            Some("{}".into())
        );
        // A second compaction permits exactly one more injection, scoped to its session.
        handle(&conn, "cursor", "PreCompact", &payloads["PreCompact"]).unwrap();
        let mut other = payloads["UserPromptSubmit"].clone();
        other["session_id"] = json!("other-session");
        assert_eq!(
            handle(&conn, "cursor", "UserPromptSubmit", &other).unwrap(),
            Some("{}".into())
        );
        assert_ne!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .unwrap(),
            Some("{}".into())
        );
        assert_eq!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .unwrap(),
            Some("{}".into())
        );
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_injection_limit_counts_utf16_without_splitting_non_bmp_characters() {
        for (input, expected) in [
            ("界".repeat(9_501), "界".repeat(9_500)),
            ("🚀".repeat(4_751), "🚀".repeat(4_750)),
            (
                format!("x{}", "🚀".repeat(4_750)),
                format!("x{}", "🚀".repeat(4_749)),
            ),
            ("context\n\"quoted\"".into(), "context\n\"quoted\"".into()),
        ] {
            let out = cursor_injection(&input);
            assert_eq!(out["additional_context"], expected);
            assert!(
                out["additional_context"]
                    .as_str()
                    .unwrap()
                    .encode_utf16()
                    .count()
                    <= 9_500
            );
            assert_eq!(out.as_object().unwrap().len(), 1);
        }
    }

    #[test]
    fn leading_bom_is_accepted_for_every_agent() {
        let dir = tmp("bom");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for agent in crate::setup::AGENTS {
            let payload = json!({"session_id":agent, "conversationId":agent, "cwd":dir, "workspacePaths":[dir], "workspace_roots":[dir]});
            let raw = format!("\u{feff}{payload}\r\n");
            let mut output = Vec::new();
            run_io(&dir, agent, "SessionStart", raw.as_bytes(), &mut output).unwrap();
            let conn = db::open(&dir).unwrap();
            let events = db::session_events(&conn, agent).unwrap();
            assert_eq!(events.len(), 1, "{agent}");
            assert_eq!(events[0].event, "SessionStart");
            if agent == "cursor" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output).unwrap(),
                    json!({"additional_context":""})
                );
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_workspace_precedence_and_missing_paths_never_use_process_cwd() {
        let dir = tmp("cursor-paths");
        let cwd = dir.join("tool-repo");
        let root = dir.join("workspace");
        let env = dir.join("environment");
        let expected = |p: &Path| Some(p.to_string_lossy().into_owned());
        assert_eq!(
            cursor_workspace(
                &json!({"cwd":cwd, "workspace_roots":[root]}),
                Some(env.as_os_str())
            ),
            expected(&cwd)
        );
        assert_eq!(
            cursor_workspace(&json!({"workspace_roots":[root]}), Some(env.as_os_str())),
            expected(&root)
        );
        assert_eq!(
            cursor_workspace(&json!({"workspace_roots":[]}), Some(env.as_os_str())),
            expected(&env)
        );
        assert_eq!(cursor_workspace(&json!({}), None), None);
        for invalid in ["", ".", "relative/repo"] {
            assert_eq!(cursor_workspace(&json!({"cwd":invalid}), None), None);
            assert_eq!(
                cursor_workspace(
                    &json!({"cwd":invalid, "workspace_roots":[root]}),
                    Some(env.as_os_str())
                ),
                expected(&root)
            );
            assert_eq!(
                cursor_workspace(
                    &json!({"cwd":invalid, "workspace_roots":[invalid]}),
                    Some(env.as_os_str())
                ),
                expected(&env)
            );
            assert_eq!(
                cursor_workspace(&json!({"workspace_roots":[invalid]}), None),
                None
            );
            assert_eq!(
                cursor_workspace(&json!({}), Some(std::ffi::OsStr::new(invalid))),
                None
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_missing_workspace_and_environment_fallback_use_isolated_processes() {
        // Child test processes isolate environment changes from the parallel Rust test runner.
        const CASE_DIR: &str = "OBOETE_CURSOR_WORKSPACE_TEST_DIR";
        if let Some(dir) = std::env::var_os(CASE_DIR) {
            let dir = PathBuf::from(dir);
            let home = dir.join("store");
            let mut output = Vec::new();
            run_io(
                &home,
                "cursor",
                "SessionStart",
                &b"{\"conversation_id\":\"env-session\"}"[..],
                &mut output,
            )
            .unwrap();
            if let Some(workspace) = std::env::var_os("CURSOR_PROJECT_DIR") {
                let conn = db::open(&home).unwrap();
                let (cwd, repo): (String, String) = conn
                    .query_row(
                        "SELECT cwd, repo FROM sessions WHERE id='env-session'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(cwd, workspace.to_string_lossy());
                assert_eq!(repo, repo::key(Path::new(&workspace)));
                assert_eq!(
                    serde_json::from_slice::<Value>(&output).unwrap(),
                    json!({"additional_context":""})
                );
            } else {
                assert_eq!(output, b"{}\n");
                assert!(
                    !home.exists(),
                    "missing workspace must not even create storage"
                );
            }
            return;
        }
        let dir = tmp("cursor-env");
        for fallback in [false, true] {
            let case = dir.join(if fallback { "fallback" } else { "missing" });
            std::fs::create_dir_all(case.join("repo/.git")).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "hook::tests::cursor_missing_workspace_and_environment_fallback_use_isolated_processes"])
                .env(CASE_DIR, &case).env_remove(SKIP_ENV).env_remove("CURSOR_PROJECT_DIR");
            if fallback {
                command.env("CURSOR_PROJECT_DIR", case.join("repo"));
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_stdout_is_empty_object_when_parsing_or_storage_fails() {
        let dir = tmp("cursor-fail-open");
        let payloads = cursor_fixture(&dir);
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, "x").unwrap();
        for input in ["invalid JSON".into(), payloads["SessionStart"].to_string()] {
            let mut output = Vec::new();
            assert!(
                run_io(
                    &blocked,
                    "cursor",
                    "SessionStart",
                    input.as_bytes(),
                    &mut output
                )
                .is_err()
            );
            assert_eq!(output, b"{}\n");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_empty_reinjection_is_consumed_and_prompts_keep_privacy_rules() {
        let dir = tmp("cursor-empty");
        let mut payloads = cursor_fixture(&dir);
        let conn = db::open(&dir).unwrap();
        handle(&conn, "cursor", "PreCompact", &payloads["PreCompact"]).unwrap();
        payloads["UserPromptSubmit"]["prompt"] = json!("<private>private only</private>");
        let out = handle(
            &conn,
            "cursor",
            "UserPromptSubmit",
            &payloads["UserPromptSubmit"],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"additional_context":""})
        );
        payloads["UserPromptSubmit"]["prompt"] = json!(
            "<hook_context>not asked</hook_context>save this <private>private text</private>"
        );
        assert_eq!(
            handle(
                &conn,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            )
            .unwrap(),
            Some("{}".into())
        );
        let events = db::session_events(&conn, "cursor-session").unwrap();
        assert_eq!(events.len(), 2); // Marker and public prompt only.
        assert_eq!(events[1].payload, "{\"prompt\":\"save this\"}");
        let hits = crate::search::search(&conn, "save this", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].body, "save this");
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
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
    fn pi_captures_prompt_tools_answer_and_session_end() {
        let dir = tmp("pi-capture");
        std::fs::create_dir(dir.join(".git")).unwrap();
        let conn = db::open(&dir).unwrap();
        let mut payload = json!({
            "session_id": "pi1",
            "cwd": dir,
            "transcript_path": dir.join("session.jsonl"),
            "source": "startup",
        });
        assert!(
            handle(&conn, "pi", "SessionStart", &payload)
                .unwrap()
                .is_none()
        );
        payload["prompt"] = json!("Fix <private>client token</private> this");
        handle(&conn, "pi", "UserPromptSubmit", &payload).unwrap();
        payload["tool_name"] = json!("read");
        payload["tool_input"] = json!({"path":"notes.txt"});
        payload["tool_response"] = json!("ok <private>secret output</private>");
        handle(&conn, "pi", "PostToolUse", &payload).unwrap();
        payload["tool_name"] = json!("bash");
        payload["tool_input"] = json!({"command":"false"});
        payload["tool_response"] = json!("command failed");
        handle(&conn, "pi", "PostToolUseFailure", &payload).unwrap();
        payload["last_assistant_message"] = json!("Fixed it");
        handle(&conn, "pi", "Stop", &payload).unwrap();
        payload["reason"] = json!("quit");
        handle(&conn, "pi", "SessionEnd", &payload).unwrap();

        let events = db::session_events(&conn, "pi1").unwrap();
        assert_eq!(
            events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            [
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "PostToolUseFailure",
                "Stop",
                "SessionEnd",
            ]
        );
        let body: String = conn
            .query_row("SELECT body FROM prompts WHERE session_id='pi1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body, "Fix  this");
        let stored: Vec<Value> = events
            .iter()
            .map(|e| serde_json::from_str(&e.payload).unwrap())
            .collect();
        assert_eq!(stored[1], json!({"prompt":"Fix  this"}));
        assert_eq!(stored[2]["tool"], "read");
        assert_eq!(stored[2]["failed"], false);
        assert!(
            !stored[2]["output"]
                .as_str()
                .unwrap()
                .contains("secret output")
        );
        assert_eq!(stored[3]["tool"], "bash");
        assert_eq!(stored[3]["failed"], true);
        assert_eq!(stored[3]["output"], "command failed");
        assert_eq!(stored[4], json!({"assistant":"Fixed it"}));
        assert_eq!(stored[5], json!({"reason":"quit"}));
        let (agent, repo, ended): (String, String, Option<i64>) = conn
            .query_row(
                "SELECT agent, repo, ended_at FROM sessions WHERE id='pi1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, "pi");
        assert_eq!(repo, repo::key(&dir));
        assert!(ended.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pi_injects_on_start_and_compact_but_not_resume() {
        let dir = tmp("pi-injection");
        std::fs::create_dir(dir.join(".git")).unwrap();
        let mut conn = db::open(&dir).unwrap();
        let repo_key = repo::key(&dir);
        let now = db::now_ms();
        db::upsert_session(&conn, "old", "pi", &repo_key, dir.to_str().unwrap(), now).unwrap();
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: "old".into(),
                agent: "pi".into(),
                repo: repo_key,
                last_event_at: now,
            },
            "test",
            "earlier Pi summary",
            &[],
            i64::MAX,
        )
        .unwrap();
        let start = json!({"session_id":"new", "cwd":dir, "source":"startup"});
        let output: Value = serde_json::from_str(
            &handle(&conn, "pi", "SessionStart", &start)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        assert!(
            output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("earlier Pi summary")
        );
        assert!(db::injected(&conn, "new").unwrap());

        let resumed = json!({"session_id":"resumed", "cwd":dir, "source":"resume"});
        assert!(
            handle(&conn, "pi", "SessionStart", &resumed)
                .unwrap()
                .is_none()
        );
        assert!(!db::injected(&conn, "resumed").unwrap());

        let compact = json!({"session_id":"new", "cwd":dir, "compact_summary":"recent work", "trigger":"manual"});
        handle(&conn, "pi", "PostCompact", &compact).unwrap();
        let reinject = json!({"session_id":"new", "cwd":dir, "source":"compact"});
        assert!(
            handle(&conn, "pi", "SessionStart", &reinject)
                .unwrap()
                .unwrap()
                .contains("earlier Pi summary")
        );
        let events = db::session_events(&conn, "new").unwrap();
        assert_eq!(events[1].event, "PostCompact");
        assert_eq!(
            serde_json::from_str::<Value>(&events[1].payload).unwrap(),
            json!({"summary":"recent work"})
        );
        std::fs::remove_dir_all(dir).unwrap();
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
        assert!(is_agent_internal("codex", &inside));
        let outside = json!({"cwd": home.join("projects").join("x").to_string_lossy()});
        assert!(!is_agent_internal("codex", &outside));
        let plugin =
            json!({"cwd": home.join(".claude").join("plugins").join("p").to_string_lossy()});
        assert!(!is_agent_internal("claude", &plugin));
        assert!(!is_agent_internal("claude", &json!({})));
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
    fn a_private_block_cut_by_the_clip_is_not_stored() {
        let dir = tmp("clip-private");
        let conn = db::open(&dir).unwrap();
        let output = format!(
            "{} <private>{}</private> tail",
            "h".repeat(7_900),
            "S".repeat(9_000)
        );
        let payload = json!({"session_id": "c1", "cwd": dir, "tool_name": "Read",
                             "tool_input": {}, "tool_response": output});
        handle(&conn, "claude", "PostToolUse", &payload).unwrap();
        let stored = &db::session_events(&conn, "c1").unwrap()[0].payload;
        assert!(!stored.contains("SSS"), "private content stored");
        assert!(!stored.contains("<private>"), "{stored}");
        assert!(stored.contains("tail"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn stray_openers_do_not_shield_later_blocks() {
        let text = "mentions <private>, then <private>CUSTOMER DATA</private> end";
        assert_eq!(strip_blocks(text, false), "mentions <private>, then  end");
        assert_eq!(strip_blocks(text, true), "mentions");
        assert_eq!(
            strip_blocks("a <private>x <private>y</private> z</private> b", false),
            "a  b"
        );
        assert_eq!(
            strip_blocks("a </private> b <private>c</private>", false),
            "a </private> b"
        );
        // Linear: a prompt of stray openers is cheap.
        let many = "<hook_context ".repeat(50_000) + "</hook_context>";
        let start = std::time::Instant::now();
        strip_blocks(&many, true);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
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
        let opencode = json!({"session_id": "oc1", "cwd": "/tmp/project"});
        assert_eq!(
            resolve_agent("opencode", &opencode, &installed),
            Some("opencode")
        );
    }

    #[test]
    fn opencode_claude_fields_store_session_prompt_tools_and_stop() {
        let dir = tmp("opencode-events");
        let conn = db::open(&dir).unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let base = json!({"session_id": "oc1", "cwd": cwd});
        assert!(
            handle(&conn, "opencode", "SessionStart", &base)
                .unwrap()
                .is_none()
        );
        let prompt = json!({"session_id": "oc1", "cwd": cwd, "prompt": "keep <private>secret text</private> this"});
        handle(&conn, "opencode", "UserPromptSubmit", &prompt).unwrap();
        let success = json!({"session_id": "oc1", "cwd": cwd, "tool_name": "read", "tool_input": {"filePath": "a.rs"}, "tool_response": "file content"});
        handle(&conn, "opencode", "PostToolUse", &success).unwrap();
        let failure = json!({"session_id": "oc1", "cwd": cwd, "tool_name": "bash", "tool_input": {"command": "false"}, "tool_response": "exit 1"});
        handle(&conn, "opencode", "PostToolUseFailure", &failure).unwrap();
        let stop = json!({"session_id": "oc1", "cwd": cwd, "last_assistant_message": "Done"});
        handle(&conn, "opencode", "Stop", &stop).unwrap();

        let (agent, stored_cwd): (String, String) = conn
            .query_row("SELECT agent, cwd FROM sessions WHERE id='oc1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            (agent.as_str(), stored_cwd.as_str()),
            ("opencode", cwd.as_str())
        );
        let body: String = conn
            .query_row("SELECT body FROM prompts WHERE session_id='oc1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body, "keep  this");
        let events = db::session_events(&conn, "oc1").unwrap();
        let names: Vec<_> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(
            names,
            [
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "PostToolUseFailure",
                "Stop"
            ]
        );
        let payloads: Vec<Value> = events
            .iter()
            .map(|e| serde_json::from_str(&e.payload).unwrap())
            .collect();
        assert_eq!(payloads[1], json!({"prompt": "keep  this"}));
        assert_eq!(
            payloads[2],
            json!({"tool": "read", "input": "{\"filePath\":\"a.rs\"}", "output": "file content", "failed": false})
        );
        assert_eq!(
            payloads[3],
            json!({"tool": "bash", "input": "{\"command\":\"false\"}", "output": "exit 1", "failed": true})
        );
        assert_eq!(payloads[4], json!({"assistant": "Done"}));
        assert!(events.iter().all(|e| !e.payload.contains("secret text")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agents_without_session_end_wait_for_stop_to_settle() {
        assert_eq!(observe_wait_ms("agy"), Some(65_000));
        assert_eq!(observe_wait_ms("opencode"), Some(65_000));
        for agent in ["claude", "codex", "grok"] {
            assert_eq!(observe_wait_ms(agent), None, "{agent}");
        }
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
