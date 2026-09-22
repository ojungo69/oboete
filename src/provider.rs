//! Summarizer providers and the fallback chain.
//! Every provider takes (prompt, json schema) and returns the parsed JSON object or an error.
//! The chain walks providers in order; a provider is skipped when its daily budget is spent,
//! and any error (HTTP, timeout, unparsable/invalid output) moves on to the next one.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::config::{self, Provider};
use crate::db;

pub struct ChainResult {
    pub provider: String,
    pub output: Value,
    /// Providers tried before the one that answered (name, reason).
    pub fallbacks: Vec<(String, String)>,
}

/// Walk the chain. `OBOETE_FAIL_PROVIDER=<name>` forces that provider to fail (spike proof).
pub fn summarize(
    conn: &Connection,
    providers: &[Provider],
    prompt: &str,
    schema: &Value,
) -> Result<ChainResult> {
    let forced_fail = std::env::var("OBOETE_FAIL_PROVIDER").ok();
    let mut fallbacks = Vec::new();
    for p in providers {
        let name = p.name().to_string();
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
        let result = if forced_fail.as_deref() == Some(name.as_str()) {
            Err(anyhow!("forced failure (OBOETE_FAIL_PROVIDER)"))
        } else {
            call(p, prompt, schema)
        };
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
                let reason = format!("{e:#}");
                let outcome = if reason.contains("invalid output") {
                    "invalid"
                } else {
                    "error"
                };
                db::record_call(conn, &name, outcome, ms, Some(&reason))?;
                fallbacks.push((name, reason));
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

fn call(p: &Provider, prompt: &str, schema: &Value) -> Result<Value> {
    match p {
        Provider::Openai {
            base_url,
            key_file,
            model,
            timeout_s,
            ..
        } => openai_compat(base_url, key_file, model, *timeout_s, prompt, schema),
        Provider::Cli { cli, timeout_s, .. } => cli_headless(cli, *timeout_s, prompt, schema),
    }
}

fn openai_compat(
    base_url: &str,
    key_file: &Path,
    model: &str,
    timeout_s: u64,
    prompt: &str,
    schema: &Value,
) -> Result<Value> {
    let key = config::read_key(key_file)?;
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "response_format": {"type": "json_schema", "json_schema": {"name": "memory", "strict": true, "schema": schema}}
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_s)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent
        .post(&format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .header("Authorization", &format!("Bearer {key}"))
        .send_json(&body)
        .context("http request")?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().context("read body")?;
    if status != 200 {
        return Err(anyhow!(
            "http {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).context("response is not JSON")?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("invalid output: no choices[0].message.content"))?;
    serde_json::from_str(content).map_err(|e| anyhow!("invalid output: content is not JSON ({e})"))
}

/// Run a subscription CLI headless. Only `agy` in M0; `claude`/`grok`/`codex` follow the same shape in M1.
fn cli_headless(cli: &str, timeout_s: u64, prompt: &str, schema: &Value) -> Result<Value> {
    let schema_text = schema.to_string();
    let mut cmd = match cli {
        "agy" => {
            let mut c = Command::new("agy");
            c.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
                "--new-project",
                "--dangerously-skip-permissions",
            ]);
            c
        }
        "claude" => {
            let mut c = Command::new("claude");
            c.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
            ]);
            c
        }
        "grok" => {
            let mut c = Command::new("grok");
            c.args([
                "-p",
                prompt,
                "--output-format",
                "json",
                "--json-schema",
                &schema_text,
            ]);
            c
        }
        other => return Err(anyhow!("unsupported cli provider {other}")),
    };
    // Keep the CLI out of the user's repo and away from the parent's secrets-bearing env.
    let scratch = std::env::temp_dir().join(format!("oboete-cli-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    cmd.current_dir(&scratch)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, _) in std::env::vars() {
        if k.contains("TOKEN")
            || k.contains("KEY")
            || k.contains("SECRET")
            || k.contains("PASSWORD")
        {
            cmd.env_remove(&k);
        }
    }
    let mut child = cmd.spawn().with_context(|| format!("spawn {cli}"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            child.kill().ok();
            return Err(anyhow!("{cli} timed out after {timeout_s}s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "{cli} exit {status}: {}",
            err.chars().take(300).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| anyhow!("invalid output: {cli} stdout is not JSON ({e})"))?;
    // agy/claude/grok wrap the answer; `structured_output` is the schema-validated object.
    if let Some(s) = v.get("structured_output") {
        return Ok(s.clone());
    }
    if let Some(s) = v.get("result").and_then(Value::as_str) {
        return serde_json::from_str(s)
            .map_err(|e| anyhow!("invalid output: result is not JSON ({e})"));
    }
    Err(anyhow!(
        "invalid output: no structured_output in {cli} response"
    ))
}
