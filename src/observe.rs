//! Summarize pending sessions. Single instance per home (file lock), bounded pass, then exit.

use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{config, db, embed, hook, provider, redact};

/// Characters of transcript sent to the model per call.
const MAX_PROMPT_CHARS: usize = 16_000;
/// Share of one call that dialogue may take; tool lines get the rest.
const DIALOGUE_CHARS: usize = 12_000;
/// Events read per query while a part is built.
const PAGE: usize = 500;
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
    // One part per session per round, until every session is done: a long session neither
    // stops early (nothing would start the next run) nor holds the others back.
    let mut pending = db::pending_sessions(&conn, db::now_ms(), settle_ms)?;
    while !pending.is_empty() {
        pending.retain(
            |s| match process_part(&mut conn, &cfg, &mut chain, s, &mut stats) {
                Ok(true) => true,
                Ok(false) => {
                    stats.sessions_done += 1;
                    false
                }
                Err(e) => {
                    stats.sessions_failed += 1;
                    eprintln!("oboete observe: session {}: {e:#}", s.id);
                    false
                }
            },
        );
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

/// Summarize the session's next part. `true`: a part was stored and more may be pending.
fn process_part(
    conn: &mut rusqlite::Connection,
    cfg: &config::Config,
    chain: &mut provider::Chain,
    s: &db::PendingSession,
    stats: &mut Stats,
) -> Result<bool> {
    let part = next_part(conn, &s.id)?;
    let Some((last_id, last_ts)) = part.last else {
        return Ok(false);
    };
    // Each part's rows carry the time of its own last event.
    let s = db::PendingSession {
        id: s.id.clone(),
        agent: s.agent.clone(),
        repo: s.repo.clone(),
        last_event_at: last_ts,
    };
    let stored = if part.text.trim().is_empty() {
        // Nothing worth a model call (e.g. only SessionStart/SessionEnd): mark the rows read.
        db::apply_batch(conn, &s, "none", "", &[], last_id)?
    } else {
        let earlier = db::latest_summary(conn, &s.id)?;
        let prompt = build_prompt(
            &s.agent,
            &cfg.summary.language,
            earlier.as_deref(),
            &part.text,
        );
        let result = chain.summarize(conn, &prompt, &schema())?;
        stats.fallbacks += result.fallbacks.len() as u32;
        let observations = parse_observations(&result.output)?;
        let summary = parse_summary(&result.output);
        stats.observations += observations.len() as u32;
        *stats
            .by_provider
            .entry(result.provider.clone())
            .or_default() += 1;
        db::apply_batch(conn, &s, &result.provider, &summary, &observations, last_id)?
    };
    // `false` also when the session was deleted meanwhile: nothing more to do for it.
    Ok(stored)
}

/// One summarizer call's worth of a session, from its observe cursor on.
struct Part {
    text: String,
    /// Id and time of the last event the part covers; `None` when nothing is pending.
    last: Option<(i64, i64)>,
}

/// Every dialogue line (the developer, answers to the agent's questions, harness reports, the
/// agent's replies) in order until they fill `DIALOGUE_CHARS`, plus the newest tool lines that fit
/// in what is left. Reads a page at a time and keeps at most one call's worth of tool lines, so a
/// long session is never held in memory whole; everything it covers is either sent or counted in
/// the omitted-tool-calls line.
// ponytail: keeps the newest tool lines of a part; the redesign's curation windows
// (docs/research/redesign-2026-09-24/sections-1-4.md, section 3) replace this.
fn next_part(conn: &rusqlite::Connection, session_id: &str) -> Result<Part> {
    let mut dialogue: Vec<(i64, String)> = Vec::new();
    let mut tools: VecDeque<(i64, String)> = VecDeque::new();
    let (mut dialogue_chars, mut tool_chars, mut omitted) = (0, 0, 0);
    let mut last = None;
    let mut after = 0;
    'read: loop {
        let page = db::session_events_after(conn, session_id, after, PAGE)?;
        let Some(end) = page.last() else {
            break;
        };
        after = end.id;
        for e in &page {
            match line(e) {
                Line::Dialogue(l) => {
                    let l = cap(l, DIALOGUE_CHARS);
                    // Every line also costs the newline that joins it.
                    let n = l.chars().count() + 1;
                    // Past its share, dialogue still fills the call while nothing else needs
                    // the room, so a short talk-heavy session stays one call.
                    if !dialogue.is_empty()
                        && dialogue_chars + n > DIALOGUE_CHARS
                        && (omitted > 0 || dialogue_chars + n + tool_chars > MAX_PROMPT_CHARS)
                    {
                        break 'read;
                    }
                    dialogue_chars += n;
                    dialogue.push((e.id, l));
                }
                Line::Tool(l) => {
                    tool_chars += l.chars().count() + 1;
                    tools.push_back((e.id, l));
                }
                Line::Skip => {}
            }
            while tool_chars > MAX_PROMPT_CHARS.saturating_sub(dialogue_chars) {
                let Some((_, l)) = tools.pop_front() else {
                    break;
                };
                tool_chars -= l.chars().count() + 1;
                omitted += 1;
            }
            last = Some((e.id, e.ts));
        }
    }
    let mut lines: Vec<(i64, String)> = dialogue;
    lines.extend(tools);
    lines.sort_by_key(|(id, _)| *id);
    let mut text: Vec<String> = lines.into_iter().map(|(_, l)| l).collect();
    if omitted > 0 {
        text.insert(
            0,
            format!("[{omitted} older tool calls of this part omitted]"),
        );
    }
    Ok(Part {
        text: text.join("\n"),
        last,
    })
}

enum Line {
    Dialogue(String),
    Tool(String),
    Skip,
}

/// One stored event as transcript text. Every field passes `redact::outbound` here: this text
/// goes to an external provider.
fn line(e: &db::RawEvent) -> Line {
    let v: Value = serde_json::from_str(&e.payload).unwrap_or(Value::Null);
    match e.event.as_str() {
        "UserPromptSubmit" => {
            let p = redact::outbound(v["prompt"].as_str().unwrap_or(""));
            if p.is_empty() {
                Line::Skip
            } else if hook::is_envelope(&p) {
                // A task report is worth summarizing, but it is not the developer speaking.
                Line::Dialogue(format!("NOTIFICATION: {}", short(&p, 1_500)))
            } else {
                Line::Dialogue(format!("USER: {p}"))
            }
        }
        "PostToolUse" | "PostToolUseFailure" => {
            if let Some(answers) = answers(&v) {
                return Line::Dialogue(answers);
            }
            let tool = redact::outbound(v["tool"].as_str().unwrap_or("?"));
            let input = short(&redact::outbound(v["input"].as_str().unwrap_or("")), 300);
            let output = short(&redact::outbound(v["output"].as_str().unwrap_or("")), 600);
            let mark = if v["failed"].as_bool().unwrap_or(false) {
                " (failed)"
            } else {
                ""
            };
            Line::Tool(format!("TOOL {tool}{mark}: {input}\n  -> {output}"))
        }
        "Stop" => {
            let a = redact::outbound(v["assistant"].as_str().unwrap_or(""));
            if a.is_empty() {
                Line::Skip
            } else {
                Line::Dialogue(format!("ASSISTANT: {}", short(&a, 1_500)))
            }
        }
        "PostCompact" => {
            let s = redact::outbound(v["summary"].as_str().unwrap_or(""));
            if s.is_empty() {
                Line::Skip
            } else {
                Line::Dialogue(format!("COMPACTED EARLIER PART: {}", short(&s, 1_500)))
            }
        }
        _ => Line::Skip,
    }
}

/// Claude Code's AskUserQuestion carries the developer's answers at the end of its input, past
/// where a tool line is cut; they are the developer speaking, so they are dialogue.
// ponytail: an input over the capture limit (`hook::clip`, 8,000 characters) has lost its answers
// before this runs and stays a tool line; the redesign keeps tool input whole.
fn answers(v: &Value) -> Option<String> {
    if v["tool"].as_str() != Some("AskUserQuestion") {
        return None;
    }
    let input: Value = serde_json::from_str(v["input"].as_str()?).ok()?;
    let answers: Vec<String> = input["answers"]
        .as_object()?
        .iter()
        .map(|(q, a)| {
            format!(
                "{q} -> {}",
                a.as_str().map_or_else(|| a.to_string(), str::to_owned)
            )
        })
        .collect();
    if answers.is_empty() {
        return None;
    }
    Some(format!(
        "USER ANSWERED: {}",
        redact::outbound(&answers.join(" / "))
    ))
}

/// A single line longer than `max` keeps its head and tail and says how much is left out.
fn cap(s: String, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s;
    }
    let half = max / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().skip(n - half).collect();
    format!("{head}…[{} characters omitted]…{tail}", n - 2 * half)
}

fn short(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// `earlier`: the session's summary so far, when earlier parts or runs already covered some of it;
/// the new summary then covers the whole session, so its newest summary row stands for all of it.
fn build_prompt(agent: &str, language: &str, earlier: Option<&str>, transcript: &str) -> String {
    let earlier = earlier
        .map(redact::outbound)
        .filter(|e| !e.trim().is_empty())
        .map(|e| format!("--- EARLIER PARTS OF THIS SESSION, AS SUMMARIZED ---\n{e}\n"))
        .unwrap_or_default();
    format!(
        "You are the long-term memory of a software developer. Below is one coding session with the `{agent}` agent.\n\
         Extract only what is worth remembering in future sessions of this repository, then write a short summary.\n\
         Observations are facts, decisions, bug fixes, discoveries, changes or the developer's stated preferences: \
         concrete, with file paths, names and numbers. Skip routine tool noise, restated instructions and anything \
         the code itself already shows. If nothing is worth remembering, return an empty observations array. \
         At most {MAX_OBSERVATIONS} observations, each with a kind, a specific title (max 80 chars) and a body of 1-3 sentences. \
         Take observations only from the SESSION part; an earlier summary is context.\n\
         The summary is 2-4 sentences about the whole session so far, earlier parts included: what was worked on, \
         what was decided, what is still open.\n\
         Write every title, body and the summary in {language}.\n\n\
         {earlier}--- SESSION ---\n{transcript}\n--- END ---"
    )
}

#[cfg(test)]
pub fn schema_for_tests() -> Value {
    schema()
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

    fn render(events: &[db::RawEvent]) -> String {
        let lines: Vec<String> = events
            .iter()
            .filter_map(|e| match line(e) {
                Line::Dialogue(l) | Line::Tool(l) => Some(l),
                Line::Skip => None,
            })
            .collect();
        lines.join("\n")
    }

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
            ts: 0,
            payload: payload.to_string(),
        };
        let prompt = build_prompt(
            "claude",
            "English",
            Some(&format!("EARLIER-SUMMARY-MARKER with {key}")),
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
        assert!(prompt.contains("EARLIER-SUMMARY-MARKER"), "{prompt}");
    }

    /// A long session: decisions in the middle, an answer to the agent's question whose text sits
    /// at the end of the tool input, one oversized prompt, thousands of tool calls around them.
    fn long_session(conn: &rusqlite::Connection) -> i64 {
        db::upsert_session(conn, "long", "claude", "/r", "/r", 1).unwrap();
        let tool = json!({"tool": "Bash", "input": "x".repeat(300), "output": "y".repeat(600), "failed": false});
        let ask = json!({"questions": [{"question": "どちらにしますか?", "header": "方針", "options": [{"label": "A", "description": "a".repeat(400)}, {"label": "B", "description": "b".repeat(400)}]}], "answers": {"どちらにしますか?": "A にする (推奨)"}}).to_string();
        let mut events = vec![("UserPromptSubmit", json!({"prompt": "HEAD-PROMPT"}))];
        events.extend((0..2_000).map(|_| ("PostToolUse", tool.clone())));
        events.push((
            "UserPromptSubmit",
            json!({"prompt": "中央の決定: 検索は trigram にする"}),
        ));
        events.push((
            "PostToolUse",
            json!({"tool": "AskUserQuestion", "input": ask, "output": ask, "failed": false}),
        ));
        events.push((
            "UserPromptSubmit",
            json!({"prompt": format!("LONG-HEAD{}LONG-TAIL", "L".repeat(30_000))}),
        ));
        events.extend((0..2_000).map(|_| ("PostToolUse", tool.clone())));
        events.push(("Stop", json!({"assistant": "TAIL-REPLY"})));
        for (i, (event, payload)) in events.iter().enumerate() {
            db::insert_event(conn, "long", event, 1 + i as i64, &payload.to_string()).unwrap();
        }
        conn.query_row("SELECT MAX(id) FROM events", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn every_dialogue_line_of_a_long_session_reaches_the_summarizer() {
        let dir = std::env::temp_dir().join(format!("oboete-observe-long-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir).unwrap();
        let last_event = long_session(&conn);
        let s = db::PendingSession {
            id: "long".into(),
            agent: "claude".into(),
            repo: "/r".into(),
            last_event_at: 1_000_000,
        };
        let mut sent = Vec::new();
        loop {
            let part = next_part(&conn, "long").unwrap();
            let Some((last, _)) = part.last else {
                break;
            };
            assert!(
                part.text.chars().count() <= MAX_PROMPT_CHARS + 200,
                "one call stays bounded"
            );
            sent.push(part.text);
            assert!(db::apply_batch(&mut conn, &s, "p", "", &[], last).unwrap());
            assert!(sent.len() < 10, "a few calls, not one per page");
        }
        let all = sent.join("\n");
        for needle in [
            "HEAD-PROMPT",
            "中央の決定: 検索は trigram にする",
            "A にする (推奨)",
            "LONG-HEAD",
            "LONG-TAIL",
            "characters omitted",
            "TAIL-REPLY",
            "older tool calls of this part omitted",
        ] {
            assert!(
                all.contains(needle),
                "{needle} never reached the summarizer"
            );
        }
        // Every tool call is either sent or counted as omitted.
        let shown = all.matches("TOOL Bash").count();
        let omitted: usize = all
            .lines()
            .filter_map(|l| {
                l.strip_prefix('[')?
                    .split(' ')
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .sum();
        assert_eq!(shown + omitted, 4_000);
        // The oversized prompt got a call of its own instead of pushing the others out.
        assert_eq!(
            sent.len(),
            3,
            "{:?}",
            sent.iter().map(|t| t.len()).collect::<Vec<_>>()
        );
        let cursor: i64 = conn
            .query_row(
                "SELECT observed_event_id FROM sessions WHERE id='long'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cursor, last_event);
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_short_talk_heavy_session_stays_one_call() {
        let dir = std::env::temp_dir().join(format!("oboete-observe-talk-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "talk", "claude", "/r", "/r", 1).unwrap();
        for i in 0..14 {
            let prompt = json!({"prompt": format!("Q{i} {}", "q".repeat(1_000))});
            db::insert_event(
                &conn,
                "talk",
                "UserPromptSubmit",
                1 + i,
                &prompt.to_string(),
            )
            .unwrap();
        }
        let part = next_part(&conn, "talk").unwrap();
        assert!(part.text.contains("Q0 ") && part.text.contains("Q13 "));
        let last: i64 = conn
            .query_row("SELECT MAX(id) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(part.last.map(|(id, _)| id), Some(last));
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn many_short_lines_still_fit_one_call() {
        let dir = std::env::temp_dir().join(format!("oboete-observe-short-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "short", "claude", "/r", "/r", 1).unwrap();
        let tool = json!({"tool": "Bash", "input": "i", "output": "o", "failed": false});
        for i in 0..3_000 {
            let (event, payload) = if i % 2 == 0 {
                ("UserPromptSubmit", json!({"prompt": "x"}))
            } else {
                ("PostToolUse", tool.clone())
            };
            db::insert_event(&conn, "short", event, 1 + i, &payload.to_string()).unwrap();
        }
        let s = db::PendingSession {
            id: "short".into(),
            agent: "claude".into(),
            repo: "/r".into(),
            last_event_at: 1_000_000,
        };
        loop {
            let part = next_part(&conn, "short").unwrap();
            let Some((last, _)) = part.last else {
                break;
            };
            let n = part.text.chars().count();
            assert!(n <= MAX_PROMPT_CHARS + 64, "{n}");
            assert!(db::apply_batch(&mut conn, &s, "p", "", &[], last).unwrap());
        }
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn harness_notifications_are_not_the_developer_speaking() {
        let ev = |event: &str, payload: Value| db::RawEvent {
            id: 0,
            event: event.into(),
            ts: 0,
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
