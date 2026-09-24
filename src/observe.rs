//! Summarize pending sessions. Single instance per home (file lock), bounded pass, then exit.

use std::path::Path;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{config, db, embed, hook, provider, redact};

/// Characters of transcript sent to the model per batch.
const MAX_PROMPT_CHARS: usize = 16_000;
const MAX_OBSERVATIONS: usize = 12;
const MAX_SUMMARY_CHARS: usize = 2_000;
pub const KINDS: [&str; 6] = [
    "decision",
    "bugfix",
    "feature",
    "discovery",
    "change",
    "preference",
];

#[derive(Default, Serialize)]
pub struct Stats {
    pub sessions_done: u32,
    pub sessions_failed: u32,
    pub observations: u32,
    pub fallbacks: u32,
    pub by_provider: std::collections::BTreeMap<String, u32>,
    pub vmhwm_kb: Option<u64>,
    /// Documents given a vector this run (`[embedding] provider = "workers-ai"`).
    pub embedded: usize,
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
    if let Err(e) = db::rekey_paths(&mut conn) {
        eprintln!("oboete observe: re-key repositories: {e:#}");
    }
    let mut stats = Stats::default();
    let mut chain = provider::Chain::new(&cfg.providers);
    let pending = db::pending_sessions(&conn, db::now_ms(), settle_ms)?;
    for s in pending {
        match process_session(&mut conn, &cfg, &mut chain, &s, &mut stats) {
            Ok(()) => stats.sessions_done += 1,
            Err(e) => {
                stats.sessions_failed += 1;
                eprintln!("oboete observe: session {}: {e:#}", s.id);
            }
        }
    }
    if cfg.embedding.provider == "workers-ai" {
        match embed::backlog(&mut conn, &cfg.embedding, Some(embed::PER_RUN)) {
            Ok(e) => stats.embedded = e.embedded,
            Err(e) => eprintln!("oboete observe: embed: {e:#}"),
        }
    }
    stats.vmhwm_kb = vmhwm_kb();
    Ok(stats)
}

fn process_session(
    conn: &mut rusqlite::Connection,
    cfg: &config::Config,
    chain: &mut provider::Chain,
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
        db::apply_batch(conn, s, "none", "", &[], last_id)?;
        return Ok(());
    }
    let prompt = build_prompt(&s.agent, &cfg.summary.language, &transcript);
    let result = chain.summarize(conn, &prompt, &schema())?;
    stats.fallbacks += result.fallbacks.len() as u32;
    let observations = parse_observations(&result.output)?;
    let summary = parse_summary(&result.output);
    stats.observations += observations.len() as u32;
    *stats
        .by_provider
        .entry(result.provider.clone())
        .or_default() += 1;
    db::apply_batch(conn, s, &result.provider, &summary, &observations, last_id)?;
    Ok(())
}

/// Plain-text transcript from stored events, oldest first; middle dropped when too long. Every
/// field passes `redact::outbound` here: this text goes to an external provider.
fn render(events: &[db::RawEvent]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for e in events {
        let v: Value = serde_json::from_str(&e.payload).unwrap_or(Value::Null);
        match e.event.as_str() {
            "UserPromptSubmit" => {
                let p = redact::outbound(v["prompt"].as_str().unwrap_or(""));
                if !p.is_empty() {
                    // A task report is worth summarizing, but it is not the developer speaking.
                    let who = if hook::is_envelope(&p) {
                        "NOTIFICATION"
                    } else {
                        "USER"
                    };
                    lines.push(format!("{who}: {p}"));
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let tool = redact::outbound(v["tool"].as_str().unwrap_or("?"));
                let input = short(&redact::outbound(v["input"].as_str().unwrap_or("")), 300);
                let output = short(&redact::outbound(v["output"].as_str().unwrap_or("")), 600);
                let mark = if v["failed"].as_bool().unwrap_or(false) {
                    " (failed)"
                } else {
                    ""
                };
                lines.push(format!("TOOL {tool}{mark}: {input}\n  -> {output}"));
            }
            "Stop" => {
                let a = redact::outbound(v["assistant"].as_str().unwrap_or(""));
                if !a.is_empty() {
                    lines.push(format!("ASSISTANT: {}", short(&a, 1_500)));
                }
            }
            "PostCompact" => {
                let s = redact::outbound(v["summary"].as_str().unwrap_or(""));
                if !s.is_empty() {
                    lines.push(format!("COMPACTED EARLIER PART: {}", short(&s, 1_500)));
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

fn build_prompt(agent: &str, language: &str, transcript: &str) -> String {
    format!(
        "You are the long-term memory of a software developer. Below is one coding session with the `{agent}` agent.\n\
         Extract only what is worth remembering in future sessions of this repository, then write a short summary.\n\
         Observations are facts, decisions, bug fixes, discoveries, changes or the developer's stated preferences: \
         concrete, with file paths, names and numbers. Skip routine tool noise, restated instructions and anything \
         the code itself already shows. If nothing is worth remembering, return an empty observations array. \
         At most {MAX_OBSERVATIONS} observations, each with a kind, a specific title (max 80 chars) and a body of 1-3 sentences.\n\
         The summary is 2-4 sentences: what was worked on, what was decided, what is still open.\n\
         Write every title, body and the summary in {language}.\n\n\
         --- SESSION ---\n{transcript}\n--- END ---"
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
                        "kind": {"type": "string", "enum": KINDS},
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

/// A provider can return any length; synced summaries must stay bounded.
fn parse_summary(v: &Value) -> String {
    v["summary"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect()
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
            // CLI providers do not enforce the schema enum (claude-mem stored 141 kinds with XML in them).
            kind: o["kind"]
                .as_str()
                .filter(|k| KINDS.contains(k))
                .unwrap_or("discovery")
                .to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_outside_the_schema_fall_back_to_discovery() {
        let out = json!({"observations": [
            {"kind": "decision", "title": "t1", "body": "b1"},
            {"kind": "discovery>\n    <title>leak</title>", "title": "t2", "body": "b2"},
            {"kind": "Decision", "title": "t3", "body": "b3"},
            {"title": "t4", "body": "b4"}
        ]});
        let kinds: Vec<String> = parse_observations(&out)
            .unwrap()
            .into_iter()
            .map(|o| o.kind)
            .collect();
        assert_eq!(kinds, ["decision", "discovery", "discovery", "discovery"]);
        // Every kind the schema offers survives the check.
        let offered =
            &schema()["properties"]["observations"]["items"]["properties"]["kind"]["enum"];
        assert_eq!(offered, &json!(KINDS));
    }

    #[test]
    fn summaries_are_cut_to_the_cap() {
        let long = "要".repeat(MAX_SUMMARY_CHARS + 500);
        let summary = parse_summary(&json!({"summary": long}));
        assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
        assert_eq!(parse_summary(&json!({})), "");
    }

    #[test]
    fn what_goes_to_the_summarizer_passes_the_gate() {
        // Built at run time so secret scanners do not flag the test source.
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let ev = |event: &str, payload: Value| db::RawEvent {
            id: 0,
            event: event.into(),
            payload: payload.to_string(),
        };
        let prompt = build_prompt(
            "claude",
            "English",
            &render(&[
                ev(
                    "UserPromptSubmit",
                    json!({"prompt": format!("deploy with {key} <private>PRIVATE-TEXT</private>")}),
                ),
                ev(
                    "PostToolUse",
                    json!({"tool": format!("mcp__{key}"), "input": "src/hook.rs", "output": format!("/// quote `<private>`, the opt-out\ntoken {key}")}),
                ),
                ev("Stop", json!({"assistant": "TAIL-MARKER done"})),
            ]),
        );
        assert!(!prompt.contains(&key), "{prompt}");
        assert!(!prompt.contains("PRIVATE-TEXT"), "{prompt}");
        // A tag an agent merely read does not swallow the rest of the session.
        assert!(prompt.contains("the opt-out"), "{prompt}");
        assert!(prompt.contains("TAIL-MARKER"), "{prompt}");
    }

    #[test]
    fn harness_notifications_are_not_the_developer_speaking() {
        let ev = |event: &str, payload: Value| db::RawEvent {
            id: 0,
            event: event.into(),
            payload: payload.to_string(),
        };
        let text = render(&[
            ev("UserPromptSubmit", json!({"prompt": "fix the search"})),
            ev(
                "UserPromptSubmit",
                json!({"prompt": "<task-notification>\n<summary>Review done: 2 findings</summary>\n</task-notification>"}),
            ),
        ]);
        assert!(
            text.starts_with("USER: fix the search\nNOTIFICATION: <task-notification>"),
            "{text}"
        );
    }
}
