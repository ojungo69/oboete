//! D9: nothing `oboete replay` records says the owner is at work, including the `oboete hook`
//! processes it spawns to time them.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

#[test]
fn a_replay_and_its_spawned_hooks_record_no_hook_record() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = serde_json::json!({"agent": "claude", "event": "UserPromptSubmit",
        "ts": "2026-09-01T00:00:00.000Z",
        "payload": {"session_id": "s", "cwd": "__OBOETE_REPLAY_ROOT__", "prompt": "one"}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "replay"])
        .arg(&fixture)
        .args(["--spawn-sample", "2"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
    let count = |sql: &str| -> i64 { raw.query_row(sql, [], |r| r.get(0)).unwrap() };
    // The fixture's prompt and the two spawned samples.
    assert_eq!(
        count("SELECT COUNT(*) FROM records WHERE source = 'replay'"),
        3
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM records WHERE source = 'hook'"),
        0
    );
}

/// Milestone 4 Task 0 (OpenCodeReview on #301): `--read-sample` runs both arms through the hooks
/// it spawns, as `run` wires them: the cold arm shows nothing for the checkout, and the warm arm,
/// after the drain, shows its manifest from the spawned SessionStart and from the in-process read.
/// With no write samples, the samples' checkout has nothing to show, so a SessionStart sent there
/// prints nothing.
#[test]
fn the_read_arms_run_through_the_spawned_hooks() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = serde_json::json!({"agent": "claude", "event": "UserPromptSubmit",
        "ts": "2026-09-01T00:00:00.000Z",
        "payload": {"session_id": "s", "cwd": "__OBOETE_REPLAY_ROOT__",
                    "prompt": "Fix the flaky test first."}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "replay"])
        .arg(&fixture)
        .args(["--read-sample", "2", "--spawn-sample", "0"])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let (cold, warm) = (&report["read"]["cold"], &report["read"]["warm"]);
    assert_eq!(
        (&cold["read_chars"], &cold["session_start_printed_bytes"]),
        (&0.into(), &0.into())
    );
    assert!(warm["read_chars"].as_u64().unwrap() > 0);
    assert!(warm["session_start_printed_bytes"].as_u64().unwrap() > 0);
    assert_eq!(warm["session_start_ms"]["n"], 2);
}

/// Milestone 4 D15: `--read-warmup W` runs W spawns of each read hook before the N it times, and
/// reports only the N. Every spawn records one event under the sample session, warm-ups too.
#[test]
fn a_read_sample_times_only_the_spawns_after_its_warm_up() {
    let home = tempfile::tempdir().unwrap();
    let fixture = home.path().join("f.jsonl");
    let line = serde_json::json!({"agent": "claude", "event": "UserPromptSubmit",
        "ts": "2026-09-01T00:00:00.000Z",
        "payload": {"session_id": "s", "cwd": "__OBOETE_REPLAY_ROOT__", "prompt": "one"}});
    std::fs::write(&fixture, line.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .args(["--home", &home.path().to_string_lossy(), "replay"])
        .arg(&fixture)
        .args([
            "--read-sample",
            "2",
            "--read-warmup",
            "3",
            "--spawn-sample",
            "0",
        ])
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for arm in ["cold", "warm"] {
        for hook in ["session_start_ms", "prompt_ms"] {
            assert_eq!(report["read"][arm][hook]["n"], 2, "{arm} {hook}");
        }
    }
    let raw = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
    let spawned: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM records WHERE type = 'event' AND session LIKE 'read-sample%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Two arms, two hooks, three warm-ups and two timed spawns each.
    assert_eq!(spawned, 2 * 2 * (3 + 2));
    let sessions: i64 = raw
        .query_row(
            "SELECT COUNT(DISTINCT session) FROM records
         WHERE type = 'event' AND session LIKE 'read-sample%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        sessions, spawned,
        "each read spawn needs an independent shown set"
    );
}

/// A loopback HTTP endpoint on 127.0.0.1: each request counted, and answered with
/// `answer(request body)`'s status and JSON body.
fn stub(answer: fn(&str) -> (u16, String)) -> (u16, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            counted.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let (status, reply) = answer(&String::from_utf8_lossy(&body));
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (port, hits)
}

const DECISION: &str = "We keep the parser errors on stderr.";

/// The curator's answer: the decision the seed prompt states, quoted from its line; a digest
/// request gets no lines (as docs/eval/m3.py's `Stub` answers).
fn curator(body: &str) -> (u16, String) {
    let request: Value = serde_json::from_str(body).unwrap_or_default();
    let content = if request["response_format"].to_string().contains("\"lines\"") {
        json!({"lines": []})
    } else {
        let text: Vec<&str> = request["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["content"].as_str())
            .collect();
        let line = text
            .iter()
            .flat_map(|t| t.lines())
            .find(|l| l.contains(DECISION))
            .and_then(|l| l.split(' ').next())
            .unwrap_or("L1");
        json!({"claims": [{"id": "c1", "kind": "decision", "status": "decided",
            "speaker": "user", "scope": "repo", "body": DECISION, "quote": DECISION,
            "line": line, "supersedes": [], "why": "stated"}], "summary": "s"})
    };
    let reply = json!({"id": "stub", "object": "chat.completion", "model": "stub",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": content.to_string()}}]});
    (200, reply.to_string())
}

/// `oboete <args>` on `home` with no worker started by a hook; its stdout, after it succeeded.
fn oboete(home: &Path, args: &[&str]) -> Vec<u8> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("OBOETE_NO_SPAWN", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// `oboete worker` on `home` until `done` holds (checked every 100 ms, 60 s at most), then
/// stopped: a worker stays up while a window or a digest waits out a failure's hold, which a
/// stub's empty or failed answer starts.
fn worker_until(home: &Path, done: impl Fn() -> bool) {
    let mut worker = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(["worker", "--idle-ms", "0"])
        .env("OBOETE_NO_SPAWN", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = worker.kill();
    worker.wait().unwrap();
    assert!(done(), "the worker did not get there in 60 s");
}

/// One prompt in the replay's checkout, as a fixture.
fn prompt_fixture(home: &Path, name: &str, session: &str, ts: &str, prompt: &str) -> PathBuf {
    let path = home.join(name);
    let line = json!({"agent": "claude", "event": "UserPromptSubmit", "ts": ts,
        "payload": {"session_id": session, "cwd": "__OBOETE_REPLAY_ROOT__", "prompt": prompt}});
    std::fs::write(&path, line.to_string()).unwrap();
    path
}

/// A home whose checkout (branch `main`) holds one delivered decision, which `oboete worker`
/// curated through the stub curator from a replayed prompt, with per-prompt injection on.
fn claimed_home() -> (tempfile::TempDir, PathBuf) {
    let (port, _) = stub(curator);
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let config = format!(
        "[summary]\ncurate = true\n\n[inject]\nper_prompt = true\n\n[[providers]]\n\
         kind = \"openai\"\nname = \"stub\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\n\
         model = \"stub\"\ndaily_budget = 1000\n"
    );
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let seed = prompt_fixture(
        home.path(),
        "seed.jsonl",
        "seed",
        "2026-09-01T00:00:00.000Z",
        DECISION,
    );
    let root_arg = root.to_string_lossy().into_owned();
    oboete(
        home.path(),
        &[
            "replay",
            &seed.to_string_lossy(),
            "--repo-root",
            &root_arg,
            "--spawn-sample",
            "0",
        ],
    );
    let knowledge = home.path().join("knowledge.db");
    worker_until(home.path(), || {
        rusqlite::Connection::open(&knowledge)
            .and_then(|k| {
                k.query_row(
                    "SELECT COUNT(*) FROM active WHERE kind = 'decision'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .is_ok_and(|n| n == 1)
    });
    (home, root)
}

/// The read sample on a claimed home: two timed spawns per hook after one warm-up.
fn read_sample(home: &Path, root: &Path) -> Value {
    let fixture = prompt_fixture(
        home,
        "f.jsonl",
        "f",
        "2026-09-02T00:00:00.000Z",
        "Fix the flaky test first.",
    );
    let out = oboete(
        home,
        &[
            "replay",
            &fixture.to_string_lossy(),
            "--repo-root",
            &root.to_string_lossy(),
            "--read-sample",
            "2",
            "--read-warmup",
            "1",
            "--spawn-sample",
            "0",
        ],
    );
    serde_json::from_slice(&out).unwrap()
}

/// D9, D15: with `per_prompt` on and a delivered claim in the fixture's checkout, every timed
/// warm prompt hook prints it, from the shortlist the arm built for the sample session's key.
#[test]
fn the_warm_prompt_hook_prints_the_shortlisted_claim() {
    let (home, root) = claimed_home();
    let report = read_sample(home.path(), &root);
    let warm = &report["read"]["warm"];
    assert_eq!(warm["prompt_ms"]["n"], 2, "{report}");
    // The fewest bytes a timed spawn printed: none printed nothing.
    assert!(
        warm["prompt_printed_bytes"].as_u64().unwrap() > 0,
        "{report}"
    );
    assert_eq!(report["read"]["per_prompt"], true, "{report}");
    let k = rusqlite::Connection::open(home.path().join("knowledge.db")).unwrap();
    let rows: i64 = k
        .query_row(
            "SELECT COUNT(*) FROM shortlist s JOIN active a ON a.uid = s.uid
             WHERE s.session LIKE '%-warm-UserPromptSubmit-%' AND a.kind = 'decision'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 3,
        "warm-up and both timed prompts use the prepared shortlist"
    );
    let builds: (i64, i64) = k
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT built_seq) FROM shortlists
         WHERE session LIKE '%-warm-UserPromptSubmit-%'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        builds,
        (3, 1),
        "every warm prompt has the same prepared build"
    );
}

/// D15: both arms' prompts go to the fixture's checkout, so the cold arm's prompt hook picks
/// from its claims, as a hook does before the worker is up (`delivered_ranked`).
#[test]
fn the_cold_prompt_arm_reads_the_fixtures_checkout() {
    let (home, root) = claimed_home();
    let report = read_sample(home.path(), &root);
    let cold = &report["read"]["cold"];
    assert!(
        cold["prompt_printed_bytes"].as_u64().unwrap() > 0,
        "{report}"
    );
}

/// D17: the drain between the arms is timed, with the records it had before it: the fixture's
/// prompt, and a start and a prompt for each cold spawn, its warm-up included.
#[test]
fn the_drain_between_the_arms_is_timed_with_its_backlog() {
    let (home, root) = claimed_home();
    let report = read_sample(home.path(), &root);
    let drain = &report["read"]["drain"];
    assert_eq!(drain["backlog"], 1 + 2 * (1 + 2), "{report}");
    assert!(drain["ms"].as_f64().is_some(), "{report}");
}

/// Spec 4.1: the hooks a read sample spawns send nothing, with a curator and an embedder that
/// would answer on 127.0.0.1; the worker started after reaches them (the control).
#[test]
fn a_hook_sample_sends_nothing_to_a_listening_stub() {
    let (port, hits) = stub(|_| (500, "{}".into()));
    let home = tempfile::tempdir().unwrap();
    let key = home.path().join("EMBED_KEY.md");
    std::fs::write(&key, format!("key\n{}\n", "x".repeat(40))).unwrap();
    let config = format!(
        "[summary]\ncurate = true\n\n[inject]\nper_prompt = true\n\n[embedding]\n\
         provider = \"workers-ai\"\naccount_id = \"a\"\nkey_file = '{}'\n\
         url = \"http://127.0.0.1:{port}\"\n\n[[providers]]\nkind = \"openai\"\nname = \"stub\"\n\
         base_url = \"http://127.0.0.1:{port}/v1\"\nmodel = \"stub\"\ndaily_budget = 1000\n",
        key.display()
    );
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let fixture = prompt_fixture(
        home.path(),
        "f.jsonl",
        "s",
        "2026-09-01T00:00:00.000Z",
        "We decided to keep the parser errors on stderr.",
    );
    oboete(
        home.path(),
        &[
            "replay",
            &fixture.to_string_lossy(),
            "--read-sample",
            "2",
            "--read-warmup",
            "1",
            "--spawn-sample",
            "2",
        ],
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    worker_until(home.path(), || hits.load(Ordering::SeqCst) > 0);
}
