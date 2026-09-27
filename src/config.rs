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
    /// Where Gemini joins the chain; absent, it is not in it (the owner decides, free or paid).
    #[serde(default)]
    pub gemini: Option<GeminiPlace>,
}

/// `gemini = "before-subscriptions"` puts it just before the first subscription CLI (it spares
/// their quota and sees more windows); `"after-subscriptions"` at the end of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeminiPlace {
    BeforeSubscriptions,
    AfterSubscriptions,
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
        /// Extra request headers (OpenCode Go's `x-opencode-session`). Never a key: keys stay
        /// in `key_file`.
        #[serde(default)]
        headers: std::collections::BTreeMap<String, String>,
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
        headers: Default::default(),
    }
}

/// Gemini through its OpenAI-compatible endpoint, key in `~/GEMINI_API_KEY.md`
/// (docs/research/curator-providers-2026-09-27.md section 6). 30 calls a day keeps a paid key
/// under the USD 5 a month paid-API cap: Flash-Lite costs about USD 0.005 a window (10,000
/// tokens in, 1,500 out, USD 0.25 and 1.50 a million, checked 2026-09-27).
fn gemini() -> Provider {
    openai(
        "gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "GEMINI_API_KEY.md",
        "gemini-3.1-flash-lite",
        30,
        true,
        serde_json::json!({}),
    )
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

/// Default chain, the owner's order of 2026-09-27: free first (the three Groq strict-schema models
/// in separate 8k-TPM buckets, OpenRouter free, Mistral, then NIM, which never answered in the
/// owner's calls), then the flat-rate OpenCode Go, then the coding subscriptions, codex and
/// claude. The subscription CLIs run their cheap models (claude Haiku, codex gpt-6-luna), as
/// claude-mem does on the Claude subscription: curation spends the quota the owner codes with.
/// Gemini is not in it: its free tier uses the input to train and lets people review it
/// (ai.google.dev/gemini-api/terms); a paid Gemini key can be configured. agy is not in
/// it: headless agy has no switch that turns its tools off (it inherits the user's own tool
/// permissions and plugins), and a curator reads untrusted text. grok is not in it either: the
/// owner keeps the grok subscription out of curation (2026-09-25).
fn default_providers() -> Vec<Provider> {
    let groq = "https://api.groq.com/openai/v1";
    let mut opencode_go = openai(
        "opencode-go",
        "https://opencode.ai/zen/go/v1",
        "OPENCODE_API_KEY.md",
        "glm-5.3-flash",
        300,
        true,
        serde_json::json!({}),
    );
    if let Provider::Openai {
        headers, timeout_s, ..
    } = &mut opencode_go
    {
        // New console keys are refused without it (HTTP 400 MissingSessionID, 2026-09-26).
        headers.insert("x-opencode-session".into(), "oboete".into());
        // glm-5.3-flash reasons first: its answers took 63 s on average and 6 calls hit 90 s
        // (owner's store, 2026-09-22..26); a call cut off at the timeout may still be billed.
        *timeout_s = 150;
    }
    vec![
        openai(
            "groq",
            groq,
            "GROQ_API_KEY.md",
            "openai/gpt-oss-120b",
            800,
            true,
            // Reasoning tokens count against Groq's 200,000 tokens a day. Low effort cut them from
            // 533 to 9 (20b) and 386 to 75 (120b) on a short window, with valid JSON (2026-09-27).
            serde_json::json!({"reasoning_effort": "low"}),
        ),
        openai(
            "groq-20b",
            groq,
            "GROQ_API_KEY.md",
            "openai/gpt-oss-20b",
            800,
            true,
            // Reasoning tokens count against Groq's 200,000 tokens a day. Low effort cut them from
            // 533 to 9 (20b) and 386 to 75 (120b) on a short window, with valid JSON (2026-09-27).
            serde_json::json!({"reasoning_effort": "low"}),
        ),
        // A third free Groq model with its own 200,000 tokens a day, same key and recipient. Strict
        // schema, no reasoning: 0.9-1.2 s, valid JSON in Japanese on a 12,000-character window
        // (probe of 2026-09-27).
        openai(
            "groq-qwen",
            groq,
            "GROQ_API_KEY.md",
            "qwen/qwen3.8-27b",
            800,
            true,
            serde_json::json!({"reasoning_effort": "none"}),
        ),
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
            "mistral",
            "https://api.mistral.ai/v1",
            "MISTRAL_API_KEY.md",
            "mistral-small-latest",
            300,
            true,
            serde_json::json!({}),
        ),
        openai(
            "nim",
            "https://integrate.api.nvidia.com/v1",
            "NVIDIA_NIM_KEY.md",
            "nvidia/nemotron-3-super-120b-a12b",
            500,
            true,
            // Nemotron reasons before it answers, and the reasoning counts against max_tokens: on
            // a full-size window it stopped at 2000 tokens mid-JSON (finish_reason "length"), which
            // is every one of nim's failures in the owner's calls. Without reasoning it answered
            // valid JSON in about 5 s with 837 tokens (probe of 2026-09-27, a 16,000-character
            // synthetic window).
            serde_json::json!({"max_tokens": 4000, "chat_template_kwargs": {"enable_thinking": false}}),
        ),
        opencode_go,
        cli("codex", Some("gpt-6-luna"), 200),
        cli("claude", Some("haiku"), 200),
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
            gemini: None,
        });
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut cfg: Config = toml::from_str(&text)
        .map_err(|e| toml_error(&text, &e))
        .with_context(|| format!("parse {}", path.display()))?;
    if let Some(place) = cfg.gemini
        && !cfg.providers.iter().any(|p| p.name() == "gemini")
    {
        let at = match place {
            GeminiPlace::BeforeSubscriptions => cfg
                .providers
                .iter()
                .position(|p| matches!(p, Provider::Cli { .. }))
                .unwrap_or(cfg.providers.len()),
            GeminiPlace::AfterSubscriptions => cfg.providers.len(),
        };
        cfg.providers.insert(at, gemini());
    }
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

/// `[redaction]` (spec 1.5, 6.4): rules the user adds to the built-in ones, which cannot be
/// removed, and false positives to keep, each the SHA-256 (hex) of one exact value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    #[serde(default)]
    pub extra_rules: Vec<ExtraRule>,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

/// One user rule, in gitleaks' terms: the secret is `secret_group` (else the first non-empty
/// group, else the whole match); `keywords`, when given, gate the regex (case-insensitive);
/// `entropy` is the Shannon entropy a secret must exceed.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraRule {
    pub id: String,
    pub regex: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub entropy: Option<f64>,
    #[serde(default)]
    pub secret_group: Option<usize>,
}

/// `[capture]` (spec 1.5, 2.4): what is recorded.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    /// Off: a prompt is recorded as an event without its text.
    #[serde(default = "default_true")]
    pub store_prompts: bool,
    #[serde(default)]
    pub tool_output: ToolOutput,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            store_prompts: true,
            tool_output: ToolOutput::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolOutput {
    /// Whole, up to the size where head and tail are kept (spec 2.4).
    #[default]
    Full,
    /// Always only its head and tail (`capture::HEAD_TAIL_BYTES`).
    HeadTail,
}

/// The two tables capture reads, and nothing else of config.toml: a mistake inside
/// `[[providers]]` must not stop recording. Missing file or tables = defaults. The other tables
/// are only named: a table no version reads (`[redactions]`) is an error, not settings that
/// silently do nothing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureConfig {
    #[serde(default)]
    pub redaction: Redaction,
    #[serde(default)]
    pub capture: Capture,
    #[serde(default, rename = "providers")]
    _providers: serde::de::IgnoredAny,
    #[serde(default, rename = "summary")]
    _summary: serde::de::IgnoredAny,
    #[serde(default, rename = "embedding")]
    _embedding: serde::de::IgnoredAny,
    #[serde(default, rename = "backup")]
    _backup: serde::de::IgnoredAny,
}

pub fn load_capture(home: &Path) -> Result<CaptureConfig> {
    let path = home.join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    parse_capture(text.as_deref()).with_context(|| format!("parse {}", path.display()))
}

/// `load_capture` on the file's text (`None`: no file).
pub fn parse_capture(text: Option<&str>) -> Result<CaptureConfig> {
    let Some(text) = text else {
        return Ok(CaptureConfig::default());
    };
    toml::from_str(text).map_err(|e| toml_error(text, &e))
}

/// A TOML error in config.toml as it may be printed: by hooks to stderr, by doctor, by a running
/// `oboete mcp`. Only its line: the error's own text can quote the value on that line (a serde
/// message quotes a value of the wrong type, the display quotes the line), and `[redaction]`
/// holds values the user means to hide.
fn toml_error(text: &str, e: &toml::de::Error) -> anyhow::Error {
    let line = e
        .span()
        .and_then(|s| text.get(..s.start))
        .map_or(0, |b| b.matches('\n').count() + 1);
    anyhow::anyhow!(
        "line {line} is not valid here (the details are not shown: they could quote a value to hide)"
    )
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
        for p in &cfg.providers {
            if let Provider::Openai {
                name,
                extra,
                timeout_s,
                ..
            } = p
            {
                let effort = extra.get("reasoning_effort").and_then(|v| v.as_str());
                let want = match name.as_str() {
                    "groq" | "groq-20b" => Some("low"),
                    "groq-qwen" => Some("none"),
                    _ => None,
                };
                assert_eq!(effort, want, "{name}");
                let want = if name == "opencode-go" { 150 } else { 90 };
                assert_eq!(*timeout_s, want, "{name}");
            }
        }
        // The owner's order (2026-09-27): free, then OpenCode Go, then the subscription CLIs.
        let names: Vec<_> = cfg.providers.iter().map(Provider::name).collect();
        assert_eq!(
            names,
            [
                "groq",
                "groq-20b",
                "groq-qwen",
                "openrouter",
                "mistral",
                "nim",
                "opencode-go",
                "codex",
                "claude"
            ]
        );
        match &cfg.providers[5] {
            Provider::Openai { extra, .. } => {
                assert_eq!(extra["chat_template_kwargs"]["enable_thinking"], false)
            }
            _ => panic!("expected nim"),
        }
        match &cfg.providers[6] {
            Provider::Openai { headers, .. } => {
                assert_eq!(headers["x-opencode-session"], "oboete")
            }
            _ => panic!("expected opencode-go"),
        }
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
headers = { "x-opencode-session" = "oboete" }
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
                key_file,
                extra,
                headers,
                ..
            } => {
                assert!(key_file.is_none());
                assert_eq!(extra["options"]["num_ctx"], 16000);
                assert_eq!(headers["x-opencode-session"], "oboete");
            }
            _ => panic!("expected openai"),
        }
        assert!(!cfg.providers[1].retry_429());
    }

    #[test]
    fn gemini_joins_the_chain_only_where_the_owner_puts_it() {
        let dir = std::env::temp_dir().join(format!("oboete-gemini-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let names = |toml: &str| {
            std::fs::write(dir.join("config.toml"), toml).unwrap();
            load(&dir)
                .unwrap()
                .providers
                .iter()
                .map(|p| p.name().to_owned())
                .collect::<Vec<_>>()
        };
        assert!(!names("").contains(&"gemini".to_owned()));
        let before = names("gemini = \"before-subscriptions\"\n");
        let at = before.iter().position(|n| n == "gemini").unwrap();
        assert_eq!(before[at - 1], "opencode-go");
        assert_eq!(before[at + 1], "codex");
        let after = names("gemini = \"after-subscriptions\"\n");
        assert_eq!(after.last().map(String::as_str), Some("gemini"));
        // An entry of the owner's own named gemini is not doubled.
        let own = names(
            "gemini = \"after-subscriptions\"\n[[providers]]\nkind = \"openai\"\nname = \"gemini\"\nbase_url = \"https://example.test/v1\"\nmodel = \"m\"\n",
        );
        assert_eq!(own, ["gemini"]);
        std::fs::write(dir.join("config.toml"), "gemini = \"first\"\n").unwrap();
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
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
