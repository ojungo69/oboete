//! `<home>/config.toml` — provider chain. Missing file = built-in default chain.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_providers")]
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Provider {
    /// OpenAI-compatible chat completions with `response_format: json_schema`.
    Openai {
        name: String,
        base_url: String,
        /// File whose second line is the API key (owner convention: ~/X_KEY.md).
        key_file: PathBuf,
        model: String,
        #[serde(default = "default_budget")]
        daily_budget: u32,
        #[serde(default = "default_timeout")]
        timeout_s: u64,
    },
    /// A subscription CLI run headless (`agy`, `claude`, `grok`, `codex`).
    Cli {
        name: String,
        /// Which CLI; decides the argument shape.
        cli: String,
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

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Default chain (docs/plan.md): Groq free → agy. The rest of the chain lands in M1.
fn default_providers() -> Vec<Provider> {
    vec![
        Provider::Openai {
            name: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            key_file: home_dir().join("GROQ_API_KEY.md"),
            model: "openai/gpt-oss-120b".into(),
            daily_budget: 800,
            timeout_s: 90,
        },
        Provider::Cli {
            name: "agy".into(),
            cli: "agy".into(),
            daily_budget: 200,
            timeout_s: 180,
        },
    ]
}

pub fn load(home: &Path) -> Result<Config> {
    let path = home.join("config.toml");
    if !path.exists() {
        return Ok(Config {
            providers: default_providers(),
        });
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
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
