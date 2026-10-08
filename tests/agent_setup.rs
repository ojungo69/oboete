//! One isolated, directly supervised resident-viewer contract test for W6 setup.
//! Linux only: this does not claim Windows or real-agent verification.
#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const AGENTS: [&str; 7] = ["claude", "codex", "grok", "agy", "opencode", "pi", "cursor"];
const SAVED_CANARY: &str = "PrivateSavedClaudeArgumentDoNotExpose";
const OUTPUT_CANARY: &str = "PrivateClaudeStubOutputDoNotExpose";

struct Viewer(Child);
impl Drop for Viewer {
    fn drop(&mut self) {
        // Only the child created and retained by this test can be signaled.
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn fixture(root: &Path) -> (PathBuf, PathBuf) {
    for relative in [
        "store/state",
        "owner/claude",
        "owner/codex",
        "owner/grok/hooks",
        "owner/.gemini/config",
        "owner/opencode/plugins",
        "owner/pi/extensions",
        "owner/cursor",
        "xdg/config",
        "xdg/cache",
        "xdg/data",
        "xdg/state",
        "tmp",
        "cwd",
        "bin",
    ] {
        fs::create_dir_all(root.join(relative)).unwrap();
    }
    write(
        root,
        "owner/claude/settings.json",
        br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"foreign-hook"}]}]}}"#,
    );
    write(
        root,
        "owner/claude/.claude.json",
        br#"{"mcpServers":{"foreign":{"command":"foreign","args":[]}}}"#,
    );
    write(
        root,
        "owner/codex/hooks.json",
        br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"foreign-hook"}]}]}}"#,
    );
    write(
        root,
        "owner/codex/config.toml",
        b"[mcp_servers.foreign]\ncommand='foreign'\nargs=[]\n",
    );
    write(root, "owner/grok/hooks/oboete.json", b"{}\n");
    write(
        root,
        "owner/grok/config.toml",
        b"[mcp_servers.foreign]\ncommand='foreign'\nargs=[]\n",
    );
    write(root, "owner/.gemini/config/hooks.json", b"{}\n");
    write(
        root,
        "owner/.gemini/config/mcp_config.json",
        br#"{"mcpServers":{"foreign":{"command":"foreign","args":[]}}}"#,
    );
    write(
        root,
        "owner/opencode/opencode.json",
        br#"{"mcp":{"servers":{"foreign":{"type":"local","command":["foreign"]}}}}"#,
    );
    write(
        root,
        "owner/cursor/hooks.json",
        br#"{"version":1,"hooks":{"sessionStart":[{"command":"foreign-hook","timeout":10}]}}"#,
    );
    write(
        root,
        "owner/cursor/mcp.json",
        br#"{"mcpServers":{"foreign":{"command":"foreign","args":[]}}}"#,
    );

    // The only agent launcher allowed to execute is this fixed shell program. It accepts exactly the
    // generated add-json or fixed remove form, and writes only the private Claude fixture.
    let stub = root.join("bin/claude");
    fs::write(&stub, format!(r#"#!/bin/sh
set -eu
[ "${{OBOETE_SKIP:-}}" = 1 ] || exit 97
if [ "$#" -eq 6 ] && [ "$1" = mcp ] && [ "$2" = add-json ] &&
   [ "$3" = --scope ] && [ "$4" = user ] && [ "$5" = oboete ]; then
    printf 'add\n' >> "$CLAUDE_CONFIG_DIR/../../stub.log"
    printf '{{"mcpServers":{{"foreign":{{"command":"foreign","args":[]}},"oboete":%s}}}}\n' "$6" > "$CLAUDE_CONFIG_DIR/.claude.json"
elif [ "$#" -eq 5 ] && [ "$1" = mcp ] && [ "$2" = remove ] &&
     [ "$3" = --scope ] && [ "$4" = user ] && [ "$5" = oboete ]; then
    printf 'remove\n' >> "$CLAUDE_CONFIG_DIR/../../stub.log"
    printf '{{"mcpServers":{{"foreign":{{"command":"foreign","args":[]}}}}}}\n' > "$CLAUDE_CONFIG_DIR/.claude.json"
else
    exit 97
fi
printf '{OUTPUT_CANARY}\n' >&2
"#)).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700)).unwrap();
    // Inventory may inspect these paths, but setup must never execute their CLIs.
    // df is a sentinel for accidental subprocess diagnostics in this same private PATH.
    for name in [
        "codex",
        "grok",
        "agy",
        "opencode",
        "pi",
        "cursor-agent",
        "agent",
        "df",
    ] {
        let launcher = root.join("bin").join(name);
        fs::write(
            &launcher,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{name}' >> \"$HOME/../sentinel.marker\"\nexit 97\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    }
    (stub, root.join("stub.log"))
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_viewer(root: &Path, port: u16) -> Viewer {
    let owner = root.join("owner");
    let mut command = Command::new(env!("CARGO_BIN_EXE_oboete"));
    command
        .arg("--home")
        .arg(root.join("store"))
        .args(["view", "--resident"])
        .env_clear()
        .env("HOME", &owner)
        .env("USERPROFILE", &owner)
        .env("OBOETE_HOME", root.join("store"))
        .env("OBOETE_NO_SPAWN", "1")
        .env("OBOETE_SKIP", "1")
        .env("CLAUDE_CONFIG_DIR", owner.join("claude"))
        .env("CODEX_HOME", owner.join("codex"))
        .env("GROK_HOME", owner.join("grok"))
        .env("OPENCODE_CONFIG_DIR", owner.join("opencode"))
        .env("PI_CODING_AGENT_DIR", owner.join("pi"))
        .env("CURSOR_CONFIG_DIR", owner.join("cursor"))
        .env("XDG_CONFIG_HOME", root.join("xdg/config"))
        .env("XDG_CACHE_HOME", root.join("xdg/cache"))
        .env("XDG_DATA_HOME", root.join("xdg/data"))
        .env("XDG_STATE_HOME", root.join("xdg/state"))
        .env("TMPDIR", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env("PATH", root.join("bin"))
        .current_dir(root.join("cwd"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Keep the one instrumentation output variable when llvm-cov launches the test.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        let profile = PathBuf::from(profile);
        let profile = if profile.as_os_str().is_empty() || profile.is_absolute() {
            profile
        } else {
            std::env::current_dir().unwrap().join(profile)
        };
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let mut viewer = Viewer(command.spawn().unwrap());
    let outcome = root.join("store/state/view-outcome");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            viewer.0.try_wait().unwrap().is_none(),
            "private viewer exited"
        );
        match fs::read_to_string(&outcome) {
            Ok(text) if text.trim() == format!("listening {port}") => break,
            Ok(text) if text.trim() == "port in use" => panic!("private port lost before bind"),
            _ => assert!(Instant::now() < deadline, "private viewer did not bind"),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    viewer
}

fn request(root: &Path, port: u16, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let token = fs::read_to_string(root.join("store/state/view-token")).unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let payload = body
        .map(serde_json::to_vec)
        .transpose()
        .unwrap()
        .unwrap_or_default();
    let headers = if body.is_some() {
        format!(
            "Origin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        )
    } else {
        String::new()
    };
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: {token}\r\n{headers}Connection: close\r\n\r\n").unwrap();
    stream.write_all(&payload).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    assert!(
        !root.join("sentinel.marker").exists(),
        "an agent or df launcher ran"
    );
    assert!(raw.len() < 1_000_000);
    let text = String::from_utf8(raw).unwrap();
    for secret in [SAVED_CANARY, OUTPUT_CANARY, &token, root.to_str().unwrap()] {
        assert!(!text.contains(secret), "private data in setup response");
    }
    let (head, data) = text.split_once("\r\n\r\n").unwrap();
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, serde_json::from_str(data).unwrap())
}

fn preview(root: &Path, port: u16, action: &str, agents: &[&str]) -> Value {
    let (status, value) = request(
        root,
        port,
        "POST",
        "/api/setup/preview",
        Some(&json!({"action":action,"agents":agents})),
    );
    assert_eq!(status, 200);
    assert_eq!(value["action"], action);
    assert_eq!(value["agents"].as_array().unwrap().len(), agents.len());
    assert_eq!(value["live_verified"], false);
    value
}

fn start(root: &Path, port: u16, shown: &Value, agents: &[&str], id: &str) -> (u16, Value) {
    request(
        root,
        port,
        "POST",
        "/api/setup/start",
        Some(&json!({
            "action":shown["action"],"agents":agents,"preview_key":shown["preview_key"],
            "operation_id":id,"confirmed":true
        })),
    )
}

fn step<'a>(receipt: &'a Value, agent: &str, component: &str) -> &'a Value {
    receipt["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["agent"] == agent)
        .unwrap()["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["component"] == component)
        .unwrap()
}

#[test]
fn seven_agent_preview_apply_replay_stale_manual_and_unwire() {
    let private = tempfile::tempdir().unwrap();
    let root = private.path();
    let (_stub, log) = fixture(root);
    let port = free_port();
    let config = root.join("store/config.toml");
    write(
        root,
        "store/config.toml",
        format!(
            "providers = []\n[summary]\ncurate = false\n[embedding]\nprovider = 'none'\n[worker]\nresident = true\n[view]\nport = {port}\n"
        ),
    );
    let mut viewer = start_viewer(root, port);

    let observed = [
        "owner/claude/settings.json",
        "owner/claude/.claude.json",
        "owner/codex/hooks.json",
        "owner/codex/config.toml",
        "owner/grok/config.toml",
        "owner/.gemini/config/mcp_config.json",
        "owner/opencode/opencode.json",
        "owner/pi/extensions/oboete.ts",
        "owner/cursor/mcp.json",
    ];
    let before: Vec<_> = observed
        .iter()
        .map(|path| fs::read(root.join(path)).ok())
        .collect();
    let first = preview(root, port, "wire", &AGENTS);
    assert_eq!(first["activation"], "next_session");
    assert_eq!(
        before,
        observed
            .iter()
            .map(|path| fs::read(root.join(path)).ok())
            .collect::<Vec<_>>()
    );
    assert!(!log.exists(), "preview ran an agent launcher");
    assert!(
        !root.join("sentinel.marker").exists(),
        "an agent or df launcher ran"
    );

    let id = "a".repeat(64);
    let (status, wired) = start(root, port, &first, &AGENTS, &id);
    assert_eq!(status, 200);
    assert_eq!(wired["phase"], "complete");
    assert_eq!(wired["activation"], "next_session");
    assert_eq!(fs::read_to_string(&log).unwrap(), "add\n");
    assert!(root.join("owner/pi/extensions/oboete.ts").is_file());
    let (status, replay) = start(root, port, &first, &AGENTS, &id);
    assert_eq!((status, replay), (200, wired.clone()));
    assert_eq!(fs::read_to_string(&log).unwrap(), "add\n");

    let noop = preview(root, port, "wire", &AGENTS);
    let (status, nooped) = start(root, port, &noop, &AGENTS, &"b".repeat(64));
    assert_eq!(status, 200);
    assert_eq!(nooped["phase"], "complete");
    assert_eq!(fs::read_to_string(&log).unwrap(), "add\n");

    let stale = preview(root, port, "wire", &AGENTS);
    let codex = root.join("owner/codex/config.toml");
    let mut external = fs::read(&codex).unwrap();
    external.extend_from_slice(b"\n# external edit\n");
    fs::write(&codex, &external).unwrap();
    let (status, refusal) = start(root, port, &stale, &AGENTS, &"c".repeat(64));
    assert_eq!(status, 409);
    assert_eq!(refusal, json!({"code":"setup_stale","field":""}));
    assert_eq!(fs::read(&codex).unwrap(), external);
    assert_eq!(fs::read_to_string(&log).unwrap(), "add\n");

    let claude = root.join("owner/claude/.claude.json");
    let existing = json!({"mcpServers":{"foreign":{"command":"foreign","args":[]},
        "oboete":{"type":"stdio","command":"foreign-old",
                  "args":[SAVED_CANARY],"env":{"PRIVATE":SAVED_CANARY}}}});
    fs::write(&claude, serde_json::to_vec(&existing).unwrap()).unwrap();
    let manual = preview(root, port, "wire", &["claude"]);
    assert_eq!(step(&manual, "claude", "mcp")["effect"], "manual");
    let (status, held) = start(root, port, &manual, &["claude"], &"d".repeat(64));
    assert_eq!(status, 200);
    assert_eq!(held["phase"], "partial");
    assert_eq!(
        fs::read(&claude).unwrap(),
        serde_json::to_vec(&existing).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "add\n",
        "manual wire started Claude"
    );

    let alias = root.join("owner/pi/extensions/oboete.ts");
    let intermediate = root.join("owner/pi/extensions/owned-intermediate.ts");
    let referent = root.join("owner/pi/extensions/owned-extension.ts");
    fs::rename(&alias, &referent).unwrap();
    symlink(&referent, &intermediate).unwrap();
    symlink(&intermediate, &alias).unwrap();
    let retained = fs::read(&referent).unwrap();
    let remove = preview(root, port, "unwire", &AGENTS);
    let (status, removed) = start(root, port, &remove, &AGENTS, &"e".repeat(64));
    assert_eq!(status, 200);
    assert_eq!(removed["phase"], "complete");
    assert!(
        alias.symlink_metadata().is_err(),
        "owned alias survived unwire"
    );
    assert_eq!(fs::read_link(&intermediate).unwrap(), referent);
    assert_eq!(fs::read(&referent).unwrap(), retained);
    let final_claude: Value = serde_json::from_slice(&fs::read(&claude).unwrap()).unwrap();
    assert!(final_claude["mcpServers"].get("oboete").is_none());
    assert_eq!(final_claude["mcpServers"]["foreign"]["command"], "foreign");
    assert_eq!(fs::read_to_string(&log).unwrap(), "add\nremove\n");
    let (status, operation) = request(root, port, "GET", "/api/setup/operation", None);
    assert_eq!(status, 200);
    assert_eq!(operation["active"], Value::Null);
    assert_eq!(operation["last"], removed);
    assert!(
        !root.join("sentinel.marker").exists(),
        "an agent or df launcher ran"
    );

    // A changed port permits leaving at the next minute check despite the final GET;
    // resident=false alone would need one more quiet interval. Normal exit flushes coverage.
    let current = fs::read_to_string(&config).unwrap();
    assert!(current.contains("resident = true"));
    let next_port = free_port();
    assert_ne!(next_port, port);
    let changed = current
        .replace("resident = true", "resident = false")
        .replace(&format!("port = {port}"), &format!("port = {next_port}"));
    fs::write(&config, changed).unwrap();
    let deadline = Instant::now() + Duration::from_secs(70);
    loop {
        if let Some(status) = viewer.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "private resident viewer exited unsuccessfully"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "private resident viewer did not leave"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
