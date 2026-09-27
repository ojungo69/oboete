//! Replay a JSONL fixture (one `{seq, agent, event, session, payload}` per line) through the
//! hook path, and report what M14 must prove: hook latency in process and spawned, backup export
//! time per segment, and resident size.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::{db, hook};

const ROOT_PLACEHOLDER: &str = "__OBOETE_REPLAY_ROOT__";

pub fn run(
    home: &Path,
    fixture: &Path,
    repo_root: Option<PathBuf>,
    spawn_sample: usize,
    sizes: &[usize],
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
    // The placeholder sits inside the fixture's JSON strings: a Windows path's backslashes are
    // escaped there, or the line no longer parses.
    let root_json = serde_json::to_string(&root_str)?;
    let root_json = &root_json[1..root_json.len() - 1];
    let text =
        std::fs::read_to_string(fixture).with_context(|| format!("read {}", fixture.display()))?;
    // Opened on the first event replayed: a fixture with none of the agent's events creates no
    // store.
    let mut raw: Option<crate::raw::Raw> = None;
    // Loaded once: the in-process loop measures the store; the spawned hooks below load it each.
    let settings = crate::capture::Settings {
        source: "replay",
        ..crate::capture::Settings::load(home)?
    };
    let clock = rusqlite::Connection::open_in_memory()?;

    // 1. In-process hook path: pure store cost per event.
    let mut micros: Vec<u128> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let line = line.replace(ROOT_PLACEHOLDER, root_json);
        let v: Value = serde_json::from_str(&line)?;
        let ev_agent = v["agent"].as_str().unwrap_or("");
        let wanted =
            ev_agent == agent || (agent == "all" && crate::setup::AGENTS.contains(&ev_agent));
        if !wanted {
            continue;
        }
        let event = v["event"].as_str().unwrap_or("");
        let started = Instant::now();
        let ts = fixture_ms(&clock, &v["ts"]).unwrap_or_else(db::now_ms);
        if raw.is_none() {
            raw = Some(crate::raw::open(home)?);
        }
        hook::record(
            home,
            raw.as_mut().expect("opened"),
            ev_agent,
            event,
            &v["payload"],
            ts,
            &settings,
        )?;
        micros.push(started.elapsed().as_micros());
    }
    drop(raw);

    // 2. Real process spawns: startup + open + redaction + insert + the worker-lock attempt,
    // what the agent actually waits for, per tool-output size (M14). The replayed agent's hook.
    let spawn_agent = if agent == "all" { "claude" } else { agent };
    let mut spawns = serde_json::Map::new();
    if spawn_sample > 0 {
        for &kb in sizes {
            let us = sample_spawns(home, &root_str, spawn_sample, spawn_agent, kb * 1024)?;
            spawns.insert(format!("{kb}KB"), stats_ms(&us));
        }
    }
    // 3. Backup export per segment (D11): each call seals one segment of at most
    // `backup::SEGMENT_BYTES` of records.
    let mut export_us = Vec::new();
    let mut segment_kb = Vec::new();
    if crate::raw::exists(home) {
        for (path, took) in crate::backup::export_timed(home)? {
            export_us.push(took.as_micros());
            segment_kb.push(std::fs::metadata(&path).map_or(0, |m| m.len() as u128 / 1024));
        }
    }
    export_us.sort_unstable();
    segment_kb.sort_unstable();

    micros.sort_unstable();
    let report = json!({
        "events": micros.len(),
        "hook_in_process_us": {"p50": pct(&micros, 50), "p95": pct(&micros, 95), "max": micros.last().copied().unwrap_or(0)},
        "hook_spawn_ms": spawns,
        "backup_export": {"segments": export_us.len(), "ms": stats_ms(&export_us), "segment_kb": {"p50": pct(&segment_kb, 50), "max": segment_kb.last().copied().unwrap_or(0)}},
        "vmhwm_kb": crate::observe::vmhwm_kb(),
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

/// p50, p95, p99 and max of sorted microseconds, in milliseconds with one decimal.
fn stats_ms(sorted_us: &[u128]) -> Value {
    let ms = |us: u128| (us as f64 / 100.0).round() / 10.0;
    json!({"n": sorted_us.len(), "p50": ms(pct(sorted_us, 50)), "p95": ms(pct(sorted_us, 95)),
           "p99": ms(pct(sorted_us, 99)), "max": ms(sorted_us.last().copied().unwrap_or(0))})
}

/// Tool-output-like text of `bytes` bytes: paths, code and Japanese, every 20th line with words
/// that wake redaction rules (api, key, token, password) but no secret, as Spike 1's payload
/// (docs/spike/hook-m14.md).
fn tool_output(bytes: usize) -> String {
    let lines = [
        "src/consumer/manifest.rs:412: fn rebuild(raw: &Raw, k: &Connection) -> Result<()> {",
        "    let text = format!(\"{}: {}\", tool, one_line(&output, CLIP));",
        "テストが 3 件失敗しました。原因はチェックポイントの巻き戻しです。",
        "warning: unused variable `seq` in /home/user/projects/app/src/lib.rs",
    ];
    let mut out = String::with_capacity(bytes + 128);
    let mut i = 0;
    while out.len() < bytes {
        if i % 20 == 5 {
            out.push_str("the api key and token come from the password manager, not this file\n");
        } else {
            out.push_str(lines[i % lines.len()]);
            out.push('\n');
        }
        i += 1;
    }
    let mut end = bytes.min(out.len());
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out
}

/// `n` spawned `oboete hook <agent> PostToolUse` runs with a tool output of `bytes`, in
/// microseconds, sorted. This process holds the worker lock meanwhile, so each hook makes the
/// lock attempt it makes after every write (D6) and starts no worker, as while one runs.
fn sample_spawns(
    home: &Path,
    root: &str,
    n: usize,
    agent: &str,
    bytes: usize,
) -> Result<Vec<u128>> {
    let exe = std::env::current_exe()?;
    let payload = json!({
        "session_id": "spawn-sample",
        "cwd": root,
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": tool_output(bytes),
    })
    .to_string();
    let _held = crate::worker::lock(home)?;
    let mut us = Vec::with_capacity(n);
    for _ in 0..n {
        let started = Instant::now();
        let mut child = std::process::Command::new(&exe)
            .arg("--home")
            .arg(home)
            .args(["hook", agent, "PostToolUse"])
            .env(crate::capture::REPLAY_ENV, "1")
            // A candidate above today's cap is measured as written under that cap (D4).
            .env(
                crate::capture::FIELD_CAP_ENV,
                bytes.max(crate::capture::MAX_FIELD_BYTES).to_string(),
            )
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
        us.push(started.elapsed().as_micros());
    }
    us.sort_unstable();
    Ok(us)
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
        run(home.path(), &fixture, None, 0, &[1], "claude").unwrap();
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
    fn the_24_hour_fixture_keeps_its_day_in_raw() {
        // Issue #65: replay records each event at the fixture's time, not at now.
        let home = tempfile::tempdir().unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/fixtures/long-24h.jsonl");
        run(home.path(), &fixture, None, 0, &[1], "claude").unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let ts: Vec<i64> = raw
            .after(raw.device(), 0, 10_000)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                crate::raw::Item::Event(e) => Some(e.ts),
                _ => None,
            })
            .collect();
        let span = ts.iter().max().unwrap() - ts.iter().min().unwrap();
        assert!(span >= 23 * 3_600_000 + 1_800_000, "{span}");
        // D9: a replay is not the owner at work.
        assert_eq!(raw.last_hook_ts().unwrap(), None);
        // The export ran: segments exist for what was recorded.
        assert!(
            home.path()
                .join("backups")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
    }

    #[test]
    fn a_repo_root_with_a_backslash_replays() {
        // Windows paths put backslashes into the fixture's JSON strings: they are escaped there.
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("repo\\xroot");
        std::fs::create_dir_all(&root).unwrap();
        let fixture = home.path().join("f.jsonl");
        let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "cwd": ROOT_PLACEHOLDER, "prompt": "one"}});
        std::fs::write(&fixture, line.to_string()).unwrap();
        run(home.path(), &fixture, Some(root), 0, &[1], "claude").unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        assert_eq!(raw.after(raw.device(), 0, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_sample_output_has_the_size_asked_for() {
        for bytes in [1024, 64 * 1024, 256 * 1024] {
            let t = tool_output(bytes);
            assert!(t.len() <= bytes && t.len() > bytes - 4, "{}", t.len());
            assert!(t.contains("token") && t.contains("テスト"));
        }
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
