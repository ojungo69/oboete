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
         );",
    )?;
    Ok(())
}

impl Consumer for Gaps {
    fn name(&self) -> &'static str {
        "gaps"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> {
        schema(k)?;
        let device = raw.device();
        let recs = raw.after(device, after, BATCH)?;
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
        Ok(recs.last().map_or(after, |r| r.seq))
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM gaps WHERE device = ?1 AND seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}

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
    crate::transcript::convert(path, agent, &mut count)?;
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
                    "{agent}: {ended} ended session(s), not checked (no transcript oboete reads)"
                );
            }
            let mut line = format!(
                "{agent}: {short} of {checked} ended session(s) short of their transcript ({missing} turn(s) not recorded)"
            );
            if ended > checked {
                line.push_str(&format!(
                    "; {} not checked (no transcript oboete reads)",
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
                "claude: 1 of 1 ended session(s) short of their transcript (1 turn(s) not recorded)",
                "codex: 1 ended session(s), not checked (no transcript oboete reads)",
            ]
        );
    }
}
