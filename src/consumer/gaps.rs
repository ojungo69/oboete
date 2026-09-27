//! Milestone 2 Task 11 (spec 2.3): when a session ends, the worker counts the turns its transcript
//! holds against the ones raw recorded, and doctor reports the sessions short of their transcript
//! per agent. Backfilling from transcripts comes later, and only if gaps show up.

use crate::capture::Settings;
use crate::raw::{Item, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Gaps {
    home: PathBuf,
}

impl Gaps {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }
}

/// Records per step, as the other consumers.
const BATCH: usize = 500;

fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS gaps(
           device TEXT NOT NULL, agent TEXT NOT NULL, session TEXT NOT NULL,
           seq INTEGER NOT NULL,        -- the end record it was checked at
           transcript_turns INTEGER,    -- NULL: not checked (no transcript a parser reads)
           raw_turns INTEGER NOT NULL,
           checked_at INTEGER NOT NULL,
           PRIMARY KEY (device, agent, session)
         );
         -- Devices whose rows a rewind dropped: the next step reads raw from seq 1 again.
         CREATE TABLE IF NOT EXISTS gaps_rebuild(device TEXT PRIMARY KEY);",
    )?;
    Ok(())
}

impl Consumer for Gaps {
    fn name(&self) -> &'static str {
        "gaps"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let device = raw.device();
        // After a rewind, from seq 1: its checkpoint moves back, which the worker takes.
        let from = if k.execute("DELETE FROM gaps_rebuild WHERE device = ?1", [device])? > 0 {
            0
        } else {
            after
        };
        let recs = raw.after(device, from, BATCH)?;
        let mut settings = None;
        for r in &recs {
            let Item::Event(e) = &r.item else { continue };
            if e.kind != "end" {
                continue;
            }
            let body: Value = serde_json::from_str(&e.body).unwrap_or(Value::Null);
            // Capture's own rules count the transcript's side (settings that do not load stop
            // capture too, and doctor names them: the bundled ones then).
            let settings = settings.get_or_insert_with(|| {
                Settings::load(&self.home).unwrap_or_else(|_| Settings::default())
            });
            // ponytail: the whole transcript is parsed once per session end (0.3 s for the
            // largest dev session, 111 MB); a resumed session's next end parses it again.
            let transcript = body["transcript"]
                .as_str()
                .and_then(|p| transcript_turns(Path::new(p), &e.agent, settings).ok());
            k.execute(
                "INSERT INTO gaps(device, agent, session, seq, transcript_turns, raw_turns, checked_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(device, agent, session) DO UPDATE SET seq = excluded.seq,
                   transcript_turns = excluded.transcript_turns, raw_turns = excluded.raw_turns,
                   checked_at = excluded.checked_at",
                params![
                    device,
                    e.agent,
                    e.session,
                    r.seq,
                    transcript,
                    raw.turns(&e.agent, &e.session)?,
                    crate::db::now_ms()
                ],
            )?;
        }
        Ok(recs.last().map_or(from, |r| r.seq))
    }

    /// A session's row may come from an end among the lost commits while an older end of it
    /// is still in raw, so every session is checked again from seq 1. ponytail: that parses
    /// every transcript still on disk once, as a knowledge.db rebuilt from empty does; lost
    /// commits are rare (MUST-M14).
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        k.execute("DELETE FROM gaps WHERE device = ?1", [device])?;
        k.execute(
            "INSERT OR IGNORE INTO gaps_rebuild(device) VALUES(?1)",
            [device],
        )?;
        Ok(())
    }
}

/// Record types the parsers pass over that hold no typed turn: those in docs/milestone-1.md's
/// transcript notes, and every type passed over in the owner's 300 newest Claude Code and 150
/// newest Codex transcripts (2026-09-27; Codex's are `kind:subkind`). A type not listed may be a
/// new place for a prompt, so a transcript with one is not checked.
const NO_TURNS: &[&str] = &[
    // Claude Code
    "agent-name",
    "ai-title",
    "atis-latch",
    "attachment",
    "bridge-session",
    "cost-state",
    "custom-title",
    "file-history-delta",
    "file-history-snapshot",
    "fork-context-ref",
    "frame-link",
    "last-prompt",
    "mode",
    "permission-mode",
    "pr-link",
    "queue-operation",
    "system",
    // Codex
    "event_msg:item_completed",
    "event_msg:task_started",
    "event_msg:thread_settings_applied",
    "event_msg:token_count",
    "inter_agent_communication_metadata",
    "response_item:agent_message",
    "response_item:reasoning",
    "token_usage_record",
    "world_state",
];

/// The turns the prompt hook would have recorded from this transcript: each prompt it implies
/// goes through capture's own rules, so both sides count the same thing (a prompt that was all
/// `<private>` is no turn on either).
fn transcript_turns(path: &Path, agent: &str, settings: &Settings) -> Result<i64> {
    let mut count = Count {
        agent,
        settings,
        line: Vec::new(),
        turns: 0,
    };
    let stats = crate::transcript::convert(path, agent, &mut count)?;
    // A line that is not JSON may have held a turn: the count would hide the gap it shows.
    anyhow::ensure!(stats.skipped == 0, "{} unreadable line(s)", stats.skipped);
    anyhow::ensure!(
        stats
            .ignored
            .keys()
            .all(|k| NO_TURNS.contains(&k.trim_end_matches(':'))),
        "a record type no parser knows"
    );
    Ok(count.turns)
}

/// Reads `oboete transcript`'s lines as they are written, keeping one line at a time.
struct Count<'a> {
    agent: &'a str,
    settings: &'a Settings,
    line: Vec<u8>,
    turns: i64,
}

impl Write for Count<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &b in buf {
            if b != b'\n' {
                self.line.push(b);
                continue;
            }
            let v: Value = serde_json::from_slice(&self.line).unwrap_or(Value::Null);
            self.line.clear();
            if v["event"] == "UserPromptSubmit" {
                let events = crate::capture::events(
                    self.agent,
                    "UserPromptSubmit",
                    &v["payload"],
                    0,
                    self.settings,
                );
                self.turns += events
                    .iter()
                    .filter(|c| matches!(c.event.kind.as_str(), "prompt" | "envelope"))
                    .count() as i64;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Doctor's lines: per agent, the ended sessions short of their transcript, and those not checked.
pub fn doctor(k: &Connection) -> Vec<String> {
    let rows = k
        .prepare(
            "SELECT agent, COUNT(*), COUNT(transcript_turns),
                    SUM(transcript_turns > raw_turns),
                    SUM(MAX(COALESCE(transcript_turns, 0) - raw_turns, 0))
             FROM gaps GROUP BY agent ORDER BY agent",
        )
        .and_then(|mut st| {
            st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        // No table yet: the worker has seen no session end.
        .unwrap_or_default();
    rows.into_iter()
        .map(|(agent, ended, checked, short, missing)| {
            if checked == 0 {
                return format!(
                    "{agent}: {ended} ended session(s), not checked (no transcript oboete could read)"
                );
            }
            let mut line = format!(
                "{agent}: {short} of {checked} ended session(s) short of their transcript ({missing} turn(s) not recorded)"
            );
            if ended > checked {
                line.push_str(&format!(
                    "; {} not checked (no transcript oboete could read)",
                    ended - checked
                ));
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{knowledge, raw, worker};
    use serde_json::json;

    /// A Claude Code transcript: one typed prompt per text, each answered.
    fn transcript(path: &Path, prompts: &[&str]) {
        let mut lines = String::new();
        for (i, p) in prompts.iter().enumerate() {
            let ts = |s: usize| format!("2026-09-01T00:00:{:02}.000Z", 2 * i + s);
            lines.push_str(
                &json!({"type": "user", "sessionId": "s1", "cwd": "/w", "timestamp": ts(0),
                        "message": {"role": "user", "content": p}})
                .to_string(),
            );
            lines.push('\n');
            lines.push_str(
                &json!({"type": "assistant", "sessionId": "s1", "cwd": "/w", "timestamp": ts(1),
                        "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}})
                .to_string(),
            );
            lines.push('\n');
        }
        std::fs::write(path, lines).unwrap();
    }

    fn end(agent: &str, session: &str, transcript: Option<&Path>) -> raw::Event {
        let mut payload = json!({"session_id": session, "reason": "exit"});
        if let Some(t) = transcript {
            payload["transcript_path"] = json!(t);
        }
        let mut c = crate::capture::events(agent, "SessionEnd", &payload, 9, &Settings::default());
        c.remove(0).event
    }

    fn prompt(session: &str, text: &str) -> raw::Event {
        raw::Event {
            session: session.into(),
            body: json!({"prompt": text}).to_string(),
            ..raw::test_event("")
        }
    }

    #[test]
    fn a_lost_end_leaves_the_session_checked_at_its_older_one() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("s1.jsonl");
        transcript(&file, &["one", "two"]);
        let mut store = raw::open(home.path()).unwrap();
        store.append(&prompt("s1", "one")).unwrap();
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        worker::run_once(home.path()).unwrap();
        // Resumed, and ended again; then that end is lost (MUST-M14).
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        worker::run_once(home.path()).unwrap();
        let top = store.max_seq().unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let k = knowledge::open(home.path()).unwrap();
        assert_eq!(
            doctor(&k),
            vec![
                "claude: 1 of 1 ended session(s) short of their transcript (1 turn(s) not recorded)"
            ]
        );
    }

    #[test]
    fn a_session_short_of_its_transcript_is_reported_for_its_agent() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("s1.jsonl");
        // Five typed prompts, and one that was all `<private>`: no turn on either side.
        transcript(
            &file,
            &[
                "one",
                "two",
                "three",
                "<private>x</private>",
                "four",
                "five",
            ],
        );
        let mut store = raw::open(home.path()).unwrap();
        for text in ["one", "two", "three", "five"] {
            store.append(&prompt("s1", text)).unwrap();
        }
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        // Codex's session sent no transcript path: not checked, never a gap.
        store.append(&end("codex", "c1", None)).unwrap();
        // A transcript with a line that is not JSON: not checked either.
        let torn = home.path().join("s2.jsonl");
        transcript(&torn, &["one"]);
        let mut text = std::fs::read_to_string(&torn).unwrap();
        text.push_str("{\"type\": \"user\", \"message\n");
        std::fs::write(&torn, text).unwrap();
        store.append(&end("claude", "s2", Some(&torn))).unwrap();
        // One with a record type no parser knows (it may hold a prompt): not checked.
        let new = home.path().join("s3.jsonl");
        transcript(&new, &["one"]);
        let mut text = std::fs::read_to_string(&new).unwrap();
        text.push_str("{\"type\": \"typed-request\", \"sessionId\": \"s1\"}\n");
        std::fs::write(&new, text).unwrap();
        store.append(&end("claude", "s3", Some(&new))).unwrap();
        worker::run_once(home.path()).unwrap();
        let k = knowledge::open(home.path()).unwrap();
        let row: (Option<i64>, i64) = k
            .query_row(
                "SELECT transcript_turns, raw_turns FROM gaps WHERE session = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (Some(5), 4));
        assert_eq!(
            doctor(&k),
            vec![
                "claude: 1 of 1 ended session(s) short of their transcript (1 turn(s) not recorded); 2 not checked (no transcript oboete could read)",
                "codex: 1 ended session(s), not checked (no transcript oboete could read)",
            ]
        );
    }
}
