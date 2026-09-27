//! The isolation gate (spec 6.5, docs/milestone-3-plan.md Task 3): a curator CLI reads hostile
//! text, so it runs only when it provably cannot act. The gate tests capability, never obedience:
//! a model that declines a planted instruction proves nothing (agy declined one holding 57 tools).
//! - claude reports its tools in every call's init event, which `provider::claude_stream` checks.
//! - codex reports none, so its permission profile is probed directly, with no model, once per
//!   codex version and profile: a write, a read and a fetch must each run and be refused, and the
//!   hosted tools the profile does not govern must be off. Two of the curator's flags are not
//!   probed, since codex prints no effective config: `-c web_search="disabled"` and
//!   `--ignore-user-config` (the MCP servers of the owner's config; docs/spike/curator-isolation.md
//!   shows it drops them).
//! - Any other CLI (agy, grok) has no proven no-tool mode and is skipped (spec 6.5, R05).

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::provider::{CODEX_OFF, CODEX_PROFILE, curator_env, scratch_dir};

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

/// The gate for `cli`, probing codex when its version and profile have no stored result.
pub fn gate(db: &Connection, cli: &str) -> Result<Gate> {
    match cli {
        "claude" => Ok(Gate::Passed),
        "codex" => gate_codex(db, Path::new("codex"), &crate::config::home_dir()),
        _ => Ok(Gate::NotProven),
    }
}

fn gate_codex(db: &Connection, exe: &Path, home: &Path) -> Result<Gate> {
    // The curator's own kind of working directory: private, fresh, removed after.
    let Ok(scratch) = scratch_dir() else {
        return Ok(Gate::Failed("no scratch directory for the probe".into()));
    };
    let cwd = scratch.0.as_path();
    let Some(version) = version(exe, cwd) else {
        return Ok(Gate::Failed("codex --version did not answer".into()));
    };
    // A result holds for the profile it was probed with: a changed profile is probed again.
    let digest = Sha256::digest(format!("{}\n{}", probe_profile(), CODEX_OFF.join(",")));
    let key: String = format!(
        "{version} {:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    );
    if let Some(g) = stored(db, "codex", &key)? {
        return Ok(g);
    }
    let result = probe(exe, home, cwd);
    db.execute(
        "INSERT OR REPLACE INTO isolation(cli, version, passed, detail, ts) VALUES(?1,?2,?3,?4,?5)",
        params![
            "codex",
            key,
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

/// `<cli> --version`, first line.
fn version(exe: &Path, cwd: &Path) -> Option<String> {
    let out = command(exe, cwd).arg("--version").output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (out.status.success() && !v.is_empty()).then_some(v)
}

fn stored(db: &Connection, cli: &str, key: &str) -> Result<Option<Gate>> {
    let row: Option<(bool, String)> = db
        .query_row(
            "SELECT passed, detail FROM isolation WHERE cli=?1 AND version=?2",
            params![cli, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(passed, detail)| {
        if passed {
            Gate::Passed
        } else {
            Gate::Failed(detail)
        }
    }))
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
    // The hosted tools the profile does not govern must be off under the curator's flags.
    let mut features = command(exe, cwd);
    features.args(["features", "list"]);
    for f in CODEX_OFF {
        features.args(["--disable", f]);
    }
    let out = features
        .output()
        .map_err(|e| format!("codex features list: {e}"))?;
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
    let canary = Canary::new(home).map_err(|e| format!("canary: {e}"))?;
    let run = |argv: &[&str]| -> Result<String, String> {
        let out = command(exe, cwd)
            .args(["sandbox", "-c", &probe_profile(), "-P", "curator", "--"])
            .args(argv)
            .output()
            .map_err(|e| format!("codex sandbox: {e}"))?;
        let said = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let tool = argv[0];
        if !out.status.success() && !said.lines().any(|l| l.starts_with(&format!("{tool}:"))) {
            return Err(format!("the {tool} probe did not run ({})", out.status));
        }
        Ok(said)
    };
    // A harmless command must run, or a sandbox that never started would look like a pass.
    run(&["true"])?;
    let touched = canary.dir.join("touched");
    run(&["touch", &touched.to_string_lossy()])?;
    if touched.exists() {
        return Err("a command wrote outside the sandbox".into());
    }
    // The secret's content, not its token: a refusal names the path, which holds the token too.
    if run(&["cat", &canary.secret.to_string_lossy()])?.contains(&canary.content()) {
        return Err("a command read a file under HOME".into());
    }
    run(&[
        "curl",
        "-sS",
        "--noproxy",
        "*",
        "--max-time",
        "5",
        &canary.url(),
    ])?;
    if canary.hits() > 0 {
        return Err("a command reached the network".into());
    }
    Ok(())
}

/// A private directory under HOME with a secret in it, and a listener on 127.0.0.1, for one probe.
struct Canary {
    dir: PathBuf,
    secret: PathBuf,
    token: String,
    port: u16,
    hits: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
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
        builder.create(&dir)?;
        let secret = dir.join("secret");
        std::fs::write(&secret, format!("SECRET-{token}\n"))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let hits = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let (seen, stop) = (Arc::clone(&hits), Arc::clone(&done));
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        seen.fetch_add(1, Ordering::SeqCst);
                        let _ = s.read(&mut [0u8; 512]);
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        });
        Ok(Self {
            dir,
            secret,
            token,
            port,
            hits,
            done,
        })
    }
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, self.token)
    }
    fn content(&self) -> String {
        format!("SECRET-{}", self.token)
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const FEATURES: &str = "for f in plugins apps browser_use browser_use_external \
        in_app_browser computer_use image_generation; do echo \"$f  stable  false\"; done";

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
                 esac\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

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
    fn codex_passes_when_every_probe_is_refused_and_is_not_probed_again() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake(dir.path(), FEATURES, REFUSES);
        let (db, g) = gate_with(&exe, dir.path());
        assert_eq!(g, Gate::Passed);
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(gate_codex(&db, &exe, dir.path()).unwrap(), Gate::Passed);
        let again = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(
            again.lines().count(),
            calls.lines().count() + 1,
            "only --version"
        );
        assert_eq!(doctor(&db).unwrap().len(), 1);
    }

    #[test]
    fn codex_fails_when_a_probe_reads_writes_or_fetches() {
        for (leak, why) in [
            ("cat) cat \"$2\";;", "read a file"),
            ("touch) touch \"$2\";;", "wrote outside"),
            (
                "curl) curl -sS --noproxy '*' \"$7\";;",
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
