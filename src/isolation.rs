//! The isolation gate (spec 6.5, docs/milestone-3-plan.md Task 3): a curator CLI reads hostile
//! text, so it runs only when it provably cannot act. The gate tests capability, never obedience:
//! a model that declines a planted instruction proves nothing (agy declined one holding 57 tools).
//! - claude reports its tools in every call's init event, which `provider::claude_stream` checks.
//! - codex reports none, so it is probed with no model before each call (about 3.4 s): its
//!   permission profile directly (`codex sandbox`), then its own `codex exec` against a scripted
//!   model (`codex_probe`), which tries a write, a read and a fetch through the exec tool, in the
//!   sandbox and escalated, in the root and in a sub-agent. Each must be refused. The hosted tools
//!   the profile does not govern must be off, and codex must still read `web_search`. A feature on
//!   that was not on when the gate was proven (`provider::CODEX_ON`), or a tool offered to the
//!   model that the gate has not reviewed (`codex_probe::OFFERED`), fails it. Probing each
//!   time, not once per version, follows whatever else changes what codex resolves: its managed
//!   requirements (from /etc or a workspace's cloud bundle), the user's config, an MDM profile.
//!   `--ignore-user-config` is not probed (the MCP servers of the owner's config;
//!   docs/spike/curator-isolation.md shows it drops them).
//! - Any other CLI (agy, grok) has no proven no-tool mode and is skipped (spec 6.5, R05).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::provider::{
    CODEX_OFF, CODEX_ON, CODEX_PROFILE, curator_env, kill_tree, own_group, scratch_dir,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Passed,
    Failed(String),
    NotProven,
}

impl Gate {
    pub fn why(&self) -> String {
        match self {
            Gate::Passed => String::new(),
            Gate::Failed(why) => format!("isolation: {why}"),
            Gate::NotProven => "isolation: this CLI has no proven no-tool mode".into(),
        }
    }
}

/// The gate for `cli`; codex is probed each time, and its last result is kept for doctor.
pub fn gate(db: &Connection, cli: &str) -> Result<Gate> {
    match cli {
        "claude" => Ok(Gate::Passed),
        "codex" => gate_codex(db, Path::new("codex"), &crate::config::home_dir()),
        _ => Ok(Gate::NotProven),
    }
}

/// Whether `cli` has a gate that can pass: any other is always skipped as a curator.
pub fn provable(cli: &str) -> bool {
    matches!(cli, "claude" | "codex")
}

fn gate_codex(db: &Connection, exe: &Path, home: &Path) -> Result<Gate> {
    // The curator's own kind of working directory: private, fresh, removed after. A failure before
    // the probe is kept too, so doctor never shows an older pass as the current state.
    let unknown = || "unknown version".to_owned();
    let (version, result) = match scratch_dir() {
        Err(_) => (unknown(), Err("no scratch directory for the probe".into())),
        Ok(scratch) => match version(exe, scratch.0.as_path()) {
            None => (unknown(), Err("codex --version did not answer".into())),
            Some(v) => (v, probe(exe, home, scratch.0.as_path())),
        },
    };
    db.execute(
        "INSERT OR REPLACE INTO isolation(cli, version, passed, detail, ts) VALUES(?1,?2,?3,?4,?5)",
        params![
            "codex",
            version,
            result.is_ok(),
            result.as_ref().err().cloned().unwrap_or_default(),
            crate::db::now_ms()
        ],
    )?;
    Ok(result.map_or_else(Gate::Failed, |()| Gate::Passed))
}

/// A command under the curator's environment and in a working directory like its own, as
/// `provider::cli_headless` runs it: a probe under another HOME, CODEX_HOME or PATH, or in a shared
/// directory another user could plant a `.codex` in, would prove nothing about the curator.
fn command(exe: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env_clear()
        .envs(curator_env(std::env::vars_os(), cfg!(windows)))
        .env(crate::hook::SKIP_ENV, "1")
        .current_dir(cwd)
        .stdin(Stdio::null());
    cmd
}

/// How long one probe command may take: a codex that stalls fails the gate, and the chain goes on
/// to the next provider.
const PROBE_LIMIT: std::time::Duration =
    std::time::Duration::from_secs(if cfg!(test) { 5 } else { 30 });

/// Most of a probe command's output kept: a codex that floods a pipe must not fill memory.
const PROBE_OUTPUT: u64 = 1 << 20;

/// `cmd`'s output, or an error when it did not finish within `PROBE_LIMIT` (it is killed, with
/// its process group).
fn output(cmd: &mut Command) -> Result<std::process::Output, String> {
    use std::io::Read;
    let mut child = own_group(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Each pipe drains on its own thread, up to the cap and then to its end, so the child never
    // blocks on a full pipe.
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.by_ref().take(PROBE_OUTPUT).read_to_end(&mut buf);
                let _ = std::io::copy(&mut p, &mut std::io::sink());
            }
            buf
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = std::time::Instant::now() + PROBE_LIMIT;
    // The child is waited for only once its pipes have closed: until then its process group is
    // still its own, and a descendant holding a pipe open is killed with it at the deadline.
    loop {
        if stdout.is_finished()
            && stderr.is_finished()
            && let Ok(Some(status)) = child.try_wait()
        {
            return Ok(std::process::Output {
                status,
                stdout: stdout.join().unwrap_or_default(),
                stderr: stderr.join().unwrap_or_default(),
            });
        }
        if std::time::Instant::now() >= deadline {
            kill_tree(&mut child);
            let _ = child.wait();
            return Err(format!("did not finish in {} s", PROBE_LIMIT.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// `<cli> --version`, first line.
fn version(exe: &Path, cwd: &Path) -> Option<String> {
    let out = output(command(exe, cwd).arg("--version")).ok()?;
    let v = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// The newest stored result per CLI, one line each, for doctor.
pub fn doctor(db: &Connection) -> Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT cli, version, passed, detail FROM isolation i
         WHERE ts = (SELECT MAX(ts) FROM isolation WHERE cli = i.cli) ORDER BY cli",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let (cli, version, passed, detail): (String, String, bool, String) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            Ok(if passed {
                format!("{cli} ({version}): cannot act, curates")
            } else {
                format!("{cli} ({version}): skipped as a curator: {detail}")
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// The curator's profile widened by one read grant, codex's own install, without which no command
/// can start at all under bubblewrap (openai/codex#29049). The probe needs commands to run: if the
/// wider profile refuses them, the curator's narrower one does too.
fn probe_profile() -> String {
    CODEX_PROFILE.replace(
        r#"":minimal"="read"}"#,
        r#"":minimal"="read","~/.codex/packages"="read"}"#,
    )
}

/// Probe codex's curator profile with no model (docs/spike/curator-isolation.md). Each probe must
/// end in the tool's own refusal (`touch:`, `cat:`, `curl: (`): a tool the sandbox could not start
/// (codex exits 101, "Failed to execvp") proves nothing, and neither does a host without the tool.
fn probe(exe: &Path, home: &Path, cwd: &Path) -> Result<(), String> {
    // The hosted tools the profile does not govern must be off under the curator's flags, as codex
    // resolves them for the curator, which reads no user config: `features list` has no
    // `--ignore-user-config`, so it lists them under an empty CODEX_HOME.
    let bare = cwd.join("codex-home");
    std::fs::create_dir_all(&bare).map_err(|e| format!("codex features list: {e}"))?;
    let mut features = command(exe, cwd);
    features.env("CODEX_HOME", &bare).args(["features", "list"]);
    for f in CODEX_OFF {
        features.args(["--disable", f]);
    }
    let out = output(&mut features).map_err(|e| format!("codex features list: {e}"))?;
    let listed = String::from_utf8_lossy(&out.stdout);
    for f in CODEX_OFF {
        // `<name>  <stage>  <true|false>`
        let off = listed.lines().any(|l| {
            let mut w = l.split_whitespace();
            w.next() == Some(f) && w.last() == Some("false")
        });
        if !off {
            return Err(format!("codex feature {f} is not off"));
        }
    }
    let unreviewed = listed.lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        (w.last() == Some(&"true") && !CODEX_ON.contains(&w[0])).then(|| w[0])
    });
    if let Some(f) = unreviewed {
        return Err(format!("codex feature {f} is on and has not been reviewed"));
    }
    // Hosted web search is a setting, not a feature: `web_search="disabled"` holds only while
    // this codex still reads that key. An invalid value must be refused by name, with the value
    // the curator sets among the allowed ones; a codex that renamed the key would ignore any value.
    let bad =
        output(command(exe, cwd).args(["features", "list", "-c", r#"web_search="oboete-probe""#]))
            .map_err(|e| format!("codex features list: {e}"))?;
    let said = String::from_utf8_lossy(&bad.stderr);
    if bad.status.success() || !(said.contains("`web_search`") && said.contains("`disabled`")) {
        return Err("codex does not refuse an invalid web_search setting".into());
    }
    let canary = Canary::new(home).map_err(|e| format!("canary: {e}"))?;
    // What the command said, and its exit code.
    let run = |argv: &[&str]| -> Result<(Option<i32>, String), String> {
        let out = output(
            command(exe, cwd)
                // With the managed requirements, as `codex exec` resolves the profile.
                .args(["sandbox", "--include-managed-config"])
                .args(["-c", &probe_profile(), "-P", "curator", "--"])
                .args(argv),
        )
        .map_err(|e| format!("codex sandbox: {e}"))?;
        let said = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let tool = argv[0];
        if !out.status.success() && !said.lines().any(|l| l.starts_with(&format!("{tool}:"))) {
            return Err(format!("the {tool} probe did not run ({})", out.status));
        }
        Ok((out.status.code(), said))
    };
    // A harmless command must run, or a sandbox that never started would look like a pass.
    run(&["true"])?;
    let touched = canary.dir.join("touched");
    run(&["touch", &touched.to_string_lossy()])?;
    if touched.exists() {
        return Err("a command wrote outside the sandbox".into());
    }
    // The secret's content, not its token: a refusal names the path, which holds the token too.
    if run(&["cat", &canary.secret.to_string_lossy()])?
        .1
        .contains(&canary.content())
    {
        return Err("a command read a file under HOME".into());
    }
    // Only a refused connection passes (curl's exit 7). The listener never answers, so a curl
    // that got through waits out its time (28) instead; any other ending proves nothing either.
    let (code, _) = run(&[
        "curl",
        "-sS",
        "--noproxy",
        "*",
        "--max-time",
        "3",
        &canary.url(),
    ])?;
    if code != Some(7) {
        return Err(format!(
            "a command reached the network (curl exit {code:?})"
        ));
    }
    probe_exec(exe, cwd, &canary)
}

/// The curator's `codex exec`, with the wider profile and only its model changed, against the
/// scripted model of `codex_probe`, which makes it try each action through its exec tool: in the
/// sandbox, escalated, and in a sub-agent. `codex sandbox` above shows the profile refuses; this
/// shows the model's own paths do not step around it (0.155.1 runs a spawned sub-agent under
/// `--ephemeral`, and a command may ask to run outside the sandbox).
fn probe_exec(exe: &Path, cwd: &Path, canary: &Canary) -> Result<(), String> {
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"));
    let model = crate::codex_probe::Model::start(crate::codex_probe::Actions {
        cat: format!("cat {}", quote(&canary.secret)),
        touch: format!("touch {}", quote(&canary.dir.join("touched"))),
        curl: format!("curl -sS --noproxy '*' --max-time 3 {}", canary.url()),
    })
    .map_err(|e| format!("the probe's model: {e}"))?;
    let base = format!("http://127.0.0.1:{}/v1", model.port);
    let mut cmd = command(exe, cwd);
    cmd.arg("exec")
        .args(crate::provider::codex_exec_flags(&probe_profile()))
        .args(["-c", r#"model_provider="oboeteprobe""#])
        .args(["-c", r#"model_providers.oboeteprobe.name="oboete probe""#])
        .args([
            "-c",
            &format!(
                "model_providers.oboeteprobe.base_url={}",
                toml::Value::from(base)
            ),
        ])
        .args(["-c", r#"model_providers.oboeteprobe.wire_api="responses""#])
        .args([
            "-c",
            r#"model_providers.oboeteprobe.env_key="OBOETE_PROBE_KEY""#,
        ])
        .args(["-c", "model_providers.oboeteprobe.request_max_retries=0"])
        .args(["-c", "model_providers.oboeteprobe.stream_max_retries=0"])
        .arg("probe")
        // Only this user can read a process's environment: codex's requests carry the key, and
        // another local user's cannot.
        .env("OBOETE_PROBE_KEY", &model.key)
        // The probe's model is on this machine, never behind the environment's proxy.
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = own_group(&mut cmd)
        .spawn()
        .map_err(|e| format!("codex exec: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                kill_tree(&mut child);
                let _ = child.wait();
                return Err("the probe's codex exec did not finish".into());
            }
        }
    }
    let touched = std::fs::read_dir(&canary.dir).is_ok_and(|d| {
        d.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("touched"))
    });
    let seen = model
        .seen
        .lock()
        .map_err(|_| "the probe's model failed")?
        .clone();
    crate::codex_probe::verdict(&seen, &canary.content(), touched)
}

/// A private directory under HOME with a secret in it, and a port on 127.0.0.1 that accepts
/// connections (the kernel's backlog) and never answers, for one probe.
struct Canary {
    dir: PathBuf,
    secret: PathBuf,
    token: String,
    port: u16,
    _listener: TcpListener,
}

impl Canary {
    fn new(home: &Path) -> std::io::Result<Self> {
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(std::io::Error::other)?;
        let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let dir = home.join(format!(".oboete-isolation-{token}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // The port first: from here on a failure must remove the directory it made.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        builder.create(&dir)?;
        let secret = dir.join("secret");
        if let Err(e) = std::fs::write(&secret, format!("SECRET-{token}\n")) {
            std::fs::remove_dir_all(&dir).ok();
            return Err(e);
        }
        Ok(Self {
            dir,
            secret,
            token,
            port,
            _listener: listener,
        })
    }
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, self.token)
    }
    fn content(&self) -> String {
        format!("SECRET-{}", self.token)
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `codex features list` as 0.155.1 answers it: an invalid `web_search` refused by name.
    const FEATURES: &str = "case \"$*\" in *web_search=*) \
        echo 'Error: unknown variant `oboete-probe`, expected one of `disabled`, `cached`' >&2; \
        echo 'in `web_search`' >&2; exit 1;; esac; for f in plugins apps browser_use \
        browser_use_external in_app_browser computer_use image_generation; do \
        echo \"$f  stable  false\"; done; echo 'shell_tool  stable  true'; \
        case \"$CODEX_HOME\" in */codex-home) ;; *) echo 'from_user_config  stable  true';; esac";

    /// A fake codex whose `sandbox` runs `body` with the probe's argv as "$@"; every call is
    /// appended to `calls`.
    fn fake(dir: &Path, features: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("codex");
        let calls = dir.join("calls");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho \"$1\" >> '{}'\ncase \"$1\" in\n\
                 --version) echo 'codex-cli 9.9.9';;\n\
                 features) {features};;\n\
                 sandbox) while [ \"$1\" != -- ]; do shift; done; shift\n{body};;\n\
                 exec) {EXEC};;\n\
                 esac\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    /// codex exec as 0.155.1 ran the scripted model's calls in the dogfood user, told to the model
    /// by `post <agent> <outputs>`: each action refused in the sandbox and escalated, in the root
    /// and in the sub-agent.
    const EXEC: &str = r#"for a in "$@"; do case "$a" in model_providers.oboeteprobe.base_url=*)
        url=${a#*=}; url=${url#\"}; url=${url%\"};; esac; done
        T='{"type":"additional_tools","tools":[{"type":"namespace","name":"functions","tools":[{"name":"exec"}]},{"type":"namespace","name":"collaboration","tools":[{"name":"spawn_agent"}]}]}'
        post() { curl -sS -o /dev/null -H "Authorization: Bearer $OBOETE_PROBE_KEY" --data "{\"client_metadata\":{\"x-codex-turn-metadata\":\"{\\\"agent_name\\\":\\\"$1\\\"}\"},\"input\":[$2,$T]}" "$url/responses"; }
        out() { printf '{"type":"custom_tool_call_output","call_id":"call_%s","output":"%s"}' "$1" "$2"; }
        S="cat:1:cat: x: No such file or directory\ntouch:1:touch: cannot touch x\ncurl:${CURL:-7}:curl: (7) Failed\n"
        E="cat:threw:rejected\ntouch:threw:rejected\ncurl:threw:rejected\n"
        post /root "$(out root_0 "$S"),$(out root_1 "$E"),$(out root_2 spawned)"
        post /root/probe "$(out sub_0 "$S"),$(out sub_1 "$E")""#;

    /// The refusals codex 0.155.1 gave under the probe profile in the dogfood user.
    const REFUSES: &str = "case \"$1\" in true) exit 0;;\n\
        touch) echo \"touch: cannot touch '$2': No such file or directory\" >&2; exit 1;;\n\
        cat) echo \"cat: $2: No such file or directory\" >&2; exit 1;;\n\
        curl) echo 'curl: (7) Failed to connect' >&2; exit 7;;\n\
        esac";

    fn gate_with(exe: &Path, home: &Path) -> (Connection, Gate) {
        let db = crate::providers_db::open(home).unwrap();
        let g = gate_codex(&db, exe, home).unwrap();
        (db, g)
    }

    #[test]
    fn codex_passes_when_every_probe_is_refused_and_is_probed_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake(dir.path(), FEATURES, REFUSES);
        let (db, g) = gate_with(&exe, dir.path());
        assert_eq!(g, Gate::Passed);
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(gate_codex(&db, &exe, dir.path()).unwrap(), Gate::Passed);
        let again = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(
            again.lines().count(),
            2 * calls.lines().count(),
            "probed again"
        );
        assert_eq!(doctor(&db).unwrap().len(), 1);
        // A codex that stops answering fails before the probe, and doctor shows that, not the pass.
        std::fs::write(&exe, "#!/bin/sh\nexit 1\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(matches!(
            gate_codex(&db, &exe, dir.path()).unwrap(),
            Gate::Failed(_)
        ));
        assert_eq!(
            doctor(&db).unwrap(),
            ["codex (unknown version): skipped as a curator: codex --version did not answer"]
        );
    }

    #[test]
    fn codex_fails_when_a_probe_reads_writes_or_fetches() {
        for (leak, why) in [
            ("cat) cat \"$2\";;", "read a file"),
            ("touch) touch \"$2\";;", "wrote outside"),
            (
                "curl) curl -sS --noproxy '*' --max-time 2 \"$7\";;",
                "reached the network",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let body = REFUSES.replacen("case \"$1\" in", &format!("case \"$1\" in {leak}"), 1);
            let (_, g) = gate_with(&fake(dir.path(), FEATURES, &body), dir.path());
            assert!(
                matches!(&g, Gate::Failed(w) if w.contains(why)),
                "{leak}: {g:?}"
            );
        }
    }

    #[test]
    fn a_probe_the_sandbox_could_not_start_is_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let body = REFUSES.replacen(
            "case \"$1\" in",
            "case \"$1\" in touch) echo \"panicked: Failed to execvp $1\" >&2; exit 101;;",
            1,
        );
        let (_, g) = gate_with(&fake(dir.path(), FEATURES, &body), dir.path());
        assert!(
            matches!(&g, Gate::Failed(w) if w.contains("touch probe did not run")),
            "{g:?}"
        );
    }

    /// Live, with `--ignored`, in the dogfood user only: the installed codex under the curator's
    /// environment and profile.
    #[test]
    #[ignore]
    fn live_codex_cannot_act_under_the_curator_profile() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::providers_db::open(dir.path()).unwrap();
        let home = crate::config::home_dir();
        assert_eq!(
            gate_codex(&db, Path::new("codex"), &home).unwrap(),
            Gate::Passed
        );
        println!("{:?}", doctor(&db).unwrap());
    }

    #[test]
    fn a_fetch_the_model_makes_through_codex_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake(dir.path(), FEATURES, REFUSES);
        let script = std::fs::read_to_string(&exe).unwrap();
        std::fs::write(&exe, script.replace("${CURL:-7}", "28")).unwrap();
        let (_, g) = gate_with(&exe, dir.path());
        assert_eq!(
            g,
            Gate::Failed("the curl probe was not refused (28)".into())
        );
    }

    #[test]
    fn a_probe_that_stalls_fails_the_gate_in_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let (_, g) = gate_with(&fake(dir.path(), "sleep 60", REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex features list: did not finish in 5 s".into())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn a_descendant_holding_the_output_open_fails_the_gate_in_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let pid = dir.path().join("pid");
        let stalls = format!("(sleep 60 & echo $! > '{}')", pid.display());
        let (_, g) = gate_with(&fake(dir.path(), &stalls, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex features list: did not finish in 5 s".into())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        // The descendant went with it: it no longer runs. An orphan's zombie is its new parent's to
        // reap (PID 1 in a container without an init may never do it).
        let pid = std::fs::read_to_string(&pid).unwrap();
        let alive = || {
            Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .is_ok_and(|o| {
                    let stat = String::from_utf8_lossy(&o.stdout);
                    !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
                })
        };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while alive() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(), "the descendant {} still runs", pid.trim());
    }

    #[test]
    fn a_codex_that_no_longer_reads_web_search_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let ignores = FEATURES.replace("*web_search=*)", "*web_search_renamed=*)");
        let (_, g) = gate_with(&fake(dir.path(), &ignores, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex does not refuse an invalid web_search setting".into())
        );
    }

    #[test]
    fn a_feature_on_that_was_not_reviewed_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features = FEATURES.to_owned() + "; echo 'new_hosted_tool  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex feature new_hosted_tool is on and has not been reviewed".into())
        );
    }

    /// codex 0.160.0 turns `write_stdin_approval` on by default: input to a terminal that was
    /// launched with more than the current permissions waits for an approval (off, it waits for
    /// none). It offers the model no tool, so it is reviewed and the gate passes with it on.
    #[test]
    fn the_stdin_approval_of_codex_0_160_does_not_fail_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features = FEATURES.to_owned() + "; echo 'write_stdin_approval  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(g, Gate::Passed);
    }

    #[test]
    fn a_hosted_tool_left_on_fails_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let features =
            FEATURES.replace("image_generation;", ";") + "; echo 'image_generation  stable  true'";
        let (_, g) = gate_with(&fake(dir.path(), &features, REFUSES), dir.path());
        assert_eq!(
            g,
            Gate::Failed("codex feature image_generation is not off".into())
        );
    }
}
