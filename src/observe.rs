//! Summarize pending sessions. Single instance per home (file lock), bounded pass, then exit.

use std::path::Path;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{config, db, provider, redact};

/// Characters of transcript sent to the model per batch.
const MAX_PROMPT_CHARS: usize = 16_000;
const MAX_OBSERVATIONS: usize = 12;

#[derive(Default, Serialize)]
pub struct Stats {
    pub sessions_done: u32,
    pub sessions_failed: u32,
    pub observations: u32,
    pub fallbacks: u32,
    pub by_provider: std::collections::BTreeMap<String, u32>,
    pub vmhwm_kb: Option<u64>,
}

pub fn run(home: &Path, settle_ms: u64) -> Result<Stats> {
    let lock_path = home.join("observe.lock");
    let lock = std::fs::File::create(&lock_path)?;
    if let Err(e) = lock.try_lock() {
        // Another observe owns this home; it will pick up whatever we would have.
        return match e {
            std::fs::TryLockError::WouldBlock => Ok(Stats::default()),
            std::fs::TryLockError::Error(e) => Err(e.into()),
        };
    }
    let cfg = config::load(home)?;
    let mut conn = db::open(home)?;
    let mut stats = Stats::default();
    let pending = db::pending_sessions(&conn, db::now_ms(), settle_ms)?;
    for s in pending {
        match process_session(&mut conn, &cfg, &s, &mut stats) {
            Ok(()) => stats.sessions_done += 1,
            Err(e) => {
                stats.sessions_failed += 1;
                eprintln!("oboete observe: session {}: {e:#}", s.id);
            }
        }
    }
    stats.vmhwm_kb = vmhwm_kb();
    Ok(stats)
}

fn process_session(
    conn: &mut rusqlite::Connection,
    cfg: &config::Config,
    s: &db::PendingSession,
    stats: &mut Stats,
) -> Result<()> {
    let events = db::session_events(conn, &s.id)?;
    let Some(last) = events.last() else {
        return Ok(());
    };
    let last_id = last.id;
    let transcript = render(&events);
    if transcript.trim().is_empty() {
        // Nothing worth a model call (e.g. only SessionStart/SessionEnd): drop the raw rows.
        db::apply_batch(conn, &s.id, &s.repo, "none", "", &[], last_id)?;
        return Ok(());
    }
    let prompt = build_prompt(&s.agent, &transcript);
    let result = provider::summarize(conn, &cfg.providers, &prompt, &schema())?;
    stats.fallbacks += result.fallbacks.len() as u32;
    let observations = parse_observations(&result.output)?;
    let summary = result.output["summary"].as_str().unwrap_or("").to_string();
    stats.observations += observations.len() as u32;
    *stats
        .by_provider
        .entry(result.provider.clone())
        .or_default() += 1;
    db::apply_batch(
        conn,
        &s.id,
        &s.repo,
        &result.provider,
        &summary,
        &observations,
        last_id,
    )?;
    Ok(())
}

/// Plain-text transcript from stored events, oldest first; middle dropped when too long.
fn render(events: &[db::RawEvent]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for e in events {
        let v: Value = serde_json::from_str(&e.payload).unwrap_or(Value::Null);
        match e.event.as_str() {
            "UserPromptSubmit" => {
                if let Some(p) = v["prompt"].as_str().filter(|p| !p.trim().is_empty()) {
                    lines.push(format!("USER: {}", p.trim()));
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let tool = v["tool"].as_str().unwrap_or("?");
                let input = short(v["input"].as_str().unwrap_or(""), 300);
                let output = short(v["output"].as_str().unwrap_or(""), 600);
                let mark = if v["failed"].as_bool().unwrap_or(false) {
                    " (failed)"
                } else {
                    ""
                };
                lines.push(format!("TOOL {tool}{mark}: {input}\n  -> {output}"));
            }
            "Stop" => {
                if let Some(a) = v["assistant"].as_str().filter(|a| !a.trim().is_empty()) {
                    lines.push(format!("ASSISTANT: {}", short(a.trim(), 1_500)));
                }
            }
            _ => {}
        }
    }
    let text = lines.join("\n");
    if text.chars().count() <= MAX_PROMPT_CHARS {
        return text;
    }
    let half = MAX_PROMPT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(half)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\n…[middle of the session omitted]…\n{tail}")
}

fn short(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn build_prompt(agent: &str, transcript: &str) -> String {
    format!(
        "You are the memory of a software developer. Below is one coding session with the `{agent}` agent.\n\
         Extract what is worth remembering for future sessions in this repository, then write a short summary.\n\
         Rules: observations are facts, decisions, bug fixes, discoveries, changes or the developer's stated preferences.\n\
         Each observation: kind, a specific title (max 80 chars), a body (1-3 sentences, concrete: file paths, names, numbers).\n\
         Skip routine tool noise. Write in the language the developer used. At most {MAX_OBSERVATIONS} observations.\n\
         The summary is 2-4 sentences: what was worked on, what was decided, what is still open.\n\n\
         --- SESSION ---\n{}\n--- END ---",
        redact::redact(transcript)
    )
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "observations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": ["decision", "bugfix", "feature", "discovery", "change", "preference"]},
                        "title": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["kind", "title", "body"],
                    "additionalProperties": false
                }
            },
            "summary": {"type": "string"}
        },
        "required": ["observations", "summary"],
        "additionalProperties": false
    })
}

fn parse_observations(v: &Value) -> Result<Vec<db::Observation>> {
    let arr = v["observations"]
        .as_array()
        .ok_or_else(|| anyhow!("invalid output: observations is not an array"))?;
    let mut out = Vec::new();
    for o in arr.iter().take(MAX_OBSERVATIONS) {
        let title = o["title"].as_str().unwrap_or("").trim();
        let body = o["body"].as_str().unwrap_or("").trim();
        if title.is_empty() || body.is_empty() {
            continue;
        }
        out.push(db::Observation {
            kind: o["kind"].as_str().unwrap_or("discovery").to_string(),
            title: title.chars().take(120).collect(),
            body: body.chars().take(1_000).collect(),
        });
    }
    Ok(out)
}

/// Peak resident size of this process (Linux), for the spike's resource report.
fn vmhwm_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
}
