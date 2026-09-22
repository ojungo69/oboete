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

/// Run a subscription CLI headless, with the smallest configuration each one allows: no hooks,
/// no tools, no session persistence, no user settings or MCP servers where the CLI can skip them.
fn cli_headless(
    cli: &str,
    model: Option<&str>,
    timeout_s: u64,
    prompt: &str,
    schema: &Value,
) -> Result<Value, CallError> {
    let schema_text = schema.to_string();
    let scratch = scratch_dir()?;
    let last = scratch.0.join("last.json");
    let mut cmd = Command::new(cli);
    match cli {
        "agy" => {
            cmd.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
            ]);
            cmd.args(["--new-project", "--dangerously-skip-permissions"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
        }
        "claude" => {
            cmd.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
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
        }
        "grok" => {
            cmd.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
            ]);
            cmd.args(["--tools", "", "--max-turns", "1"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
        }
        "codex" => {
            let schema_file = scratch.0.join("schema.json");
            std::fs::write(&schema_file, &schema_text)
                .map_err(|e| CallError::other(format!("write schema: {e}")))?;
            cmd.args(["exec", prompt, "--output-schema"])
                .arg(&schema_file);
            cmd.arg("-o").arg(&last);
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
        }
        other => {
            return Err(CallError::other(format!(
                "unsupported cli provider {other}"
            )));
        }
    }
    // Keep the CLI out of the user's repo and away from the parent's secrets-bearing env, and
    // make sure our own hooks ignore the summarizer's session.
    cmd.current_dir(&scratch.0)
        .stdin(Stdio::null())
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
    let mut child = cmd
        .spawn()
        .map_err(|e| CallError::other(format!("spawn {cli}: {e}")))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(CallError::other(format!("wait {cli}: {e}"))),
        }
        if Instant::now() > deadline {
            child.kill().ok();
            return Err(CallError::other(format!(
                "{cli} timed out after {timeout_s}s"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = child
        .wait_with_output()
        .map_err(|e| CallError::other(format!("output {cli}: {e}")))?;
    if !status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(CallError::other(format!(
            "{cli} exit {status}: {}",
            err.chars().take(300).collect::<String>()
        )));
    }
    let text = if cli == "codex" {
        std::fs::read_to_string(&last)
            .map_err(|_| CallError::other("invalid output: codex wrote no last message"))?
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    extract_structured(cli, &text)
}

/// The schema-validated object out of a CLI's JSON envelope: `structured_output` (claude, agy),
/// `structuredOutput` (grok), or the answer text itself when the envelope is the answer (codex).
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
