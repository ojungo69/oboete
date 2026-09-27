//! `oboete inject` is where OpenCode reads its context: a config.toml that stops recording must
//! still reach it as the recording-failure line, not as an empty context.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn inject_reports_the_failure_a_broken_config_causes() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = 5\n",
    )
    .unwrap();
    let oboete = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oboete"));
        cmd.arg("--home")
            .arg(home.path())
            .current_dir(cwd.path())
            .env("OBOETE_NO_SPAWN", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    };
    let mut hook = oboete()
        .args(["hook", "opencode", "SessionStart"])
        .spawn()
        .unwrap();
    let start = format!(
        r#"{{"session_id":"s","cwd":{},"source":"startup"}}"#,
        serde_json::to_string(&cwd.path()).unwrap()
    );
    hook.stdin
        .take()
        .unwrap()
        .write_all(start.as_bytes())
        .unwrap();
    assert!(hook.wait_with_output().unwrap().status.success());
    let out = oboete().args(["inject", "--session=s"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("recording has failed since"), "{text:?}");
}
