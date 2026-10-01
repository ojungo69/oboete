//! `oboete search` through the binary, in a home that holds Design B's store (review on #106).

use std::io::Write;
use std::process::{Command, Stdio};

fn oboete(home: &std::path::Path, cwd: &std::path::Path, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        .current_dir(cwd)
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// `timeline` reads Design B since milestone 4's Task 4: it creates no v1 store beside raw.db.
#[test]
fn raw_hits_stay_found_after_timeline() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    let payload = serde_json::json!({
        "session_id": "s",
        "prompt": "zebra crossing notes",
        "cwd": c,
    });
    oboete(
        h,
        c,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    assert!(oboete(h, c, &["search", "zebra"], "").contains("zebra crossing"));
    oboete(h, c, &["timeline"], "");
    assert!(!h.join("oboete.db").exists());
    assert!(oboete(h, c, &["search", "zebra"], "").contains("zebra crossing"));
}

#[test]
fn a_raw_hit_id_from_search_opens_with_get() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    let payload = serde_json::json!({"session_id": "s", "prompt": "zebra plan", "cwd": c});
    oboete(
        h,
        c,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    let hit = oboete(h, c, &["search", "zebra"], "");
    let id = hit.split_whitespace().next().unwrap().to_owned();
    let got = oboete(h, c, &["get", &id], "");
    assert!(
        got.starts_with(&id) && got.contains("zebra plan"),
        "{hit}\n{got}"
    );
    assert!(!h.join("oboete.db").exists()); // a raw-only home stays one
}

#[test]
fn the_limit_holds_across_agents() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    for agent in ["claude", "grok"] {
        // Both agents' hooks write raw.db.
        for i in 0..3 {
            let payload = serde_json::json!({
                "session_id": format!("{agent}-{i}"),
                "prompt": format!("zebra note {agent} {i}"),
                "cwd": c,
            });
            oboete(
                h,
                c,
                &["hook", agent, "UserPromptSubmit"],
                &payload.to_string(),
            );
        }
    }
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    let all = oboete(h, c, &["search", "zebra"], "");
    assert!(
        all.contains("zebra note claude") && all.contains("zebra note grok"),
        "{all}"
    );
    let four = oboete(h, c, &["search", "--limit", "4", "zebra"], "");
    assert_eq!(
        four.lines().filter(|l| l.contains("zebra note")).count(),
        4,
        "{four}"
    );
}

#[test]
fn a_rule_added_after_capture_hides_its_value_before_the_rescan_runs() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    let payload = serde_json::json!({"session_id": "s", "prompt": "zebra acme-123456", "cwd": c});
    oboete(
        h,
        c,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    std::fs::write(
        h.join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    // No worker has run since: the value is only in raw and the index, not yet tombstoned.
    let hit = oboete(h, c, &["search", "zebra"], "");
    let id = hit.split_whitespace().next().unwrap().to_owned();
    let got = oboete(h, c, &["get", &id], "");
    assert!(
        hit.contains("zebra") && !hit.contains("123456") && !got.contains("123456"),
        "{hit}\n{got}"
    );
}

#[test]
fn get_applies_a_rule_anchored_to_a_field_before_the_rescan_runs() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // The repo label (a checkout's path) ends in a value the rule covers too.
    let c = &cwd.path().join("app-acme-777777");
    std::fs::create_dir(c).unwrap();
    let h = home.path();
    let payload = serde_json::json!({"session_id": "s", "prompt": "zebra acme-654321", "cwd": c});
    oboete(
        h,
        c,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    let id = oboete(h, c, &["search", "zebra"], "")
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    std::fs::write(
        h.join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}$' }]\n",
    )
    .unwrap();
    let got = oboete(h, c, &["get", &id], "");
    assert!(
        got.contains("zebra") && !got.contains("654321") && !got.contains("777777"),
        "{got}"
    );
}

#[test]
fn search_applies_a_rule_anchored_to_a_field_to_every_hit() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    for (s, v) in [("s1", "111111"), ("s2", "222222")] {
        let payload =
            serde_json::json!({"session_id": s, "prompt": format!("zebra acme-{v}"), "cwd": c});
        oboete(
            h,
            c,
            &["hook", "claude", "UserPromptSubmit"],
            &payload.to_string(),
        );
    }
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    std::fs::write(
        h.join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}$' }]\n",
    )
    .unwrap();
    let hits = oboete(h, c, &["search", "zebra"], "");
    assert!(
        hits.lines().count() == 2 && !hits.contains("111111") && !hits.contains("222222"),
        "{hits}"
    );
}

/// A loopback stand-in for Workers AI's bge-m3: every text gets the same unit vector.
fn embedder() -> String {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/run/bge-m3", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            std::thread::spawn(move || {
                let (mut req, mut buf) = (Vec::new(), [0u8; 65536]);
                let body = loop {
                    let n = conn.read(&mut buf).unwrap_or(0);
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= end + 4 + len {
                            break req[end + 4..end + 4 + len].to_vec();
                        }
                    }
                    if n == 0 {
                        return;
                    }
                };
                let texts: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let n = texts["text"].as_array().unwrap().len();
                let mut v = vec![0.0f32; 1024];
                v[0] = 1.0;
                let out = serde_json::json!({
                    "result": {"shape": [n, 1024], "data": vec![v; n]},
                    "success": true
                })
                .to_string();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    out.len()
                );
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(out.as_bytes());
            });
        }
    });
    url
}

/// Milestone 4 Task 6: `oboete eval` runs on Design B's stores through the binary and creates no
/// v1 store (spec 7.4); its vectors come from `[embedding] url` at a loopback address (D8).
#[test]
fn an_eval_on_a_design_b_home_creates_no_oboete_db() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    let key = h.join("key.md");
    std::fs::write(&key, "workers ai\nk\n").unwrap();
    let config = format!(
        "[embedding]\nprovider = \"workers-ai\"\naccount_id = \"a\"\nkey_file = '{}'\nurl = \"{}\"\n",
        key.display(),
        embedder()
    );
    std::fs::write(h.join("config.toml"), config).unwrap();
    let payload =
        serde_json::json!({"session_id": "s", "prompt": "zebra crossing notes", "cwd": c});
    oboete(
        h,
        c,
        &["hook", "claude", "UserPromptSubmit"],
        &payload.to_string(),
    );
    oboete(h, c, &["worker", "--idle-ms", "0"], "");
    let questions = c.join("questions.jsonl");
    std::fs::write(
        &questions,
        "{\"qid\":\"q1\",\"text\":\"zebra\",\"session\":\"other\"}\n",
    )
    .unwrap();
    let out = c.join("runs");
    let args = [
        "eval",
        questions.to_str().unwrap(),
        "--depth",
        "10",
        "--arms",
        "off,rrf:5,only",
        "--out",
        out.to_str().unwrap(),
    ];
    oboete(h, c, &args, "");
    assert!(!h.join("oboete.db").exists());
    let run = std::fs::read_to_string(out.join("b-only.trec")).unwrap();
    assert!(run.starts_with("q1 Q0 r:"), "{run}");
    assert!(out.join("b-rrf5.trec").exists() && out.join("b-docs.jsonl").exists());
}
