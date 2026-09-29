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
    /// What every paid entry together may spend in a calendar month (owner decision 5: USD 5).
    #[serde(default = "default_paid_usd_per_month", deserialize_with = "usd")]
    pub paid_usd_per_month: f64,
    /// `[chain]`: the user's changes to the chain by entry name, applied by `load()` (#94).
    #[serde(default)]
    pub chain: ChainOverlay,
    /// What `load()` ignored or doubts in the chain, one line each, for doctor.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// `[chain]` (#94): changes to the chain by entry name, over the built-in chain or the user's
/// `[[providers]]`, so later changes to the defaults still reach every value the user did not
/// set. Unknown keys are refused: a misspelled `off` would keep calling an entry turned off.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainOverlay {
    /// Entries put first, in this order; the rest keep their place after them.
    #[serde(default)]
    pub order: Vec<String>,
    /// Entries turned off: kept in the list, never called.
    #[serde(default)]
    pub off: Vec<String>,
    #[serde(default)]
    pub daily_budget: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    pub timeout_s: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub model: std::collections::BTreeMap<String, String>,
}

impl ChainOverlay {
    /// Whether `[chain] off` names the entry: it stays in `Config::providers` for doctor, and
    /// `load_chain` leaves it out of the chain.
    pub fn turns_off(&self, name: &str) -> bool {
        self.off.iter().any(|n| n == name)
    }
}

fn default_paid_usd_per_month() -> f64 {
    5.0
}

/// A USD amount of the config: finite and 0 or more. A negative price would make a paid entry
/// free or earn it credit, and NaN compares false with every cap.
fn usd<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if v.is_finite() && v >= 0.0 {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(
            "a USD amount is a number, 0 or more",
        ))
    }
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
    /// Whether the worker curates windows (milestone 3). Off until the cut-over (spec 7.5): only
    /// a home that asks for it sends anything to a provider.
    #[serde(default)]
    pub curate: bool,
    /// A window's size in estimated tokens (docs/milestone-3-plan.md D8).
    #[serde(default = "default_window_tokens")]
    pub window_tokens: u32,
    /// How long after the owner's last hook record a window that reaches the last record still
    /// waits for more (D9). At most 30: a longer value is read as 30 (D10, the longest the worker
    /// stays up for a wait).
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: u32,
    /// Task 12's shrink: a window shows a tool's input short and a long output as its head and
    /// tail, so it holds more of a session (docs/spike/m3-dev.md). Off until measurement shows the
    /// input shrinks by 30% or more and recall drops by 0.02 or less (spec 3.5).
    #[serde(default)]
    pub shrink: bool,
}

impl Summary {
    /// How the curation phase cuts its windows.
    pub fn cut(&self) -> crate::curate::Cut {
        crate::curate::Cut {
            tokens: self.window_tokens,
            shrink: self.shrink,
        }
    }
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            language: default_language(),
            curate: false,
            window_tokens: default_window_tokens(),
            idle_minutes: default_idle_minutes(),
            shrink: false,
        }
    }
}

fn default_window_tokens() -> u32 {
    crate::curate::WINDOW_TOKENS
}
fn default_idle_minutes() -> u32 {
    10
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
        /// Unset: 300 calls a day, none for a subscription (owner decision 30), and a fifth of its
        /// key's own limit for an OpenRouter `:free` entry, 10 until that is read (#238).
        #[serde(default)]
        daily_budget: Option<u32>,
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
        #[serde(default)]
        limits: Limits,
        /// Paid by a subscription the owner codes with (OpenCode Go, owner decision 25): its tier
        /// is a subscription's and it has no daily cap unless it sets one, as a subscription
        /// CLI (spec 1.4).
        #[serde(default)]
        subscription: bool,
    },
    /// A subscription CLI run headless (`agy`, `claude`, `grok`, `codex`).
    Cli {
        name: String,
        /// Which CLI; decides the argument shape.
        cli: String,
        #[serde(default)]
        model: Option<String>,
        /// No cap by default: a subscription has no cap of calls a day (owner decision 30).
        #[serde(default = "no_daily_cap")]
        daily_budget: u32,
        #[serde(default = "default_cli_timeout")]
        timeout_s: u64,
        #[serde(default)]
        limits: Limits,
    },
}

/// What one entry may take (docs/milestone-3-plan.md Task 4), as `limits = { ... }`. With prices
/// it is a paid entry, inside `paid_usd_per_month` with every other paid entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// The provider's ceiling for one request, in tokens (Groq free: 8,000). A larger request is
    /// skipped before it is sent.
    #[serde(default)]
    pub max_request_tokens: Option<u32>,
    /// Tokens (prompt plus completion) a day.
    #[serde(default)]
    pub daily_tokens: Option<u64>,
    /// USD per million tokens, in and out.
    #[serde(default, deserialize_with = "usd")]
    pub usd_per_mtok_in: f64,
    #[serde(default, deserialize_with = "usd")]
    pub usd_per_mtok_out: f64,
    /// The largest answer a paid entry may send, which its request asks for as `max_tokens`
    /// and its admission counts as spent.
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_request_tokens: None,
            daily_tokens: None,
            usd_per_mtok_in: 0.0,
            usd_per_mtok_out: 0.0,
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

impl Limits {
    pub fn is_paid(&self) -> bool {
        self.usd_per_mtok_in > 0.0 || self.usd_per_mtok_out > 0.0
    }
    /// USD for `prompt` tokens in and `completion` tokens out.
    pub fn usd(&self, prompt: f64, completion: f64) -> f64 {
        (prompt * self.usd_per_mtok_in + completion * self.usd_per_mtok_out) / 1e6
    }
}

fn default_max_output_tokens() -> u32 {
    4000
}

impl Provider {
    /// The output tokens a request reserves on top of its prompt: `max_tokens` or
    /// `max_completion_tokens` in `extra`, at most `max_output_tokens` on a paid entry (as
    /// `provider::call` sends it), or 0 when it names none.
    pub fn declared_output(&self) -> u32 {
        let Provider::Openai { extra, limits, .. } = self else {
            return 0;
        };
        let declared = ["max_tokens", "max_completion_tokens"]
            .iter()
            .filter_map(|k| extra.get(*k)?.as_u64())
            .max();
        let cap = u64::from(limits.max_output_tokens);
        let n = match (declared, limits.is_paid()) {
            (Some(n), true) => n.min(cap),
            (None, true) => cap,
            (Some(n), false) => n,
            (None, false) => 0,
        };
        u32::try_from(n).unwrap_or(u32::MAX)
    }
    pub fn name(&self) -> &str {
        match self {
            Provider::Openai { name, .. } | Provider::Cli { name, .. } => name,
        }
    }
    /// Whether its calls a day are a fifth of what its own key may request (#238): an entry of
    /// OpenRouter's `:free` models with no `daily_budget` of the owner's, and not a subscription
    /// (which has no cap). OpenRouter gives each account its own daily limit on them, by the
    /// credits it has bought.
    pub fn budget_from_key(&self) -> bool {
        matches!(self, Provider::Openai {
                base_url, model, daily_budget: None, subscription: false, ..
            } if openrouter_free(base_url, model))
    }
    pub fn daily_budget(&self) -> u32 {
        match self {
            Provider::Openai {
                daily_budget,
                subscription,
                ..
            } => daily_budget.unwrap_or(if *subscription {
                no_daily_cap()
            } else if self.budget_from_key() {
                OPENROUTER_FREE_BUDGET
            } else {
                default_budget()
            }),
            Provider::Cli { daily_budget, .. } => *daily_budget,
        }
    }
    pub fn limits(&self) -> &Limits {
        match self {
            Provider::Openai { limits, .. } | Provider::Cli { limits, .. } => limits,
        }
    }
    /// Its tier (spec 1.4): 3 paid, 2 subscription, 1 free or local. The highest tier's
    /// derivation of a claim is the active one (MUST-M18).
    pub fn tier(&self) -> i64 {
        if self.limits().is_paid() {
            3
        } else if self.subscription() {
            2
        } else {
            1
        }
    }
    /// Whether a call spends a subscription the owner codes with: every CLI, and an API entry
    /// marked so.
    pub fn subscription(&self) -> bool {
        match self {
            Provider::Openai { subscription, .. } => *subscription,
            Provider::Cli { .. } => true,
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
/// OpenRouter's API.
pub const OPENROUTER: &str = "https://openrouter.ai/api/v1";
/// A fifth of the 50 requests a day that OpenRouter gives all `:free` models of an account that
/// bought less than 10 credits (1,000 after that; openrouter.ai/docs/api/reference/limits,
/// 2026-09-29): the cost line (spec 8.2) for any account, until its key's own limit is read.
const OPENROUTER_FREE_BUDGET: u32 = 10;
/// A `daily_budget` no day reaches. A subscription stops at its own limits instead: a cooldown
/// until their reset (spec 3.1, Claude decision C1).
fn no_daily_cap() -> u32 {
    u32::MAX
}
fn default_timeout() -> u64 {
    90
}
/// A subscription CLI's answer, at most. claude haiku thinks even at `--effort low`: on the dev
/// windows of milestone 3 (212 calls, 2026-09-28) it took 101 s at the median, 187 s at p99 and
/// 200 s at most, and 6 calls passed 180 s, each of which cooled the entry for ten minutes (#193).
fn default_cli_timeout() -> u64 {
    300
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
    daily_budget: Option<u32>,
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
        limits: Limits::default(),
        subscription: false,
    }
}

/// Gemini through its OpenAI-compatible endpoint, key in `~/GEMINI_API_KEY.md`
/// (docs/research/curator-providers-2026-09-27.md section 6). 30 calls a day keeps a paid key
/// under the USD 5 a month paid-API cap: Flash-Lite costs about USD 0.005 a window (10,000
/// tokens in, 1,500 out, USD 0.25 and 1.50 a million, checked 2026-09-27).
fn gemini() -> Provider {
    let mut p = openai(
        "gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "GEMINI_API_KEY.md",
        "gemini-3.1-flash-lite",
        Some(30),
        true,
        serde_json::json!({}),
    );
    // Paid prices (ai.google.dev/gemini-api/docs/pricing, checked 2026-09-27): counted even on a
    // free key, which only makes the cap stricter.
    if let Provider::Openai { limits, .. } = &mut p {
        limits.usd_per_mtok_in = 0.25;
        limits.usd_per_mtok_out = 1.50;
    }
    p
}

fn cli(name: &str, model: Option<&str>) -> Provider {
    Provider::Cli {
        name: name.into(),
        cli: name.into(),
        model: model.map(Into::into),
        daily_budget: no_daily_cap(),
        timeout_s: default_cli_timeout(),
        limits: Limits::default(),
    }
}

/// Default chain, the owner's order of 2026-09-27: free first (the three Groq strict-schema models
/// in separate 8k-TPM buckets, OpenRouter free, then NIM, which never answered in the owner's
/// calls), then the flat-rate OpenCode Go, then the coding subscriptions, codex and claude.
/// Mistral is not in it (owner, 2026-09-29): the owner's workspace allows no requests a minute,
/// so the entry only spent a refused call each time the chain reached it (#233); a key whose free
/// plan is on can be configured. The subscription CLIs run their cheap models (claude Haiku, codex gpt-6-luna), as
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
        // No cap of calls a day (owner decision 30): the owner's Go account does not fall back to
        // a paid Zen balance past its limits (the owner, 2026-09-28).
        Some(no_daily_cap()),
        true,
        serde_json::json!({}),
    );
    if let Provider::Openai {
        headers,
        timeout_s,
        subscription,
        ..
    } = &mut opencode_go
    {
        *subscription = true;
        // New console keys are refused without it (HTTP 400 MissingSessionID, 2026-09-26).
        headers.insert("x-opencode-session".into(), "oboete".into());
        // glm-5.3-flash reasons first: its answers took 63 s on average and 6 calls hit 90 s
        // (owner's store, 2026-09-22..26); a call cut off at the timeout may still be billed.
        *timeout_s = 150;
    }
    let mut nim = openai(
        "nim",
        "https://integrate.api.nvidia.com/v1",
        "NVIDIA_NIM_KEY.md",
        // NIM retires nemotron-3-super on 2026-10-03: its answers carry that `deprecation` (#233).
        "nvidia/nemotron-3-ultra-550b-a55b",
        Some(500),
        true,
        // Nemotron reasons before it answers, and the reasoning counts against max_tokens: on a
        // full-size window super stopped at 2000 tokens mid-JSON (finish_reason "length"), which
        // is every one of nim's failures in the owner's calls. Without reasoning it answered
        // valid JSON in about 5 s with 837 tokens (probe of 2026-09-27, a 16,000-character
        // synthetic window).
        serde_json::json!({"max_tokens": 4000, "chat_template_kwargs": {"enable_thinking": false}}),
    );
    // ultra answered 24 of 24 calls on six windows of main's prompt in valid JSON (2026-09-29,
    // #233): 27 s at the median, 75 s at p95, 96 s at most. Twice the p95 keeps it within half
    // the timeout, the probe's line.
    if let Provider::Openai { timeout_s, .. } = &mut nim {
        *timeout_s = 160;
    }
    let openrouter = openai(
        "openrouter",
        OPENROUTER,
        "OPENROUTER_API_KEY.md",
        "nvidia/nemotron-3-super-120b-a12b:free",
        // Unset: its calls a day are a fifth of what its key may request, and a fifth of the
        // least OpenRouter gives any account until that is read (#238).
        None,
        false,
        serde_json::json!({"models": ["qwen/qwen3.8-27b:free"], "provider": {"require_parameters": true}}),
    );
    let mut chain = vec![
        openai(
            "groq",
            groq,
            "GROQ_API_KEY.md",
            "openai/gpt-oss-120b",
            // A fifth of Groq free's 1,000 requests a day per model: the cost line (spec 8.2).
            Some(200),
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
            Some(200),
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
            Some(200),
            true,
            serde_json::json!({"reasoning_effort": "none"}),
        ),
        openrouter,
        nim,
        opencode_go,
        cli("codex", Some("gpt-6-luna")),
        cli("claude", Some("haiku")),
    ];
    // Groq free refuses a request over 8,000 tokens (its tokens-a-minute limit is also a ceiling
    // per request; docs/research/curator-providers-2026-09-27.md section 3). The curator takes a
    // fifth of each model's 200,000 tokens a day, in any 24 hours (the cost line, spec 8.2; the
    // limits page, console.groq.com/docs/rate-limits, 2026-09-29).
    for p in &mut chain {
        if let Provider::Openai {
            base_url, limits, ..
        } = p
            && base_url == groq
        {
            limits.max_request_tokens = Some(8000);
            limits.daily_tokens = Some(40_000);
        }
    }
    chain
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
            paid_usd_per_month: default_paid_usd_per_month(),
            chain: ChainOverlay::default(),
            warnings: Vec::new(),
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
    overlay(&mut cfg);
    // A CLI's subscription pays for it, and its answer has no cap to price; prices are for HTTP.
    if let Some(p) = cfg
        .providers
        .iter()
        .find(|p| matches!(p, Provider::Cli { .. }) && p.limits().is_paid())
    {
        anyhow::bail!(
            "{}: provider \"{}\" is a CLI: its limits take no USD prices",
            path.display(),
            p.name()
        );
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

/// `load()` for the callers that build a chain (the worker, `recurate`): the entries turned off
/// are not in it.
pub fn load_chain(home: &Path) -> Result<Config> {
    let mut cfg = load(home)?;
    let Config {
        providers, chain, ..
    } = &mut cfg;
    providers.retain(|p| !chain.turns_off(p.name()));
    Ok(cfg)
}

/// `[chain]` over the chain as built so far. A name that matches no entry is ignored with a
/// warning, so a default renamed upstream does not stop curation; nothing here fails `load()`,
/// which would stop it.
fn overlay(cfg: &mut Config) {
    let Config {
        providers,
        summary,
        chain,
        warnings,
        ..
    } = cfg;
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for p in providers.iter() {
        *counts.entry(p.name().to_owned()).or_default() += 1;
    }
    let names = |name: &String| {
        chain.order.contains(name)
            || chain.off.contains(name)
            || chain.daily_budget.contains_key(name)
            || chain.timeout_s.contains_key(name)
            || chain.model.contains_key(name)
    };
    for (name, n) in &counts {
        // Only where `[chain]` names it: otherwise the repeat changes nothing.
        if *n > 1 && names(name) {
            warnings.push(format!(
                "{n} chain entries are named \"{name}\": [chain] changes each of them"
            ));
        }
    }
    let named = (chain.order.iter().map(|n| ("order", n)))
        .chain(chain.off.iter().map(|n| ("off", n)))
        .chain(chain.daily_budget.keys().map(|n| ("daily_budget", n)))
        .chain(chain.timeout_s.keys().map(|n| ("timeout_s", n)))
        .chain(chain.model.keys().map(|n| ("model", n)));
    for (key, name) in named {
        if !counts.contains_key(name) {
            warnings.push(format!(
                "[chain] {key}: no chain entry is named \"{name}\", so it is ignored"
            ));
        }
    }
    // Stable: the entries `order` does not name keep their order after the ones it does.
    providers.sort_by_key(|p| {
        (chain.order.iter())
            .position(|n| n == p.name())
            .unwrap_or(usize::MAX)
    });
    for p in providers.iter_mut() {
        let name = p.name().to_owned();
        let set = chain.model.get(&name);
        let (Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. }) = p;
        if let Some(&s) = chain.timeout_s.get(&name) {
            *timeout_s = s;
        }
        match p {
            Provider::Openai {
                base_url,
                model,
                daily_budget,
                limits,
                ..
            } => {
                if let Some(&n) = chain.daily_budget.get(&name) {
                    *daily_budget = Some(n);
                }
                // Only a model whose price the entry knows: an entry's prices are its model's,
                // and OpenRouter bills a model that is not `:free` outside the monthly USD cap
                // (#94, revision 1; cubic on #94).
                if let Some(m) = set {
                    if limits.is_paid() {
                        warnings.push(format!(
                            "[chain] model: \"{name}\" keeps its model, as its prices are its model's: set another model with its prices in [[providers]]"
                        ));
                    } else if openrouter_free(base_url, model) && !is_free(m) {
                        warnings.push(format!(
                            "[chain] model: \"{m}\" is not a :free model, and OpenRouter may bill it outside the monthly USD cap, so \"{name}\" keeps its model"
                        ));
                    } else {
                        model.clone_from(m);
                    }
                }
            }
            Provider::Cli {
                model,
                daily_budget,
                ..
            } => {
                if let Some(&n) = chain.daily_budget.get(&name) {
                    *daily_budget = n;
                }
                if let Some(m) = set {
                    *model = Some(m.clone());
                }
            }
        }
    }
    if summary.curate
        && !providers.is_empty()
        && providers.iter().all(|p| chain.turns_off(p.name()))
    {
        warnings.push(
            "every chain entry is off, so nothing is curated: to stop curation, set [summary] curate = false instead".into(),
        );
    }
}

/// An OpenRouter `:free` model, by the URL as written in any case (#238).
fn openrouter_free(base_url: &str, model: &str) -> bool {
    base_url
        .trim()
        .trim_end_matches('/')
        .eq_ignore_ascii_case(OPENROUTER)
        && is_free(model)
}

fn is_free(model: &str) -> bool {
    model.to_ascii_lowercase().ends_with(":free")
}

/// Doctor's line under a chain entry (#94): off, its calls a day and timeout as they apply, and
/// its model when `[chain]` sets it. A budget from the entry's key is on the entry's own line
/// (#238), not here.
pub fn doctor_line(p: &Provider, chain: &ChainOverlay) -> String {
    let (timeout_s, model) = match p {
        Provider::Openai {
            timeout_s, model, ..
        } => (*timeout_s, Some(model.as_str())),
        Provider::Cli {
            timeout_s, model, ..
        } => (*timeout_s, model.as_deref()),
    };
    let mut parts = Vec::new();
    if chain.turns_off(p.name()) {
        parts.push("off".to_owned());
    }
    if !p.budget_from_key() {
        parts.push(match p.daily_budget() {
            n if n == no_daily_cap() => "no cap of calls a day".to_owned(),
            n => format!("{n} calls a day"),
        });
    }
    parts.push(format!("timeout {timeout_s} s"));
    if let Some(m) = model.filter(|m| chain.model.get(p.name()).is_some_and(|c| c == m)) {
        parts.push(format!("model {m} (set in [chain])"));
    }
    format!("    {}", parts.join(", "))
}

/// `[inject]` (#94): the manifest the hooks inject, wherever an agent takes it: at a session
/// start, at the first event of an agent without one (grok, agy), after a Cursor compaction, and
/// through `oboete inject` (OpenCode, pi).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Inject {
    pub session_start: bool,
    /// The manifest's size in characters, 1,000 to 6,000: it is stored rendered at 6,000.
    pub session_start_chars: usize,
}

impl Default for Inject {
    fn default() -> Self {
        Self {
            session_start: true,
            session_start_chars: crate::consumer::manifest::CAP,
        }
    }
}

/// `home`'s `[inject]`, by its own parse, so a mistake elsewhere in the file stops neither
/// recording nor this. A mistake in `[inject]` itself is an error, which injects nothing: the
/// defaults would show the manifest to a user who turned it off (cubic on #94).
pub fn inject(home: &Path) -> Result<Inject> {
    let path = home.join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_inject(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Inject::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn parse_inject(text: &str) -> Result<Inject> {
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        inject: Inject,
    }
    let i = toml::from_str::<File>(text)
        .map_err(|e| toml_error(text, &e))?
        .inject;
    anyhow::ensure!(
        (1_000..=crate::consumer::manifest::CAP).contains(&i.session_start_chars),
        "[inject] session_start_chars is 1000 to {}",
        crate::consumer::manifest::CAP
    );
    Ok(i)
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
    // `Config`'s top-level keys: each one a user sets must not stop recording.
    #[serde(default, rename = "gemini")]
    _gemini: serde::de::IgnoredAny,
    #[serde(default, rename = "paid_usd_per_month")]
    _paid_usd_per_month: serde::de::IgnoredAny,
    #[serde(default, rename = "chain")]
    _chain: serde::de::IgnoredAny,
    #[serde(default, rename = "inject")]
    _inject: serde::de::IgnoredAny,
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

    /// #238: the default OpenRouter entry takes its budget from its key, 10 until that is read.
    /// An entry with the owner's own budget, a subscription, a model that is not free, or another
    /// URL does not.
    #[test]
    fn only_an_openrouter_free_entry_without_a_budget_takes_its_keys() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(dir.path()).unwrap();
        let openrouter = cfg.providers.iter().find(|p| p.name() == "openrouter");
        assert!(openrouter.is_some_and(Provider::budget_from_key));
        assert_eq!(openrouter.map(Provider::daily_budget), Some(10));
        let entry = |rest: &str| {
            toml::from_str::<Provider>(&format!("kind = \"openai\"\nname = \"o\"\n{rest}")).unwrap()
        };
        let free = "base_url = \"https://openrouter.ai/api/v1/\"\nmodel = \"m:free\"\n";
        assert!(entry(free).budget_from_key());
        assert_eq!(entry(free).daily_budget(), 10);
        // The URL as written, in any case, and a model's suffix in any case.
        assert!(
            entry("base_url = \" https://OpenRouter.ai/api/v1/ \"\nmodel = \"m:FREE\"\n")
                .budget_from_key()
        );
        let own = entry(&format!("{free}daily_budget = 30\n"));
        assert!(!own.budget_from_key());
        assert_eq!(own.daily_budget(), 30);
        let subscription = entry(&format!("{free}subscription = true\n"));
        assert!(!subscription.budget_from_key());
        assert_eq!(subscription.daily_budget(), u32::MAX);
        assert!(
            !entry("base_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m\"\n")
                .budget_from_key()
        );
        let elsewhere = entry("base_url = \"https://example.com/api/v1\"\nmodel = \"m:free\"\n");
        assert!(!elsewhere.budget_from_key());
        assert_eq!(elsewhere.daily_budget(), 300);
    }

    #[test]
    fn every_key_config_reads_leaves_capture_working() {
        let text = r#"
gemini = "before-subscriptions"
paid_usd_per_month = 2.5
[summary]
[embedding]
[backup]
[redaction]
[capture]
[inject]
session_start = false
[chain]
off = ["codex"]
"#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.paid_usd_per_month, 2.5);
        for bad in ["-1.0", "nan", "inf"] {
            assert!(toml::from_str::<Config>(&format!("paid_usd_per_month = {bad}")).is_err());
            assert!(toml::from_str::<Limits>(&format!("usd_per_mtok_in = {bad}")).is_err());
            assert!(toml::from_str::<Limits>(&format!("usd_per_mtok_out = {bad}")).is_err());
        }
        parse_capture(Some(text)).unwrap();
        // A table no version reads is still an error.
        assert!(parse_capture(Some("[redactions]\n")).is_err());
    }

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
                let want = match name.as_str() {
                    "opencode-go" => 150,
                    "nim" => 160,
                    _ => 90,
                };
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
                "nim",
                "opencode-go",
                "codex",
                "claude"
            ]
        );
        match &cfg.providers[4] {
            Provider::Openai { extra, .. } => {
                assert_eq!(extra["chat_template_kwargs"]["enable_thinking"], false)
            }
            _ => panic!("expected nim"),
        }
        match &cfg.providers[5] {
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let names = |toml: &str| {
            std::fs::write(dir.join("config.toml"), toml).unwrap();
            load(dir)
                .unwrap()
                .providers
                .iter()
                .map(|p| p.name().to_owned())
                .collect::<Vec<_>>()
        };
        assert!(!names("").contains(&"gemini".to_owned()));
        // The subscriptions: every CLI, and OpenCode Go (owner decision 25).
        let subscriptions: Vec<String> = load(dir)
            .unwrap()
            .providers
            .iter()
            .filter(|p| p.subscription())
            .map(|p| p.name().to_owned())
            .collect();
        assert_eq!(subscriptions, ["opencode-go", "codex", "claude"]);
        // The default chain's subscriptions have no cap of calls a day, nor has one the owner
        // writes without one; another API entry has 300 (owner decision 30).
        let caps: Vec<u32> = load(dir)
            .unwrap()
            .providers
            .iter()
            .filter(|p| p.subscription())
            .map(Provider::daily_budget)
            .collect();
        assert_eq!(caps, [u32::MAX; 3]);
        let own = |entry: &str| toml::from_str::<Provider>(entry).unwrap().daily_budget();
        assert_eq!(
            own("kind = \"cli\"\nname = \"codex\"\ncli = \"codex\"\n"),
            u32::MAX
        );
        let api = "kind = \"openai\"\nname = \"go\"\nbase_url = \"u\"\nmodel = \"m\"\n";
        assert_eq!(own(&format!("{api}subscription = true\n")), u32::MAX);
        assert_eq!(
            own(&format!("{api}subscription = true\ndaily_budget = 5\n")),
            5
        );
        assert_eq!(own(api), 300);
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
        assert!(load(dir).is_err());
        // A CLI entry takes no prices: its subscription pays, and its answer has no cap.
        let cli = "[[providers]]\nkind = \"cli\"\nname = \"claude\"\ncli = \"claude\"\n";
        std::fs::write(dir.join("config.toml"), cli).unwrap();
        assert!(load(dir).is_ok());
        std::fs::write(
            dir.join("config.toml"),
            format!("{cli}limits = {{ usd_per_mtok_out = 1.0 }}\n"),
        )
        .unwrap();
        assert!(load(dir).is_err());
    }

    fn load_text(text: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        load(dir.path()).unwrap()
    }

    fn find<'a>(cfg: &'a Config, name: &str) -> &'a Provider {
        cfg.providers.iter().find(|p| p.name() == name).unwrap()
    }

    fn timeout(p: &Provider) -> u64 {
        match p {
            Provider::Openai { timeout_s, .. } | Provider::Cli { timeout_s, .. } => *timeout_s,
        }
    }

    fn model(p: &Provider) -> Option<&str> {
        match p {
            Provider::Openai { model, .. } => Some(model),
            Provider::Cli { model, .. } => model.as_deref(),
        }
    }

    /// #94: `[chain]` changes the built-in chain by name, and every entry and value it does not
    /// name stays the built-in one, so later changes to the defaults still reach them.
    #[test]
    fn chain_changes_the_built_in_chain_by_name() {
        let cfg = load_text(
            r#"
[chain]
order = ["claude", "groq-qwen"]
off = ["codex", "groq-20b"]
daily_budget = { groq = 50, claude = 7 }
timeout_s = { nim = 200, claude = 100 }
model = { claude = "sonnet", groq = "openai/gpt-oss-20b" }
"#,
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        let names: Vec<_> = cfg.providers.iter().map(Provider::name).collect();
        assert_eq!(
            names,
            [
                "claude",
                "groq-qwen",
                "groq",
                "groq-20b",
                "openrouter",
                "nim",
                "opencode-go",
                "codex"
            ]
        );
        let off: Vec<_> = (cfg.providers.iter())
            .filter(|p| cfg.chain.turns_off(p.name()))
            .map(Provider::name)
            .collect();
        assert_eq!(off, ["groq-20b", "codex"]);
        let claude = find(&cfg, "claude");
        assert_eq!(
            (claude.daily_budget(), timeout(claude), model(claude)),
            (7, 100, Some("sonnet"))
        );
        let groq = find(&cfg, "groq");
        assert_eq!(
            (groq.daily_budget(), timeout(groq), model(groq)),
            (50, 90, Some("openai/gpt-oss-20b"))
        );
        assert_eq!(timeout(find(&cfg, "nim")), 200);
        // Untouched entries are the built-in ones, extra, headers and timeouts included.
        for p in default_providers() {
            if ["openrouter", "opencode-go", "groq-qwen"].contains(&p.name()) {
                assert_eq!(format!("{p:?}"), format!("{:?}", find(&cfg, p.name())));
            }
        }
        // The chain the worker calls: without the entries turned off.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[chain]\noff = [\"codex\"]\n",
        )
        .unwrap();
        let called: Vec<_> = (load_chain(dir.path()).unwrap().providers.iter())
            .map(|p| p.name().to_owned())
            .collect();
        assert!(!called.contains(&"codex".to_owned()), "{called:?}");
        assert_eq!(called.len(), default_providers().len() - 1);
        assert!(load(dir.path()).unwrap().chain.turns_off("codex"));
    }

    #[test]
    fn chain_changes_a_hand_written_chain_too() {
        let cfg = load_text(
            r#"
[chain]
order = ["claude"]
off = ["local"]
daily_budget = { local = 3, claude = 4 }
timeout_s = { local = 20 }
model = { claude = "sonnet", local = "qwen3:14b" }
[[providers]]
kind = "openai"
name = "local"
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
headers = { "x-a" = "b" }
[[providers]]
kind = "openai"
name = "other"
base_url = "http://127.0.0.1:11435/v1"
model = "m"
[[providers]]
kind = "cli"
name = "claude"
cli = "claude"
"#,
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        let names: Vec<_> = cfg.providers.iter().map(Provider::name).collect();
        assert_eq!(names, ["claude", "local", "other"]);
        let local = find(&cfg, "local");
        assert!(cfg.chain.turns_off("local"));
        assert_eq!(
            (local.daily_budget(), timeout(local), model(local)),
            (3, 20, Some("qwen3:14b"))
        );
        let Provider::Openai { headers, .. } = local else {
            panic!("expected openai")
        };
        assert_eq!(headers["x-a"], "b");
        let claude = find(&cfg, "claude");
        assert_eq!(
            (claude.daily_budget(), timeout(claude), model(claude)),
            (4, 300, Some("sonnet"))
        );
        let other = find(&cfg, "other");
        assert_eq!((other.daily_budget(), timeout(other)), (300, 90));
        assert!(!cfg.chain.turns_off("other") && !cfg.chain.turns_off("claude"));
    }

    /// Names that match nothing, and names given twice, are warnings for doctor: a failed
    /// `load()` would stop curation.
    #[test]
    fn unknown_and_repeated_chain_names_are_warnings_not_failures() {
        let cfg = load_text(
            r#"
[chain]
order = ["groq", "gone", "groq"]
off = ["nim", "nim", "mistral"]
daily_budget = { gone = 1 }
timeout_s = { gone = 1 }
model = { gone = "m" }
"#,
        );
        assert_eq!(cfg.providers[0].name(), "groq");
        assert!(cfg.chain.turns_off("nim"));
        let w = cfg.warnings.join("\n");
        for line in [
            "[chain] order: no chain entry is named \"gone\"",
            "[chain] off: no chain entry is named \"mistral\"",
            "[chain] daily_budget: no chain entry is named \"gone\"",
            "[chain] timeout_s: no chain entry is named \"gone\"",
            "[chain] model: no chain entry is named \"gone\"",
        ] {
            assert!(w.contains(line), "{line}: {w}");
        }
        // A name repeated in `order` or `off` changes nothing: the first counts.
        assert_eq!(cfg.warnings.len(), 5, "{w}");
        // Two `[[providers]]` of one name: a warning, and `[chain]` changes both.
        let entry = "[[providers]]\nkind = \"cli\"\nname = \"c\"\ncli = \"claude\"\n";
        let cfg = load_text(&format!("[chain]\ntimeout_s = {{ c = 9 }}\n{entry}{entry}"));
        assert_eq!(
            cfg.warnings,
            ["2 chain entries are named \"c\": [chain] changes each of them"]
        );
        assert!(cfg.providers.iter().all(|p| timeout(p) == 9));
        // `[chain]` not naming it: no warning (OpenCodeReview on #267).
        let cfg = load_text(&format!("[chain]\noff = [\"groq\"]\n{entry}{entry}"));
        assert!(
            cfg.warnings.iter().all(|w| !w.contains("named \"c\"")),
            "{:?}",
            cfg.warnings
        );
        // A key `[chain]` does not have is refused, with its line: a misspelled `off` would
        // keep calling the entry.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[chain]\nof = [\"codex\"]\n",
        )
        .unwrap();
        let err = format!("{:#}", load(dir.path()).unwrap_err());
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn every_entry_off_is_a_warning_that_points_to_curate() {
        let names: Vec<String> = default_providers()
            .iter()
            .map(|p| format!("\"{}\"", p.name()))
            .collect();
        let off = |names: &[String], curate: bool| {
            load_text(&format!(
                "[summary]\ncurate = {curate}\n[chain]\noff = [{}]\n",
                names.join(", ")
            ))
        };
        let cfg = off(&names, true);
        assert_eq!(cfg.providers.len(), names.len());
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
        assert!(cfg.warnings[0].contains("[summary] curate = false"));
        // One left on, or curation off already: no warning.
        assert!(off(&names[1..], true).warnings.is_empty());
        assert!(off(&names, false).warnings.is_empty());
    }

    /// `[chain] model` sets no model whose price the entry does not know: a model that is not
    /// `:free` on an OpenRouter `:free` entry (OpenRouter bills it, outside the monthly USD cap),
    /// or any model on an entry with prices (they are its model's). Each is a warning (revision 1;
    /// cubic on #94).
    #[test]
    fn chain_sets_no_model_that_changes_the_price() {
        let set = |text: &str, name: &str| {
            let cfg = load_text(text);
            (model(find(&cfg, name)).map(str::to_owned), cfg.warnings)
        };
        let (m, w) = set(
            "[chain]\nmodel = { openrouter = \"openai/gpt-6\" }\n",
            "openrouter",
        );
        assert_ne!(m.as_deref(), Some("openai/gpt-6"));
        assert!(w.iter().any(|w| w.contains("not a :free model")), "{w:?}");
        let (m, w) = set(
            "[chain]\nmodel = { openrouter = \"qwen/qwen3.8-27b:FREE\" }\n",
            "openrouter",
        );
        assert_eq!(m.as_deref(), Some("qwen/qwen3.8-27b:FREE"));
        assert!(w.is_empty(), "{w:?}");
        let (m, w) = set("[chain]\nmodel = { groq = \"openai/gpt-6\" }\n", "groq");
        assert_eq!(m.as_deref(), Some("openai/gpt-6"));
        assert!(w.is_empty(), "{w:?}");
        let entry = "[[providers]]\nkind = \"openai\"\nname = \"o\"\nbase_url = \" https://OpenRouter.ai/api/v1/ \"\nmodel = \"m:free\"\n";
        let paid = "[chain]\nmodel = { o = \"paid/m\" }\n";
        let (m, w) = set(&format!("{paid}{entry}"), "o");
        assert_eq!(m.as_deref(), Some("m:free"));
        assert!(w.iter().any(|w| w.contains("not a :free model")), "{w:?}");
        let (m, w) = set(
            &format!("{paid}{entry}limits = {{ usd_per_mtok_in = 1.0 }}\n"),
            "o",
        );
        assert_eq!(m.as_deref(), Some("m:free"));
        assert!(
            w.iter().any(|w| w.contains("its prices are its model's")),
            "{w:?}"
        );
        // Doctor names a model `[chain]` set, not one it asked for and did not set.
        let cfg = load_text(&format!(
            "{paid}{entry}limits = {{ usd_per_mtok_in = 1.0 }}\n"
        ));
        assert!(!doctor_line(find(&cfg, "o"), &cfg.chain).contains("set in [chain]"));
    }

    #[test]
    fn doctor_line_shows_what_applies_to_an_entry() {
        let cfg = load_text(
            "[chain]\noff = [\"codex\"]\nmodel = { claude = \"sonnet\" }\ndaily_budget = { groq = 5 }\n",
        );
        let line = |name: &str| doctor_line(find(&cfg, name), &cfg.chain);
        assert_eq!(line("groq"), "    5 calls a day, timeout 90 s");
        assert_eq!(
            line("codex"),
            "    off, no cap of calls a day, timeout 300 s"
        );
        assert_eq!(
            line("claude"),
            "    no cap of calls a day, timeout 300 s, model sonnet (set in [chain])"
        );
        // Its key's budget is on its own line (#238): 10 here would be only the placeholder.
        assert!(find(&cfg, "openrouter").budget_from_key());
        assert!(
            !line("openrouter").contains("calls a day"),
            "{}",
            line("openrouter")
        );
    }

    #[test]
    fn inject_reads_its_table_and_refuses_a_wrong_one() {
        let dir = tempfile::tempdir().unwrap();
        let at = |text: &str| {
            std::fs::write(dir.path().join("config.toml"), text).unwrap();
            inject(dir.path())
        };
        assert_eq!(inject(dir.path()).unwrap(), Inject::default());
        assert_eq!(
            Inject::default(),
            Inject {
                session_start: true,
                session_start_chars: 6_000
            }
        );
        let set = at("[inject]\nsession_start = false\nsession_start_chars = 1000\n");
        assert_eq!(
            set.unwrap(),
            Inject {
                session_start: false,
                session_start_chars: 1_000
            }
        );
        assert_eq!(
            at("[inject]\nsession_start_chars = 6000\n")
                .unwrap()
                .session_start_chars,
            6_000
        );
        // Out of range, the wrong type, an unknown key, a file that is not TOML: an error, never
        // the defaults, which would inject for a user who turned it off.
        for bad in [
            "[inject]\nsession_start = false\nsession_start_chars = 999\n",
            "[inject]\nsession_start = false\nsession_start_chars = 6001\n",
            "[inject]\nsession_start = \"false\"\n",
            "[inject]\nsesion_start = false\n",
            "[inject]\nsession_start = false\n[inject\n",
        ] {
            assert!(at(bad).is_err(), "{bad}");
        }
        // Other tables are not its business.
        let other = at("[chain]\noff = 3\n[inject]\nsession_start = false\n");
        assert!(!other.unwrap().session_start);
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
