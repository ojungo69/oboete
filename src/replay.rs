//! Replay a JSONL fixture (one `{seq, agent, event, session, payload}` per line) through the
//! hook path, and report what M14 must prove: hook latency in process and spawned, backup export
//! time per segment, and resident size; with `--read-sample`, also the read path's (milestone 4
//! Task 0).

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
    read_sample: usize,
) -> Result<()> {
    // Grok injects at PreToolUse and agy at PreInvocation, with payloads of their own: the read
    // sample spawns Claude Code's SessionStart and prompt hooks only.
    if read_sample > 0 && !matches!(agent, "claude" | "all") {
        anyhow::bail!("--read-sample times Claude Code's hooks: use --agent claude or all");
    }
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
    // This process holds the worker lock while it times hooks, so each hook's lock attempt starts
    // no worker; from before the replay, so no worker consumes the replayed records before the
    // cold arm reads (Codex on #301).
    let busy =
        "another process holds the worker lock: stop it before --spawn-sample or --read-sample";
    let mut held = if spawn_sample > 0 || read_sample > 0 {
        Some(crate::worker::lock(home)?.context(busy)?)
    } else {
        None
    };

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
    // Hooks timed for their write alone record in a checkout of their own, so the fixture's
    // checkout, whose manifest the read arms show, keeps its last prompt and commands (Codex on
    // #301). SessionStart reads the fixture's checkout, so it records there.
    let samples = match &held {
        Some(_) => {
            let s = home.join("sample-repo");
            std::fs::create_dir_all(s.join(".git"))?;
            s.canonicalize()?.to_string_lossy().into_owned()
        }
        None => String::new(),
    };
    let mut spawns = serde_json::Map::new();
    if let Some(held) = held.as_ref().filter(|_| spawn_sample > 0) {
        for &kb in sizes {
            let us = sample_spawns(home, held, &samples, spawn_sample, spawn_agent, kb * 1024)?;
            spawns.insert(format!("{kb}KB"), stats_ms(&us));
        }
    }
    // 2b. The read path (milestone 4 Task 0): cold, before any consumer has run, as a hook finds
    // the home before the worker is up (spec 4.2); warm, after the consumers are drained.
    let mut read = serde_json::Map::new();
    if let Some(cold) = held.take().filter(|_| read_sample > 0) {
        read.insert(
            "cold".into(),
            read_arm(home, &cold, &root_str, &samples, read_sample, spawn_agent)?,
        );
        drop(cold);
        drain_for_read(home)?;
        let warm = crate::worker::lock(home)?.context(busy)?;
        read.insert(
            "warm".into(),
            read_arm(home, &warm, &root_str, &samples, read_sample, spawn_agent)?,
        );
        // Its forgetting applied: the home is left as the replay made it, for a run on it again.
        drop(warm);
        drain_for_read(home)?;
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
        "read": read,
        "backup_export": {"segments": export_us.len(), "ms": stats_ms(&export_us), "segment_kb": {"p50": pct(&segment_kb, 50), "max": segment_kb.last().copied().unwrap_or(0)}},
        "vmhwm_kb": vmhwm_kb(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Peak resident size of this process (Linux), for the report.
fn vmhwm_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
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
/// microseconds, sorted.
fn sample_spawns(
    home: &Path,
    held: &crate::worker::Lock,
    root: &str,
    n: usize,
    agent: &str,
    bytes: usize,
) -> Result<Vec<u128>> {
    let payload = json!({
        "session_id": "spawn-sample",
        "cwd": root,
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": tool_output(bytes),
    })
    .to_string();
    // A candidate above today's cap is measured as written under that cap (D4).
    let cap = bytes.max(crate::capture::MAX_FIELD_BYTES).to_string();
    let env = [
        (crate::capture::REPLAY_ENV, "1".to_owned()),
        (crate::capture::FIELD_CAP_ENV, cap),
    ];
    Ok(time_spawns(home, held, n, agent, "PostToolUse", &payload, &env)?.0)
}

/// Drains the consumers for the warm arm; an error when another process holds the worker lock,
/// whose consumers may not be drained.
fn drain_for_read(home: &Path) -> Result<()> {
    crate::worker::drained(home)?
        .map(drop)
        .context("another process holds the worker lock: stop it before --read-sample")
}

/// The session the read arms' hooks record under.
const SAMPLE_SESSION: &str = "read-sample";

/// The read path's times on `home` as it stands: `n` spawned SessionStart hooks for the checkout
/// at `root` and `n` prompt hooks in the checkout at `samples`, as the agent runs them (the write,
/// the read of what is injected, the lock attempt), and `n` in-process reads of what SessionStart
/// shows for `root`. The prompts are recorded away from `root`, whose last prompt the warm arm
/// shows, and every record the arm's hooks made is forgotten after it.
fn read_arm(
    home: &Path,
    held: &crate::worker::Lock,
    root: &str,
    samples: &str,
    n: usize,
    agent: &str,
) -> Result<Value> {
    let hook = |event: &str, cwd: &str, extra: Value| {
        let mut payload = json!({"session_id": SAMPLE_SESSION, "cwd": cwd,
                                 "hook_event_name": event});
        payload
            .as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        let env = [(crate::capture::REPLAY_ENV, "1".to_owned())];
        time_spawns(home, held, n, agent, event, &payload.to_string(), &env)
    };
    let mut raw = crate::raw::open(home)?;
    let before = raw.max_seq_of(raw.device())?;
    let (starts, printed) = hook("SessionStart", root, json!({"source": "startup"}))?;
    let (prompts, _) = hook(
        "UserPromptSubmit",
        samples,
        json!({"prompt": "How did we fix the flaky test last time?"}),
    )?;
    let (reads, chars) = read_in_process(home, Path::new(root), n);
    forget_samples(&mut raw, before)?;
    Ok(json!({
        "session_start_ms": stats_ms(&starts),
        "session_start_printed_bytes": printed,
        "prompt_ms": stats_ms(&prompts),
        "read_in_process_us": {"p50": pct(&reads, 50), "p95": pct(&reads, 95),
                               "max": reads.last().copied().unwrap_or(0)},
        "read_chars": chars,
    }))
}

/// Forgets what the read arms' hooks recorded after `before`, and nothing another process recorded
/// meanwhile: a start on the fixture's checkout would be its newest event, and move its manifest's
/// as-of time to now for the drain and the next arm (Codex on #301).
fn forget_samples(raw: &mut crate::raw::Raw, before: i64) -> Result<()> {
    let device = raw.device().to_owned();
    let mut sampled = Vec::new();
    let mut after = before;
    loop {
        let records = raw.after(&device, after, 500)?;
        let Some(last) = records.last() else { break };
        after = last.seq;
        for r in records {
            if let crate::raw::Item::Event(e) = &r.item
                && e.session == SAMPLE_SESSION
                && e.source == "replay"
            {
                sampled.push(r.seq);
            }
        }
    }
    for seq in sampled {
        raw.append_tombstone(crate::raw::Target::Record {
            device: device.clone(),
            seq,
        })?;
    }
    Ok(())
}

/// `n` in-process reads of what SessionStart shows for the checkout at `cwd` (`oboete inject`'s
/// text), in microseconds, sorted, and the characters of the last one.
fn read_in_process(home: &Path, cwd: &Path, n: usize) -> (Vec<u128>, usize) {
    let mut us = Vec::with_capacity(n);
    let mut chars = 0;
    for _ in 0..n {
        let started = Instant::now();
        chars = hook::inject_text(home, cwd, Some(SAMPLE_SESSION))
            .chars()
            .count();
        us.push(started.elapsed().as_micros());
    }
    us.sort_unstable();
    (us, chars)
}

/// `n` spawned `oboete hook <agent> <event>` runs fed `payload`, in microseconds, sorted, and the
/// bytes the last one printed. The caller holds the worker lock (`_held`), so each hook makes the
/// lock attempt it makes after every write (D6) and starts no worker, as while one runs.
fn time_spawns(
    home: &Path,
    _held: &crate::worker::Lock,
    n: usize,
    agent: &str,
    event: &str,
    payload: &str,
    env: &[(&str, String)],
) -> Result<(Vec<u128>, usize)> {
    use std::io::Write;
    let exe = std::env::current_exe()?;
    let mut us = Vec::with_capacity(n);
    let mut printed = 0;
    for _ in 0..n {
        let started = Instant::now();
        let mut child = std::process::Command::new(&exe)
            .arg("--home")
            .arg(home)
            .args(["hook", agent, event])
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        // Dropped at the end of the statement: the hook reads to the end of its input.
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(payload.as_bytes())?;
        let out = child.wait_with_output()?;
        // A hook that failed ran another path than the one timed (Codex on #301). It fails open,
        // exiting 0 with its error on stderr, where a hook that worked prints nothing.
        anyhow::ensure!(
            out.status.success() && out.stderr.is_empty(),
            "a sampled `oboete hook {agent} {event}` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        us.push(started.elapsed().as_micros());
        printed = out.stdout.len();
    }
    us.sort_unstable();
    Ok((us, printed))
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
        run(home.path(), &fixture, None, 0, &[1], "claude", 0).unwrap();
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
        // v1's store was never opened.
        assert!(!home.path().join("oboete.db").exists());
    }

    #[test]
    fn the_24_hour_fixture_keeps_its_day_in_raw() {
        // Issue #65: replay records each event at the fixture's time, not at now.
        let home = tempfile::tempdir().unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/fixtures/long-24h.jsonl");
        run(home.path(), &fixture, None, 0, &[1], "claude", 0).unwrap();
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
        run(home.path(), &fixture, Some(root), 0, &[1], "claude", 0).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        assert_eq!(raw.after(raw.device(), 0, 10).unwrap().len(), 1);
    }

    /// Milestone 4 Task 0, the read arms: before the consumers run, SessionStart shows nothing for
    /// the checkout; once they are drained it shows the manifest, the replayed prompt with it.
    #[test]
    fn a_cold_read_shows_nothing_and_a_warm_one_shows_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("replay-repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let fixture = home.path().join("f.jsonl");
        let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "cwd": ROOT_PLACEHOLDER,
                                      "prompt": "Fix the flaky test first."}});
        std::fs::write(&fixture, line.to_string()).unwrap();
        run(
            home.path(),
            &fixture,
            Some(root.clone()),
            0,
            &[1],
            "claude",
            0,
        )
        .unwrap();
        let (cold, chars) = read_in_process(home.path(), &root, 3);
        assert_eq!((cold.len(), chars), (3, 0));
        drop(crate::worker::drained(home.path()).unwrap());
        let (warm, chars) = read_in_process(home.path(), &root, 3);
        assert_eq!(warm.len(), 3);
        let text = hook::inject_text(home.path(), &root, Some("read-sample"));
        assert_eq!(chars, text.chars().count());
        assert!(text.contains("Fix the flaky test first."), "{text}");
    }

    /// cubic on #301: a warm arm whose drain did not run would report the cold path as warm.
    #[test]
    fn the_warm_arm_refuses_a_home_whose_worker_lock_is_held() {
        let home = tempfile::tempdir().unwrap();
        crate::raw::open(home.path()).unwrap();
        let _held = crate::worker::lock(home.path()).unwrap().unwrap();
        let err = drain_for_read(home.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("holds the worker lock"),
            "{err:#}"
        );
    }

    /// Codex on #301: a worker running when the replay starts would consume the replayed records
    /// before the cold arm reads, and one that stops before the drain would go unnoticed.
    #[test]
    fn the_read_sample_refuses_a_worker_running_before_the_replay() {
        let home = tempfile::tempdir().unwrap();
        let fixture = home.path().join("f.jsonl");
        let line = json!({"agent": "claude", "event": "UserPromptSubmit",
                          "ts": "2026-09-01T00:00:00.000Z",
                          "payload": {"session_id": "s", "prompt": "one"}});
        std::fs::write(&fixture, line.to_string()).unwrap();
        let _held = crate::worker::lock(home.path()).unwrap().unwrap();
        let err = run(home.path(), &fixture, None, 0, &[1], "claude", 1).unwrap_err();
        assert!(
            format!("{err:#}").contains("holds the worker lock"),
            "{err:#}"
        );
        assert!(!home.path().join("raw.db").exists());
    }

    /// cubic on #301: Grok and agy inject at other points, with other payloads.
    #[test]
    fn the_read_sample_is_for_claude_code_only() {
        let home = tempfile::tempdir().unwrap();
        let fixture = home.path().join("f.jsonl");
        std::fs::write(&fixture, "").unwrap();
        for agent in ["grok", "agy", "codex"] {
            assert!(run(home.path(), &fixture, None, 0, &[1], agent, 1).is_err());
        }
        assert!(!home.path().join("raw.db").exists());
    }

    /// #301: a read arm forgets its hooks' records, never one another process recorded meanwhile.
    #[test]
    fn a_read_arm_forgets_only_its_own_records() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let before = raw.max_seq_of(raw.device()).unwrap();
        let event = |session: &str, source: &str| crate::raw::Event {
            session: session.into(),
            source: source.into(),
            body: json!({"prompt": "hello"}).to_string(),
            ..crate::raw::test_event("")
        };
        let sampled = raw.append(&event(SAMPLE_SESSION, "replay")).unwrap();
        let real = raw.append(&event("s1", "hook")).unwrap();
        let replayed = raw.append(&event("s2", "replay")).unwrap();
        forget_samples(&mut raw, before).unwrap();
        let kept: Vec<(i64, bool)> = raw
            .after(raw.device(), before, 10)
            .unwrap()
            .into_iter()
            .filter(|r| !matches!(r.item, crate::raw::Item::Tombstone(_)))
            .map(|r| (r.seq, matches!(r.item, crate::raw::Item::Event(_))))
            .collect();
        assert_eq!(kept, [(sampled, false), (real, true), (replayed, true)]);
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
