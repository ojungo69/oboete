//! Hook path: one agent event in on stdin, one row out. Must be fast and must never fail the agent.

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::{db, inject, redact, repo};

/// Largest text kept per field. Tool outputs beyond this are clipped with a marker.
const MAX_FIELD: usize = 8_000;

pub fn run_stdin(home: &Path, agent: &str, event: &str) -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let payload: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&raw)?
    };
    let conn = db::open(home)?;
    if let Some(out) = handle(&conn, agent, event, &payload)? {
        println!("{out}");
    }
    if matches!(event, "Stop" | "SessionEnd") && std::env::var_os("OBOETE_NO_SPAWN").is_none() {
        spawn_observe(home);
    }
    Ok(())
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
    db::upsert_session(conn, session_id, agent, &repo_key, cwd, ts)?;

    let stored = match event {
        "SessionStart" => Some(json!({"source": payload.get("source")})),
        "UserPromptSubmit" => {
            Some(json!({"prompt": clip(str_field(payload, &["prompt"]).unwrap_or(""))}))
        }
        "PostToolUse" | "PostToolUseFailure" => Some(json!({
            "tool": str_field(payload, &["tool_name", "toolName"]).unwrap_or("?"),
            "input": clip(&compact(&payload["tool_input"])),
            "output": clip(&compact(payload.get("tool_response").or(payload.get("tool_output")).unwrap_or(&Value::Null))),
            "failed": event == "PostToolUseFailure",
        })),
        "Stop" => Some(
            json!({"assistant": clip(str_field(payload, &["last_assistant_message"]).unwrap_or(""))}),
        ),
        "SessionEnd" => {
            db::end_session(conn, session_id, ts)?;
            Some(json!({"reason": payload.get("reason")}))
        }
        _ => None, // PreToolUse and the rest carry nothing a summary needs
    };
    if let Some(v) = stored {
        db::insert_event(conn, session_id, event, ts, &v.to_string())?;
    }

    if event == "SessionStart" {
        let text = inject::context(conn, &repo_key)?;
        if !text.is_empty() {
            let out = json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": text}});
            return Ok(Some(out.to_string()));
        }
    }
    Ok(None)
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

/// Redact secrets, then clip to MAX_FIELD characters (on a char boundary).
fn clip(s: &str) -> String {
    let s = redact::redact(s);
    if s.chars().count() <= MAX_FIELD {
        return s;
    }
    let head: String = s.chars().take(MAX_FIELD).collect();
    format!("{head}\n…[clipped {} chars]", s.chars().count() - MAX_FIELD)
}

/// Detached `oboete observe`; the lock inside observe makes duplicates harmless.
fn spawn_observe(home: &Path) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let _ = std::process::Command::new(exe)
        .arg("--home")
        .arg(home)
        .arg("observe")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
