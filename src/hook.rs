//! Hook path: one agent event in on stdin, one row out. Must be fast and must never fail the agent.
//! Claude Code, Codex and Grok Build share one JSON dialect; Grok also sends camelCase copies and
//! runs Claude Code's hooks as a compatibility layer, which is handled in `resolve_agent`.

use std::io::Read;
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
    if std::env::var_os(SKIP_ENV).is_some() {
        return Ok(());
    }
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let payload: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&raw)?
    };
    let Some(agent) = resolve_agent(agent, &payload, &grok_hooks_file()) else {
        return Ok(());
    };
    if is_agent_internal(&payload) {
        return Ok(());
    }
    let conn = db::open(home)?;
    if let Some(out) = handle(&conn, agent, event, &payload)? {
        println!("{out}");
    }
    if matches!(event, "Stop" | "SessionEnd") && std::env::var_os("OBOETE_NO_SPAWN").is_none() {
        spawn_observe(home);
    }
    Ok(())
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
    let Some(cwd) = str_field(payload, &["cwd", "workspaceRoot"]) else {
        return false;
    };
    let home = config::home_dir();
    HOUSEKEEPING_DIRS
        .iter()
        .any(|d| Path::new(cwd).starts_with(home.join(d)))
}

/// Store the event. Returns the hook's stdout JSON (context injection) when there is one.
pub fn handle(
    conn: &Connection,
    agent: &str,
    event: &str,
    payload: &Value,
) -> Result<Option<String>> {
    let session_id =
        str_field(payload, &["session_id", "sessionId", "conversation_id"]).unwrap_or("unknown");
    let cwd = str_field(payload, &["cwd", "workspaceRoot"]).unwrap_or(".");
    let repo_key = repo::key(Path::new(cwd));
    let ts = db::now_ms();

    let prompt = (event == "UserPromptSubmit")
        .then(|| clip(&strip_blocks(str_field(payload, &["prompt"]).unwrap_or(""))))
        .filter(|p| !p.is_empty());
    let stored = match event {
        "SessionStart" => Some(json!({"source": payload.get("source")})),
        "UserPromptSubmit" => prompt.as_ref().map(|p| json!({"prompt": p})),
        "PostToolUse" | "PostToolUseFailure" => Some(json!({
            "tool": str_field(payload, &["tool_name", "toolName"]).unwrap_or("?"),
            "input": clip(&compact(field(payload, &["tool_input", "toolInput"]))),
            "output": clip(&compact(field(payload, &["tool_response", "toolResult", "tool_output", "error"]))),
            "failed": event == "PostToolUseFailure",
        })),
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
    if event == "SessionEnd" {
        db::end_session(&tx, session_id, ts)?;
    }
    if let Some(v) = stored {
        db::insert_event(&tx, session_id, event, ts, &v.to_string())?;
    }
    // The event feeds the summary and goes with it; the prompt itself is kept as a document.
    if let Some(p) = prompt.as_deref().filter(|p| !is_envelope(p)) {
        db::insert_prompt(&tx, session_id, ts, p)?;
    }
    tx.commit()?;

    // Claude Code and Codex read context at SessionStart (not on resume: the transcript already
    // has it; after a compaction it is gone, so `compact` gets it again). Grok ignores
    // SessionStart stdout, so its context rides on the first tool call.
    let inject_now = match event {
        "SessionStart" => agent != "grok" && payload["source"].as_str() != Some("resume"),
        "PreToolUse" => agent == "grok" && !db::injected(conn, session_id)?,
        _ => false,
    };
    if inject_now {
        let text = inject::context(conn, &repo_key)?;
        if !text.is_empty() {
            db::mark_injected(conn, session_id, ts)?;
            let out =
                json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}});
            return Ok(Some(out.to_string()));
        }
    }
    Ok(None)
}

/// The prompt without the blocks in `STRIP_BLOCKS` (`<tag>` or `<tag attr…>` up to its own
/// `</tag>`, nesting counted), trimmed.
fn strip_blocks(s: &str) -> String {
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
                None if *tag == "private" => out.truncate(at),
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

/// Last assistant `output_text` in a Codex rollout JSONL, reading only the file's tail.
fn last_assistant_in_transcript(path: &Path) -> String {
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
    if f.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
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
fn spawn_observe(home: &Path) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--home")
        .arg(home)
        .arg("observe")
        .stdin(std::process::Stdio::null())
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
        let d = std::env::temp_dir().join(format!("oboete-hook-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
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
