//! Summarizer providers and the fallback chain.
//! Every provider takes (prompt, json schema) and returns the parsed JSON object or an error.
//! The chain walks providers in order; a provider is skipped when its daily budget is spent or
//! it is cooling down after a failure in this run, a 429 with a near reset is waited out once,
//! and any other error (HTTP, timeout, unparsable/invalid output) moves on to the next provider.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::config::{self, Provider};
use crate::{db, hook};

/// Longest 429 reset the chain waits for instead of falling through.
const MAX_WAIT_S: f64 = 60.0;
/// Cooldown after a 429 that was not waited out (rate windows are a minute), and after an
/// outage-shaped failure (5xx, timeout, missing binary, auth), which the next session would only
/// hit again. A schema-mismatch 400 or unparsable output is per-answer luck and gets no cooldown.
const COOLDOWN_429: Duration = Duration::from_secs(45);
const COOLDOWN_OUTAGE: Duration = Duration::from_secs(600);

pub struct ChainResult {
    pub provider: String,
    pub output: Value,
    /// Providers tried before the one that answered (name, reason).
    pub fallbacks: Vec<(String, String)>,
}

/// One failed call, with what the chain needs to decide what to do next.
#[derive(Debug)]
struct CallError {
    status: Option<u16>,
    retry_after_s: Option<f64>,
    message: String,
}

impl CallError {
    fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            retry_after_s: None,
            message: message.into(),
        }
    }
    fn invalid(&self) -> bool {
        self.message.starts_with("invalid output")
    }
}

/// The chain for one observe run: a provider that failed cools down before it is tried again.
pub struct Chain<'a> {
    providers: &'a [Provider],
    down_until: HashMap<String, Instant>,
}

impl<'a> Chain<'a> {
    pub fn new(providers: &'a [Provider]) -> Self {
        Self {
            providers,
            down_until: HashMap::new(),
        }
    }

    /// Walk the chain. `OBOETE_FAIL_PROVIDER=<name>` forces that provider to fail (fallback proof).
    pub fn summarize(
        &mut self,
        conn: &Connection,
        prompt: &str,
        schema: &Value,
    ) -> Result<ChainResult> {
        let forced_fail = std::env::var("OBOETE_FAIL_PROVIDER").ok();
        let mut fallbacks = Vec::new();
        for p in self.providers {
            let name = p.name().to_string();
            if self
                .down_until
                .get(&name)
                .is_some_and(|t| *t > Instant::now())
            {
                fallbacks.push((name, "cooling down after an earlier failure".into()));
                continue;
            }
            let used = db::calls_today(conn, &name)?;
            if used >= p.daily_budget() {
                db::record_call(
                    conn,
                    &name,
                    "budget",
                    0,
                    Some(&format!("{used}/{}", p.daily_budget())),
                )?;
                fallbacks.push((name, "daily budget spent".into()));
                continue;
            }
            let started = Instant::now();
            let mut result = if forced_fail.as_deref() == Some(name.as_str()) {
                Err(CallError::other("forced failure (OBOETE_FAIL_PROVIDER)"))
            } else {
                call(p, prompt, schema)
            };
            // The retry is a second request: only when the daily budget has room for it.
            if let Err(e) = &result
                && e.status == Some(429)
                && p.retry_429()
                && used + 1 < p.daily_budget()
                && let Some(wait) = e.retry_after_s
                && wait <= MAX_WAIT_S
            {
                db::record_call(
                    conn,
                    &name,
                    "wait",
                    started.elapsed().as_millis() as i64,
                    Some(&format!("429, retry in {wait:.0}s")),
                )?;
                std::thread::sleep(Duration::from_secs_f64(wait + 0.5));
                result = call(p, prompt, schema);
            }
            let ms = started.elapsed().as_millis() as i64;
            match result {
                Ok(v) => {
                    db::record_call(conn, &name, "ok", ms, None)?;
                    return Ok(ChainResult {
                        provider: name,
                        output: v,
                        fallbacks,
                    });
                }
                Err(e) => {
                    let outcome = if e.invalid() { "invalid" } else { "error" };
                    db::record_call(conn, &name, outcome, ms, Some(&e.message))?;
                    if let Some(c) = cooldown_for(&e) {
                        self.down_until.insert(name.clone(), Instant::now() + c);
                    }
                    fallbacks.push((name, e.message));
                }
            }
        }
        Err(anyhow!(
            "every provider failed: {}",
            fallbacks
                .iter()
                .map(|(n, r)| format!("{n}: {r}"))
                .collect::<Vec<_>>()
                .join(" | ")
        ))
    }
}

/// How long to skip a provider after this failure. Per-answer failures (schema mismatch, an
/// unparsable reply, OpenRouter's moderation 403) get none: the next session may pass.
/// A rejected key or an outage fails every request of the pass, so it is skipped for a while.
fn cooldown_for(e: &CallError) -> Option<Duration> {
    let moderation = {
        let m = e.message.to_ascii_lowercase();
        m.contains("moderat") || m.contains("flagged")
    };
    match e.status {
        Some(429) => Some(COOLDOWN_429),
        Some(401) => Some(COOLDOWN_OUTAGE),
        Some(403) if !moderation => Some(COOLDOWN_OUTAGE),
        Some(400..=499) => None,
        _ if e.invalid() => None,
        _ => Some(COOLDOWN_OUTAGE),
    }
}

fn call(p: &Provider, prompt: &str, schema: &Value) -> Result<Value, CallError> {
    match p {
        Provider::Openai {
            base_url,
            key_file,
            model,
            timeout_s,
            extra,
            ..
        } => openai_compat(
            base_url,
            key_file.as_deref(),
            model,
            *timeout_s,
            extra,
            prompt,
            schema,
        ),
        Provider::Cli {
            cli,
            model,
            timeout_s,
            ..
        } => cli_headless(cli, model.as_deref(), *timeout_s, prompt, schema),
    }
}

fn openai_compat(
    base_url: &str,
    key_file: Option<&Path>,
    model: &str,
    timeout_s: u64,
    extra: &serde_json::Map<String, Value>,
    prompt: &str,
    schema: &Value,
) -> Result<Value, CallError> {
    let mut body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "response_format": {"type": "json_schema", "json_schema": {"name": "memory", "strict": true, "schema": schema}}
    });
    for (k, v) in extra {
        body[k] = v.clone();
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_s)))
        .http_status_as_error(false)
        .user_agent(concat!("oboete/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut req = agent.post(&url);
    if let Some(key_file) = key_file {
        let key = config::read_key(key_file).map_err(|e| CallError::other(format!("{e:#}")))?;
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    let mut resp = req
        .send_json(&body)
        .map_err(|e| CallError::other(format!("http request: {e}")))?;
    let status = resp.status().as_u16();
    let retry_after_s = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok());
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| CallError::other(format!("read body: {e}")))?;
    if status != 200 {
        return Err(CallError {
            status: Some(status),
            retry_after_s: retry_after_s.or_else(|| retry_after_in_body(&text)),
            message: format!(
                "http {status}: {}",
                text.chars().take(300).collect::<String>()
            ),
        });
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| CallError::other("invalid output: response is not JSON"))?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| CallError::other("invalid output: no choices[0].message.content"))?;
    serde_json::from_str(content)
        .map_err(|e| CallError::other(format!("invalid output: content is not JSON ({e})")))
}

/// Groq spells the reset out in the body: "Please try again in 17.28s".
fn retry_after_in_body(body: &str) -> Option<f64> {
    let rest = &body[body.find("try again in ")? + "try again in ".len()..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let secs = num.parse::<f64>().ok()?;
    // "1m26.4s" style is not produced here; a bare number is seconds.
    rest[num.len()..].starts_with('s').then_some(secs)
}

/// A fresh private directory for one CLI run, removed again when dropped (on every return
/// path, so failed attempts leave nothing behind). The name is random and the directory must
/// not exist yet, so nobody else on the machine can plant one under a guessable name (the pid)
/// and read what the CLI writes there; on Unix it is also created mode 0700.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn scratch_dir() -> Result<Scratch, CallError> {
    let mut raw = [0u8; 8];
    getrandom::fill(&mut raw).map_err(|e| CallError::other(format!("scratch dir: {e}")))?;
    let name: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("oboete-cli-{name}"));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|e| CallError::other(format!("scratch dir: {e}")))?;
    Ok(Scratch(dir))
}

/// The command for one headless CLI run, with the smallest configuration each one allows: no
/// hooks, no tools, no session persistence, no user settings or MCP servers where the CLI can skip
/// them. The prompt never goes on the command line (any local user can read another process's
/// arguments): it is piped to stdin (claude, codex, and agy as one stream-json turn) or written
/// into the private scratch directory (grok's --prompt-file). Returns what to write to stdin.
fn headless_command(
    cli: &str,
    model: Option<&str>,
    dir: &Path,
    prompt: &str,
    schema_text: &str,
) -> Result<(Command, Option<String>), CallError> {
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| CallError::other(format!("write {name}: {e}")))?;
        Ok::<_, CallError>(path)
    };
    let mut cmd = Command::new(cli);
    let stdin = match cli {
        "agy" => {
            // `--print=` keeps -p from taking the next flag as its prompt; the turn comes on stdin.
            cmd.args(["--print=", "--input-format", "stream-json"]);
            cmd.args([
                "--output-format",
                "stream-json",
                "--json-schema",
                schema_text,
            ]);
            cmd.args(["--new-project", "--dangerously-skip-permissions"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            let turn = json!({"event": "user", "message": {"role": "user", "content": prompt}});
            Some(format!("{turn}\n"))
        }
        "claude" => {
            cmd.args([
                "-p",
                "--output-format",
                "json",
                "--json-schema",
                schema_text,
            ]);
            cmd.args([
                "--setting-sources",
                "",
                "--tools",
                "",
                "--strict-mcp-config",
            ]);
            cmd.args([
                "--no-session-persistence",
                "--settings",
                r#"{"disableAllHooks":true}"#,
            ]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            Some(prompt.to_owned())
        }
        "grok" => {
            cmd.arg("--prompt-file").arg(write("task.md", prompt)?);
            cmd.args(["--output-format", "json", "--json-schema", schema_text]);
            cmd.args(["--tools", "", "--max-turns", "1"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            None
        }
        "codex" => {
            // Without a PROMPT argument, codex exec reads the instructions from stdin.
            cmd.arg("exec")
                .arg("--output-schema")
                .arg(write("schema.json", schema_text)?);
            cmd.arg("-o").arg(dir.join("last.json"));
            cmd.args([
                "--ephemeral",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
            ]);
            cmd.args(["-c", "model_reasoning_effort=low"]);
            if let Some(m) = model {
                cmd.args(["-c", &format!("model={m}")]);
            }
            Some(prompt.to_owned())
        }
        other => {
            return Err(CallError::other(format!(
                "unsupported cli provider {other}"
            )));
        }
    };
    Ok((cmd, stdin))
}

/// Run a subscription CLI headless (see `headless_command`) and return its structured answer.
fn cli_headless(
    cli: &str,
    model: Option<&str>,
    timeout_s: u64,
    prompt: &str,
    schema: &Value,
) -> Result<Value, CallError> {
    let scratch = scratch_dir()?;
    let last = scratch.0.join("last.json");
    let (mut cmd, stdin) = headless_command(cli, model, &scratch.0, prompt, &schema.to_string())?;
    // Keep the CLI out of the user's repo and away from the parent's secrets-bearing env, and
    // make sure our own hooks ignore the summarizer's session.
    cmd.current_dir(&scratch.0)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(hook::SKIP_ENV, "1")
        .env_remove("CLAUDECODE");
    for (k, _) in std::env::vars() {
        if k.contains("TOKEN")
            || k.contains("KEY")
            || k.contains("SECRET")
            || k.contains("PASSWORD")
        {
            cmd.env_remove(&k);
        }
    }
    let out = run_cli(cmd, stdin, Duration::from_secs(timeout_s)).map_err(|e| {
        let tagged = if e.invalid() {
            format!(
                "invalid output: {cli}: {}",
                &e.message["invalid output: ".len()..]
            )
        } else {
            format!("{cli} {}", e.message)
        };
        CallError::other(tagged)
    })?;
    let stdout = String::from_utf8_lossy(&out);
    let text = match cli {
        "codex" => {
            use std::io::Read;
            let mut text = String::new();
            std::fs::File::open(&last)
                .and_then(|f| f.take(MAX_CLI_OUTPUT).read_to_string(&mut text))
                .map_err(|_| CallError::other("invalid output: codex wrote no last message"))?;
            text
        }
        "agy" => agy_result(&stdout)?,
        _ => stdout.into_owned(),
    };
    extract_structured(cli, &text)
}

/// The schema-validated object out of a CLI's JSON envelope: `structured_output` (claude, agy),
/// `structuredOutput` (grok), or the answer text itself when the envelope is the answer (codex).
/// Most a CLI may print before its answer is dropped: a broken or hijacked provider must not
/// fill memory (the summaries it returns are capped much lower anyway).
const MAX_CLI_OUTPUT: u64 = 1 << 20;

/// Spawn `cmd`, feed it `stdin`, and return its stdout if it exits 0 within `timeout`.
/// stdin, stdout and stderr each get a thread: a prompt larger than the pipe buffer, or an
/// answer larger than it, must not deadlock against a child that has not read or exited yet.
fn run_cli(
    mut cmd: Command,
    stdin: Option<String>,
    timeout: Duration,
) -> Result<Vec<u8>, CallError> {
    use std::io::{Read, Write};
    let mut child = cmd
        .spawn()
        .map_err(|e| CallError::other(format!("spawn: {e}")))?;
    let feeder = child.stdin.take().zip(stdin).map(|(mut w, text)| {
        // A child that exits without reading just makes the write fail.
        std::thread::spawn(move || w.write_all(text.as_bytes()).ok())
    });
    let drain = |r: Option<Box<dyn Read + Send>>| {
        r.map(|r| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut r = r.take(MAX_CLI_OUTPUT + 1);
                r.read_to_end(&mut buf).ok();
                // Keep reading past the cap so the child is never blocked on a full pipe.
                std::io::copy(r.get_mut(), &mut std::io::sink()).ok();
                buf
            })
        })
    };
    let stdout = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let stderr = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(CallError::other(format!("wait: {e}"))),
        }
        if Instant::now() > deadline {
            // Reap it before the scratch directory goes: a killed child still holds that
            // directory as its cwd until it is waited for (Windows refuses the removal).
            child.kill().ok();
            child.wait().ok();
            return Err(CallError::other(format!(
                "timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if let Some(f) = feeder {
        f.join().ok();
    }
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    let (out, err) = (join(stdout), join(stderr));
    if !status.success() {
        let err = String::from_utf8_lossy(&err);
        return Err(CallError::other(format!(
            "exit {status}: {}",
            err.chars().take(300).collect::<String>()
        )));
    }
    if out.len() as u64 > MAX_CLI_OUTPUT {
        return Err(CallError::other(format!(
            "invalid output: more than {MAX_CLI_OUTPUT} bytes"
        )));
    }
    Ok(out)
}

/// agy's stream-json output: one event per line; the answer is in the last `result` event.
fn agy_result(stdout: &str) -> Result<String, CallError> {
    stdout
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["event"] == "result")
        .map(|v| v["result"].to_string())
        .ok_or_else(|| CallError::other("invalid output: agy printed no result event"))
}

fn extract_structured(cli: &str, text: &str) -> Result<Value, CallError> {
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|e| CallError::other(format!("invalid output: {cli} output is not JSON ({e})")))?;
    if let Some(s) = v.get("structured_output").or(v.get("structuredOutput")) {
        return Ok(s.clone());
    }
    if v.get("observations").is_some() || v.get("summary").is_some() {
        return Ok(v);
    }
    let inner = ["result", "text", "response"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str));
    match inner {
        Some(s) => serde_json::from_str(s).map_err(|e| {
            CallError::other(format!("invalid output: {cli} answer is not JSON ({e})"))
        }),
        None => Err(CallError::other(format!(
            "invalid output: no structured_output in {cli} response"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_prompts_stay_off_the_command_line() {
        let scratch = scratch_dir().unwrap();
        let dir = &scratch.0;
        let prompt = "SECRET-SESSION-TEXT 秘密";
        for cli in ["agy", "claude", "grok", "codex"] {
            let (cmd, stdin) = headless_command(cli, Some("m"), dir, prompt, "{}").unwrap();
            let on_stdin = stdin.is_some_and(|t| t.contains(prompt));
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(
                args.iter().all(|a| !a.contains("SECRET-SESSION-TEXT")),
                "{cli}: {args:?}"
            );
            let file = dir.join("task.md");
            let in_file = std::fs::read_to_string(&file).is_ok_and(|t| t == prompt);
            assert!(
                on_stdin != in_file,
                "{cli}: exactly one of stdin / task.md carries it"
            );
            std::fs::remove_file(&file).ok();
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_runs_neither_deadlock_nor_outlive_the_timeout() {
        let sh = |script: &str| {
            let mut c = Command::new("sh");
            c.args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            c
        };
        // Larger than any pipe buffer in both directions.
        let big = "x".repeat(300_000);
        let out = run_cli(sh("cat"), Some(big.clone()), Duration::from_secs(10)).unwrap();
        assert_eq!(out, big.as_bytes());
        let err = run_cli(
            sh("head -c 1100000 /dev/zero"),
            None,
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(err.message.contains("more than"), "{}", err.message);
        let err = run_cli(sh("echo boom >&2; exit 3"), None, Duration::from_secs(10)).unwrap_err();
        assert!(err.message.contains("boom"), "{}", err.message);
        let start = Instant::now();
        let err = run_cli(sh("sleep 5"), None, Duration::from_secs(1)).unwrap_err();
        assert!(err.message.contains("timed out"), "{}", err.message);
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn agy_answer_is_the_last_result_event() {
        let out = concat!(
            "{\"event\":\"init\",\"session\":\"s\"}\n",
            "not json\n",
            "{\"event\":\"result\",\"result\":{\"structured_output\":{\"summary\":\"x\"}}}\n",
        );
        let text = agy_result(out).unwrap();
        assert_eq!(
            extract_structured("agy", &text).unwrap(),
            json!({"summary": "x"})
        );
        assert!(agy_result("{\"event\":\"init\"}\n").is_err());
    }

    #[test]
    fn unusable_cli_answers_are_invalid_output() {
        let dir = scratch_dir().unwrap();
        let e = headless_command("nano", None, &dir.0, "p", "{}").unwrap_err();
        assert!(e.message.contains("unsupported"), "{}", e.message);
        for bad in [r#"{"result": "not json"}"#, r#"{"other": 1}"#, "plain text"] {
            let e = extract_structured("claude", bad).unwrap_err();
            assert!(e.invalid(), "{bad}: {}", e.message);
        }
    }

    #[test]
    fn cooldown_depends_on_status_and_moderation() {
        let err = |status: Option<u16>, msg: &str| CallError {
            status,
            retry_after_s: None,
            message: msg.into(),
        };
        assert_eq!(cooldown_for(&err(Some(429), "")), Some(COOLDOWN_429));
        assert_eq!(
            cooldown_for(&err(Some(401), "invalid api key")),
            Some(COOLDOWN_OUTAGE)
        );
        assert_eq!(
            cooldown_for(&err(Some(403), "forbidden")),
            Some(COOLDOWN_OUTAGE)
        );
        assert_eq!(
            cooldown_for(&err(Some(403), "Your input was flagged for violence")),
            None
        );
        assert_eq!(cooldown_for(&err(Some(403), "blocked by moderation")), None);
        assert_eq!(
            cooldown_for(&err(Some(400), "Generated JSON does not match")),
            None
        );
        assert_eq!(cooldown_for(&err(Some(503), "")), Some(COOLDOWN_OUTAGE));
        assert_eq!(cooldown_for(&err(None, "timed out")), Some(COOLDOWN_OUTAGE));
    }

    #[test]
    fn retry_after_is_read_from_groq_bodies() {
        let body = r#"{"error":{"message":"Rate limit reached ... Please try again in 17.2875s. Need more tokens?"}}"#;
        assert_eq!(retry_after_in_body(body), Some(17.2875));
        assert_eq!(retry_after_in_body("try again in 2m3s"), None);
        assert_eq!(retry_after_in_body("nothing"), None);
    }

    #[test]
    fn structured_output_is_found_in_every_cli_envelope() {
        let want = json!({"observations": [], "summary": "s"});
        let claude =
            json!({"result": "{\"observations\":[],\"summary\":\"s\"}", "structured_output": want})
                .to_string();
        let grok = json!({"text": "x", "structuredOutput": want}).to_string();
        let agy = json!({"response": "{\"observations\":[],\"summary\":\"s\"}", "structured_output": want}).to_string();
        let codex = want.to_string();
        let text_only = json!({"result": "{\"observations\":[],\"summary\":\"s\"}"}).to_string();
        for (cli, text) in [
            ("claude", claude),
            ("grok", grok),
            ("agy", agy),
            ("codex", codex),
            ("claude", text_only),
        ] {
            assert_eq!(extract_structured(cli, &text).unwrap(), want, "{cli}");
        }
        assert!(
            extract_structured("grok", "not json")
                .unwrap_err()
                .invalid()
        );
        assert!(
            extract_structured("grok", "{\"text\":\"plain\"}")
                .unwrap_err()
                .invalid()
        );
    }
}
