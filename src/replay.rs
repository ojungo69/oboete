//! Replay a JSONL fixture (one `{seq, agent, event, session, payload}` per line) through the
//! hook path, then run observe, and report what the spike must prove: hook latency, resident
//! size, summarizer success and fallback counts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::{db, hook, observe};

const ROOT_PLACEHOLDER: &str = "__OBOETE_REPLAY_ROOT__";

pub fn run(
    home: &Path,
    fixture: &Path,
    repo_root: Option<PathBuf>,
    spawn_sample: usize,
    agent: &str,
) -> Result<()> {
    let root = match repo_root {
        Some(r) => r,
        None => {
            let r = home.join("replay-repo");
            std::fs::create_dir_all(r.join(".git"))?;
            r
        }
    };
    let root_str = root.canonicalize()?.to_string_lossy().into_owned();
    let text =
        std::fs::read_to_string(fixture).with_context(|| format!("read {}", fixture.display()))?;
    // Each store is opened on the first event that needs it: a replay of ported agents never
    // touches v1's oboete.db, and one of unported agents never creates raw.db.
    let mut conn: Option<rusqlite::Connection> = None;
    let mut raw: Option<crate::raw::Raw> = None;
    let clock = rusqlite::Connection::open_in_memory()?;

    // 1. In-process hook path: pure store cost per event. Ported agents write Design B's
    // raw.db (milestone 2 Task 2), the others still v1's store.
    let mut micros: Vec<u128> = Vec::new();
    let mut injected = 0u32;
    let mut v1_events = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let line = line.replace(ROOT_PLACEHOLDER, &root_str);
        let v: Value = serde_json::from_str(&line)?;
        let ev_agent = v["agent"].as_str().unwrap_or("");
        let wanted =
            ev_agent == agent || (agent == "all" && crate::setup::AGENTS.contains(&ev_agent));
        if !wanted {
            continue;
        }
        let event = v["event"].as_str().unwrap_or("");
        let started = Instant::now();
        let out = if crate::capture::PORTED.contains(&ev_agent) {
            let ts = fixture_ms(&clock, &v["ts"]).unwrap_or_else(db::now_ms);
            if raw.is_none() {
                raw = Some(crate::raw::open(home)?);
            }
            hook::record(
                raw.as_mut().expect("opened"),
                ev_agent,
                event,
                &v["payload"],
                ts,
            )?;
            None
        } else {
            v1_events += 1;
            if conn.is_none() {
                conn = Some(db::open(home)?);
            }
            hook::handle(
                conn.as_ref().expect("opened"),
                ev_agent,
                event,
                &v["payload"],
            )?
        };
        micros.push(started.elapsed().as_micros());
        if out.as_deref().is_some_and(|s| s != "{}") {
            injected += 1;
        }
    }
    drop((conn, raw));

    // 2. Real process spawns: startup + open + insert, what the agent actually waits for.
    let spawn_ms = if spawn_sample > 0 {
        sample_spawns(home, &root_str, spawn_sample)?
    } else {
        Vec::new()
    };

    // 3. Summarize what v1's store captured (in-process; nothing spawns here). observe reads only
    // oboete.db: ported agents' raw records wait for milestone 3's curation, so a replay of them
    // alone reports no summary rather than a summary of nothing.
    let stats = if v1_events > 0 {
        Some(observe::run(home, 0)?)
    } else {
        None
    };

    micros.sort_unstable();
    let report = json!({
        "events": micros.len(),
        "hook_in_process_us": {"p50": pct(&micros, 50), "p95": pct(&micros, 95), "max": micros.last().copied().unwrap_or(0)},
        "hook_spawn_ms": {"n": spawn_ms.len(), "p50": pct(&spawn_ms, 50), "p95": pct(&spawn_ms, 95), "max": spawn_ms.last().copied().unwrap_or(0)},
        "session_start_injections": injected,
        "observe": stats,
        "observe_covers": format!("the {v1_events} events of agents not in capture::PORTED {:?}", crate::capture::PORTED),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// A fixture line's `ts` (RFC 3339, as `oboete transcript` writes it) in unix ms; `None` when the
/// line has none (events-1000.jsonl), so replay falls back to now (issue #65).
fn fixture_ms(conn: &rusqlite::Connection, ts: &Value) -> Option<i64> {
    conn.query_row(
        "SELECT CAST(round(unixepoch(?1, 'subsec') * 1000) AS INTEGER)",
        [ts.as_str()?],
        |r| r.get(0),
    )
    .ok()
    .flatten()
}

fn sample_spawns(home: &Path, root: &str, n: usize) -> Result<Vec<u128>> {
    let exe = std::env::current_exe()?;
    let payload = json!({
        "session_id": "spawn-sample",
        "cwd": root,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "spawn sample prompt: measure process start plus one insert"
    })
    .to_string();
    let mut ms = Vec::with_capacity(n);
    for _ in 0..n {
        let started = Instant::now();
        let mut child = std::process::Command::new(&exe)
            .arg("--home")
            .arg(home)
            .args(["hook", "claude", "UserPromptSubmit"])
            .env("OBOETE_NO_SPAWN", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(payload.as_bytes())?;
        }
        child.wait()?;
        ms.push(started.elapsed().as_millis());
    }
    ms.sort_unstable();
    Ok(ms)
}

fn pct(sorted: &[u128], p: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ported_replay_keeps_fixture_times_and_reports_no_v1_summary() {
        let home = tempfile::tempdir().unwrap();
        let fixture = home.path().join("f.jsonl");
        let line = |ts: &str, prompt: &str| {
            json!({"agent": "claude", "event": "UserPromptSubmit", "ts": ts,
                   "payload": {"session_id": "s", "prompt": prompt}})
            .to_string()
        };
        let lines = [
            line("2026-09-01T00:00:00.000Z", "one"),
            line("2026-09-02T00:00:00.000Z", "two"),
        ];
        std::fs::write(&fixture, lines.join("\n")).unwrap();
        run(home.path(), &fixture, None, 0, "claude").unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let ts: Vec<i64> = raw
            .after(raw.device(), 0, 10)
            .unwrap()
            .into_iter()
            .map(|r| match r.item {
                crate::raw::Item::Event(e) => e.ts,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ts, vec![1_788_220_800_000, 1_788_307_200_000]);
        // observe never ran (it would have created its lock file), and v1's store was never opened.
        assert!(!home.path().join("observe.lock").exists());
        assert!(!home.path().join("oboete.db").exists());
    }

    #[test]
    fn fixture_times_are_read_to_the_millisecond() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let ms = |t: &str| fixture_ms(&conn, &json!(t));
        assert_eq!(ms("1970-01-02T00:00:00.000Z"), Some(86_400_000));
        assert_eq!(ms("2026-09-01T00:00:00.123Z"), Some(1_788_220_800_123));
        assert_eq!(ms("not a time"), None);
        assert_eq!(fixture_ms(&conn, &Value::Null), None);
    }
}
