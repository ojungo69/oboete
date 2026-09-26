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

#[test]
fn raw_hits_stay_found_after_a_v1_command_creates_oboete_db() {
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
    oboete(h, c, &["timeline"], ""); // opens, and so creates, v1's oboete.db
    assert!(h.join("oboete.db").exists());
    assert!(oboete(h, c, &["search", "zebra"], "").contains("zebra crossing"));
}

#[test]
fn the_limit_holds_across_both_stores() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (h, c) = (home.path(), cwd.path());
    for agent in ["claude", "grok"] {
        // Claude Code writes raw.db; Grok, not yet ported (Task 2b), writes v1's oboete.db.
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
