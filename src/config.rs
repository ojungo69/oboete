//! `<home>/config.toml` — provider chain and summary options. Missing file = built-in defaults.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_providers")]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub summary: Summary,
    #[serde(default)]
    pub embedding: Embedding,
}

/// Semantic search is a provider slot (docs/plan.md 2b, docs/pr-d.md): `none` (full-text only,
/// the default), `workers-ai` (bge-m3 on Cloudflare), later `local` (fastembed). One model per
/// store; switching reindexes.
#[derive(Debug, Clone, Deserialize)]
pub struct Embedding {
    #[serde(default = "default_embedding")]
    pub provider: String,
    /// Workers AI: the Cloudflare account that runs the model.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Workers AI: file whose second line is an API token limited to Workers AI (owner
    /// convention), not the account's global key.
    #[serde(default = "default_embedding_key")]
    pub key_file: PathBuf,
    /// Workers AI requests per day (up to 100 documents each). 200 is about 9,000 neurons with
    /// the texts measured in docs/pr-d.md, inside the free 10,000 a day.
    #[serde(default = "default_embedding_requests")]
    pub daily_requests: u32,
}

impl Default for Embedding {
    fn default() -> Self {
        Self {
            provider: default_embedding(),
            account_id: None,
            key_file: default_embedding_key(),
            daily_requests: default_embedding_requests(),
        }
    }
}

fn default_embedding_key() -> PathBuf {
    home_dir().join("CF_WORKERS_AI_KEY.md")
}
fn default_embedding_requests() -> u32 {
    200
}

#[derive(Debug, Clone, Deserialize)]
pub struct Summary {
    /// Language of observations and summaries, as written into the prompt. Never inferred.
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            language: default_language(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Provider {
    /// OpenAI-compatible chat completions with `response_format: json_schema`.
    Openai {
        name: String,
        base_url: String,
        /// File whose second line is the API key (owner convention: ~/X_KEY.md). None = no auth.
        #[serde(default)]
        key_file: Option<PathBuf>,
        model: String,
        #[serde(default = "default_budget")]
        daily_budget: u32,
        #[serde(default = "default_timeout")]
        timeout_s: u64,
        /// On 429 with a near reset, wait and retry once. Off where failed calls count
        /// against the quota (OpenRouter).
        #[serde(default = "default_true")]
        retry_429: bool,
        /// Extra request-body fields merged in (OpenRouter's `models` fallback, `provider`).
        #[serde(default)]
        extra: serde_json::Map<String, serde_json::Value>,
    },
    /// A subscription CLI run headless (`agy`, `claude`, `grok`, `codex`).
    Cli {
        name: String,
        /// Which CLI; decides the argument shape.
        cli: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default = "default_budget")]
        daily_budget: u32,
        #[serde(default = "default_cli_timeout")]
        timeout_s: u64,
    },
}

impl Provider {
    pub fn name(&self) -> &str {
        match self {
            Provider::Openai { name, .. } | Provider::Cli { name, .. } => name,
        }
    }
    pub fn daily_budget(&self) -> u32 {
        match self {
            Provider::Openai { daily_budget, .. } | Provider::Cli { daily_budget, .. } => {
                *daily_budget
            }
        }
    }
    pub fn retry_429(&self) -> bool {
        match self {
            Provider::Openai { retry_429, .. } => *retry_429,
            Provider::Cli { .. } => false,
        }
    }
}

fn default_budget() -> u32 {
    300
}
fn default_timeout() -> u64 {
    90
}
fn default_cli_timeout() -> u64 {
    180
}
fn default_true() -> bool {
    true
}
fn default_language() -> String {
    "Japanese".into()
}
fn default_embedding() -> String {
    "none".into()
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn openai(
    name: &str,
    base_url: &str,
    key: &str,
    model: &str,
    daily_budget: u32,
    retry_429: bool,
    extra: serde_json::Value,
) -> Provider {
    Provider::Openai {
        name: name.into(),
        base_url: base_url.into(),
        key_file: Some(home_dir().join(key)),
        model: model.into(),
        daily_budget,
        timeout_s: default_timeout(),
        retry_429,
        extra: extra.as_object().cloned().unwrap_or_default(),
    }
}

fn cli(name: &str, model: Option<&str>, daily_budget: u32) -> Provider {
    Provider::Cli {
        name: name.into(),
        cli: name.into(),
        model: model.map(Into::into),
        daily_budget,
        timeout_s: default_cli_timeout(),
    }
}

/// Default chain (docs/plan.md, verified by probes 2026-09-23): the two Groq strict-schema models
/// (separate 8k-TPM buckets) → claude → OpenRouter free → NIM → Mistral → codex. The
/// subscription CLIs run their cheap models (claude Haiku, codex gpt-6-luna), as claude-mem does
/// on the Claude subscription: curation spends the quota the owner codes with. agy is not in
/// it: headless agy has no switch that turns its tools off (it inherits the user's own tool
/// permissions and plugins), and a curator reads untrusted text. grok is not in it either: the
/// owner keeps the grok subscription out of curation (2026-09-25).
fn default_providers() -> Vec<Provider> {
    let groq = "https://api.groq.com/openai/v1";
    vec![
        openai(
            "groq",
            groq,
            "GROQ_API_KEY.md",
            "openai/gpt-oss-120b",
            800,
            true,
            serde_json::json!({}),
        ),
        openai(
            "groq-20b",
            groq,
            "GROQ_API_KEY.md",
            "openai/gpt-oss-20b",
            800,
            true,
            serde_json::json!({}),
        ),
        cli("claude", Some("haiku"), 200),
        openai(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "OPENROUTER_API_KEY.md",
            "nvidia/nemotron-3-super-120b-a12b:free",
            300,
            false,
            serde_json::json!({"models": ["qwen/qwen3.8-27b:free"], "provider": {"require_parameters": true}}),
        ),
        openai(
            "nim",
            "https://integrate.api.nvidia.com/v1",
            "NVIDIA_NIM_KEY.md",
            "nvidia/nemotron-3-super-120b-a12b",
            500,
            true,
            serde_json::json!({"max_tokens": 2000}),
        ),
        openai(
            "mistral",
            "https://api.mistral.ai/v1",
            "MISTRAL_API_KEY.md",
            "mistral-small-latest",
            300,
            true,
            serde_json::json!({}),
        ),
        cli("codex", Some("gpt-6-luna"), 200),
    ]
}

/// The `[embedding]` section for a search: a config.toml that does not load falls back to
/// full-text search with a line on stderr, like every other reason the hybrid cannot run.
pub fn search_embedding(home: &Path) -> Embedding {
    load(home).map(|c| c.embedding).unwrap_or_else(|e| {
        eprintln!("oboete: full-text search only: {e:#}");
        Embedding::default()
    })
}

pub fn load(home: &Path) -> Result<Config> {
    let path = home.join("config.toml");
    if !path.exists() {
        return Ok(Config {
            providers: default_providers(),
            summary: Summary::default(),
            embedding: Embedding::default(),
        });
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg: Config = toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    match cfg.embedding.provider.as_str() {
        "none" => {}
        "workers-ai" => anyhow::ensure!(
            cfg.embedding.account_id.is_some(),
            "{}: [embedding] provider = \"workers-ai\" needs account_id",
            path.display()
        ),
        other => anyhow::bail!(
            "{}: [embedding] provider = \"{other}\" does not exist yet; use \"none\" (full-text search) or \"workers-ai\"",
            path.display()
        ),
    }
    Ok(cfg)
}

/// Read an API key from the owner's key-file convention (token on line 2).
pub fn read_key(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read key file {}", path.display()))?;
    let key = text.lines().nth(1).map(str::trim).unwrap_or("");
    anyhow::ensure!(
        !key.is_empty(),
        "key file {} has no token on line 2",
        path.display()
    );
    Ok(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_toml_extra_fields_parse() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.providers.len(), 7);
        assert!(
            cfg.providers
                .iter()
                .all(|p| p.name() != "agy" && p.name() != "grok")
        );
        assert_eq!(cfg.summary.language, "Japanese");
        assert_eq!(cfg.embedding.provider, "none");
        let cfg: Config = toml::from_str(
            r#"
[summary]
language = "English"
[[providers]]
kind = "openai"
name = "ollama"
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
[providers.extra]
options = { num_ctx = 16000 }
[[providers]]
kind = "cli"
name = "claude"
cli = "claude"
model = "haiku"
"#,
        )
        .unwrap();
        assert_eq!(cfg.summary.language, "English");
        match &cfg.providers[0] {
            Provider::Openai {
                key_file, extra, ..
            } => {
                assert!(key_file.is_none());
                assert_eq!(extra["options"]["num_ctx"], 16000);
            }
            _ => panic!("expected openai"),
        }
        assert!(!cfg.providers[1].retry_429());
    }

    #[test]
    fn unknown_embedding_provider_is_refused_at_load() {
        let dir = std::env::temp_dir().join(format!("oboete-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[embedding]\nprovider = \"local\"\n",
        )
        .unwrap();
        let err = load(&dir).unwrap_err().to_string();
        assert!(err.contains("does not exist yet"), "{err}");
        std::fs::write(
            dir.join("config.toml"),
            "[embedding]\nprovider = \"none\"\n",
        )
        .unwrap();
        assert!(load(&dir).is_ok());
        // Workers AI needs the account; the token file and the daily cap have defaults.
        std::fs::write(
            dir.join("config.toml"),
            "[embedding]\nprovider = \"workers-ai\"\n",
        )
        .unwrap();
        let err = load(&dir).unwrap_err().to_string();
        assert!(err.contains("needs account_id"), "{err}");
        std::fs::write(
            dir.join("config.toml"),
            "[embedding]\nprovider = \"workers-ai\"\naccount_id = \"abc\"\n",
        )
        .unwrap();
        let cfg = load(&dir).unwrap();
        assert_eq!(cfg.embedding.daily_requests, 200);
        assert!(cfg.embedding.key_file.ends_with("CF_WORKERS_AI_KEY.md"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
