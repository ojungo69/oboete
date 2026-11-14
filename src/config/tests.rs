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
        !entry("base_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m\"\n").budget_from_key()
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
deliverables = false
[inject]
session_start = false
[chain]
off = ["codex"]
[worker]
resident = true
autostart = true
[view]
port = 17374
[retention]
days = 90
[my_work]
curate = true
[judge]
key_file = "/k/TYPESAFE_KEY.md"
enabled = false
"#;
    let c: Config = toml::from_str(text).unwrap();
    assert_eq!(c.paid_usd_per_month, 2.5);
    assert_eq!(parse_retention(text).unwrap().days, Some(90));
    for bad in ["-1.0", "nan", "inf"] {
        assert!(toml::from_str::<Limits>(&format!("usd_per_mtok_in = {bad}")).is_err());
        assert!(toml::from_str::<Limits>(&format!("usd_per_mtok_out = {bad}")).is_err());
    }
    // Decision 64: the monthly cap may be off, written `inf`; no other value outside 0 or more.
    for bad in ["-1.0", "nan", "-inf"] {
        assert!(toml::from_str::<Config>(&format!("paid_usd_per_month = {bad}")).is_err());
    }
    let off = toml::from_str::<Config>("paid_usd_per_month = inf").unwrap();
    assert_eq!(off.paid_usd_per_month, no_monthly_cap());
    parse_capture(Some(text)).unwrap();
    // Every key `Config` reads, as its derived `Deserialize` names them, so that a key a later
    // version adds there never stops recording (CodeRabbit on #660: `[my_work]` did).
    let keys = config_keys();
    assert!(
        keys.contains(&"my_work") && keys.contains(&"providers"),
        "{keys:?}"
    );
    for key in keys {
        assert!(
            parse_capture(Some(&format!("{key} = 1\n"))).is_ok(),
            "{key}"
        );
    }
    // A table no version reads is still an error.
    assert!(parse_capture(Some("[redactions]\n")).is_err());
}

/// `[summary] conversation` (decision 107) is off unless a home turns it on.
#[test]
fn the_conversation_is_off_unless_asked_for() {
    let of = |text: &str| toml::from_str::<Config>(text).unwrap().summary.conversation;
    assert!(!of(""));
    assert!(!of("[summary]\ncurate = true\n"));
    assert!(of("[summary]\nconversation = true\n"));
    assert!(!Summary::default().conversation);
}

/// `[summary] shrink` (decision 107): `true` is every tool call, `"subagents"` only a
/// subagent's, `false` and no line none; any other value is refused, not read as one of them.
#[test]
fn shrink_is_true_false_or_subagents() {
    use crate::curate::Shrink;
    let of = |line: &str| {
        toml::from_str::<Config>(&format!("[summary]\n{line}\n")).map(|c| c.summary.shrink)
    };
    assert_eq!(of("shrink = true").unwrap(), Shrink::All);
    assert_eq!(of("shrink = false").unwrap(), Shrink::Off);
    assert_eq!(of("shrink = \"subagents\"").unwrap(), Shrink::Subagents);
    assert_eq!(of("").unwrap(), Shrink::Off);
    for bad in ["shrink = \"all\"", "shrink = \"Subagents\"", "shrink = 1"] {
        assert!(of(bad).is_err(), "{bad}");
    }
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
            "anthropic",
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
    assert_eq!(before[at - 1], "anthropic");
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

/// `[my_work]` (docs/my-work-model.md A6, A7): off and 100 calls a day unless set.
#[test]
fn my_work_is_off_with_100_calls_a_day_unless_set() {
    let none = tempfile::tempdir().unwrap();
    let defaults = MyWork {
        curate: false,
        calls_a_day: 100,
    };
    assert_eq!(load(none.path()).unwrap().my_work, defaults);
    assert_eq!(load_text("[summary]\ncurate = true\n").my_work, defaults);
    let set = load_text("[my_work]\ncurate = true\ncalls_a_day = 40\n").my_work;
    assert_eq!((set.curate, set.calls_a_day), (true, 40));
}

/// CodeRabbit on #660: a key misspelt in `[my_work]` is an error, as in `[worker]`, not a
/// switch silently left off.
#[test]
fn a_key_misspelt_in_my_work_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[my_work]\ncurat = true\n").unwrap();
    assert!(load(dir.path()).is_err());
}

/// The top-level keys `Config` reads, as its derived `Deserialize` names them.
fn config_keys() -> &'static [&'static str] {
    struct Fields(std::cell::Cell<&'static [&'static str]>);
    impl<'de> serde::Deserializer<'de> for &Fields {
        type Error = serde::de::value::Error;
        fn deserialize_any<V: serde::de::Visitor<'de>>(
            self,
            _: V,
        ) -> Result<V::Value, Self::Error> {
            Err(serde::de::Error::custom("only the fields"))
        }
        fn deserialize_struct<V: serde::de::Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.set(fields);
            Err(serde::de::Error::custom("only the fields"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }
    let fields = Fields(std::cell::Cell::new(&[]));
    let _ = Config::deserialize(&fields);
    fields.0.get()
}

fn load_text(text: &str) -> Config {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), text).unwrap();
    load(dir.path()).unwrap()
}

/// J7: each destination of the list loads by its name; a table written before it (a Typesafe
/// key and the switch) is Jev's, as it was. A name not on the list, or an account id that is not
/// Cloudflare's, is refused at load, and neither is repeated in the error.
#[test]
fn the_judge_sends_to_one_of_the_list() {
    for d in Destination::ALL {
        let cfg = load_text(&format!("[judge]\ndestination = \"{}\"\n", d.id()));
        assert_eq!(cfg.judge.unwrap().destination, d);
    }
    let old = load_text("[judge]\nkey_file = \"/k/TYPESAFE_KEY.md\"\nenabled = true\n");
    let old = old.judge.unwrap();
    assert_eq!(old.destination, Destination::Jev);
    assert_eq!(
        old.key_file.as_deref(),
        Some(Path::new("/k/TYPESAFE_KEY.md"))
    );
    let account = "0123456789abcdef0123456789abcdef";
    let cfg = load_text(&format!(
        "[judge]\ndestination = \"clef-flash\"\ncloudflare_account_id = \"{account}\"\n"
    ));
    assert_eq!(
        cfg.judge.unwrap().cloudflare_account_id.as_deref(),
        Some(account)
    );
    for (line, says) in [
        ("destination = \"clef-omni\"", "line 2 is not valid here"),
        (
            "destination = \"https://example.invalid\"",
            "line 2 is not valid here",
        ),
        (
            "cloudflare_account_id = \"0123456789ABCDEF0123456789abcdef\"",
            "not a Cloudflare account id",
        ),
        (
            "cloudflare_account_id = \"0123456789abcdef0123456789abcde\"",
            "not a Cloudflare account id",
        ),
        (
            "cloudflare_account_id = \"../../zones/z/purge_cache?x=\"",
            "not a Cloudflare account id",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[judge]\n{line}\n");
        std::fs::write(dir.path().join("config.toml"), &text).unwrap();
        let err = format!("{:#}", load(dir.path()).unwrap_err());
        assert!(err.contains(says), "{line}: {err}");
        if line.starts_with("cloudflare") {
            assert!(!err.contains("ABCDEF") && !err.contains("zones"), "{err}");
        }
    }
}

/// J1: `[judge]` absent is off; present, its defaults; a destination of its own is refused,
/// and a chain entry named as the judge's ledger rows is never in the chain, with a warning.
#[test]
fn the_judge_table_has_its_defaults_no_destination_and_no_chain_namesake() {
    assert_eq!(load_text("").judge, None);
    let cfg = load_text("[judge]\nkey_file = \"/k/TYPESAFE_KEY.md\"\n");
    assert_eq!(
        cfg.judge,
        Some(Judge {
            enabled: true,
            destination: Destination::Jev,
            key_file: Some("/k/TYPESAFE_KEY.md".into()),
            cloudflare_key_file: None,
            cloudflare_account_id: None,
            liquid_key_file: None,
            model: "jev-latest".into(),
            timeout_s: 15,
            daily_budget: 300,
        })
    );
    for field in [
        "base_url = \"https://example.invalid\"",
        "headers = {}",
        "extra = {}",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[judge]\nkey_file = \"/k\"\n{field}\n");
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        assert!(load(dir.path()).is_err(), "{field}");
    }
    let cfg = load_text(
        "[[providers]]\nkind = \"openai\"\nname = \"judge:jev\"\nbase_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n\
             [[providers]]\nkind = \"openai\"\nname = \"local\"\nbase_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n",
    );
    assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
    assert!(cfg.warnings[0].contains("judge:jev"), "{:?}", cfg.warnings);
    // Kept in the file's order for the settings page, never in the chain.
    let names = |cfg: &Config| -> Vec<String> {
        cfg.providers.iter().map(|p| p.name().to_owned()).collect()
    };
    assert_eq!(names(&cfg), ["judge:jev", "local"]);
    let dir = tempfile::tempdir().unwrap();
    let text = "[[providers]]\nkind = \"openai\"\nname = \"judge:jev\"\nbase_url = \"https://example.invalid/v1\"\nmodel = \"m\"\n";
    std::fs::write(dir.path().join("config.toml"), text).unwrap();
    assert!(names(&load_chain(dir.path()).unwrap()).is_empty());
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
            "anthropic",
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
    // Nor the built-in entries that are off until turned on (anthropic).
    let on = default_providers().iter().filter(|p| p.enabled()).count();
    assert_eq!(called.len(), on - 1);
    assert!(!called.contains(&"anthropic".to_owned()), "{called:?}");
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
fn doctor_reports_native_off_entries_even_without_a_name_group_override() {
    let cfg = load_text(
        "[[providers]]\nkind = \"cli\"\nname = \"same\"\ncli = \"claude\"\nenabled = false\n\
            [[providers]]\nkind = \"cli\"\nname = \"same\"\ncli = \"codex\"\n\
            [[providers]]\nkind = \"openai\"\nname = \"http\"\nenabled = false\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n",
    );
    for (p, off) in cfg.providers.iter().zip([true, false, true]) {
        assert_eq!(doctor_line(p, &cfg.chain).starts_with("    off, "), off);
    }
    let mut chain = cfg.chain.clone();
    chain.off.push("same".into());
    assert!(doctor_line(&cfg.providers[1], &chain).starts_with("    off, "));
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
            session_start_chars: 9_000,
            ..Inject::default()
        }
    );
    let set = at("[inject]\nsession_start = false\nsession_start_chars = 1000\n");
    assert_eq!(
        set.unwrap(),
        Inject {
            session_start: false,
            session_start_chars: 1_000,
            ..Inject::default()
        }
    );
    assert_eq!(
        at("[inject]\nsession_start_chars = 6000\n")
            .unwrap()
            .session_start_chars,
        6_000
    );
    // Decision 75: none to 50 index lines.
    for lines in [0, 50] {
        let text = format!("[inject]\nsession_start_index = {lines}\n");
        assert_eq!(at(&text).unwrap().session_start_index, lines);
    }
    // Out of range, the wrong type, an unknown key, a file that is not TOML: an error, never
    // the defaults, which would inject for a user who turned it off.
    for bad in [
        "[inject]\nsession_start = false\nsession_start_chars = 999\n",
        "[inject]\nsession_start = false\nsession_start_chars = 9001\n",
        "[inject]\nsession_start = false\nsession_start_index = 51\n",
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

/// Task 8 (spec 1.5): the prompt's injection, off until the Inject harness has measured it,
/// and the corrections, on; each size in its range, one past either end refused.
#[test]
fn inject_settings_check_ranges_and_save_alone() {
    let d = Inject::default();
    assert_eq!(
        (
            d.per_prompt,
            d.per_prompt_chars,
            d.per_prompt_wait_ms,
            d.correction,
            d.correction_chars
        ),
        (false, 2_500, 2_000, true, 800)
    );
    for (key, low, high) in [
        ("session_start_chars", 1_000, 9_000),
        ("per_prompt_chars", 500, 6_000),
        ("per_prompt_wait_ms", 100, 3_000),
        ("correction_chars", 300, 3_000),
    ] {
        for (value, ok) in [
            (low, true),
            (high, true),
            (low - 1, false),
            (high + 1, false),
        ] {
            let parsed = parse_inject(&format!("[inject]\n{key} = {value}\n"));
            assert_eq!(parsed.is_ok(), ok, "{key} = {value}");
        }
    }
    let set = parse_inject("[inject]\nper_prompt = true\ncorrection = false\n").unwrap();
    assert!(set.per_prompt && !set.correction);
}

/// docs/retention.md: no key or no table is forever; a day count past either end, or not a
/// whole number of days, is refused rather than read as some period, and capture never minds.
#[test]
fn retention_is_forever_unless_a_whole_number_of_days_in_range() {
    assert_eq!(parse_retention("").unwrap().days, None);
    assert_eq!(parse_retention("[retention]\n").unwrap().days, None);
    for (value, ok) in [
        ("1", true),
        ("36500", true),
        ("0", false),
        ("36501", false),
        ("-1", false),
        ("1.5", false),
        ("\"30\"", false),
        ("4294967296", false),
    ] {
        let text = format!("[retention]\ndays = {value}\n");
        assert_eq!(parse_retention(&text).is_ok(), ok, "days = {value}");
        parse_capture(Some(&text)).unwrap();
    }
    assert!(parse_retention("[retention]\nweeks = 2\n").is_err());
}

/// `[view]` (docs/resident.md R5): the resident viewer's port, 17373 where the table names
/// none. Port 0 (any port, which no bookmark can name) and a mistake in the table are errors.
#[test]
fn the_resident_viewers_port_is_its_tables_or_17373() {
    let dir = tempfile::tempdir().unwrap();
    let at = |text: &str| {
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        view(dir.path())
    };
    let port = |v: Result<View>| v.unwrap().port.get();
    assert_eq!(port(view(dir.path())), 17373, "no file");
    assert_eq!(port(at("[worker]\nresident = true\n")), 17373);
    assert_eq!(port(at("[view]\nport = 17374\n")), 17374);
    for bad in [
        "[view]\nport = 0\n",
        "[view]\nport = 65536\n",
        "[view]\nport = \"17374\"\n",
        "[view]\nprot = 17374\n",
    ] {
        assert!(at(bad).is_err(), "{bad}");
    }
}

/// `[worker]` (docs/resident.md R2): resident only where the table says so. A mistake in the
/// table, or a file that is not TOML, is an error: a worker reads that as "stay as you are".
#[test]
fn a_worker_is_resident_only_where_its_table_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let at = |text: &str| {
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        worker(dir.path())
    };
    assert!(!worker(dir.path()).unwrap().resident, "no file");
    assert!(!at("[summary]\ncurate = true\n").unwrap().resident);
    assert!(at("[worker]\nresident = true\n").unwrap().resident);
    assert!(!at("[worker]\nresident = false\n").unwrap().resident);
    for bad in [
        "[worker]\nresident = \"true\"\n",
        "[worker]\nresidant = true\n",
        "[worker]\nresident = true\n[worker\n",
    ] {
        assert!(at(bad).is_err(), "{bad}");
    }
    // Other tables are not its business.
    let other = at("[chain]\noff = 3\n[worker]\nresident = true\n");
    assert!(other.unwrap().resident);
}

/// `[worker] autostart` (docs/resident.md R16, parity row 27): hooks start a worker unless the
/// table says `false`, as claude-mem's `CLAUDE_MEM_WORKER_AUTOSTART` does.
#[test]
fn hooks_start_a_worker_unless_its_table_says_autostart_false() {
    let dir = tempfile::tempdir().unwrap();
    let at = |text: &str| {
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        worker(dir.path())
    };
    assert!(worker(dir.path()).unwrap().autostart, "no file");
    assert!(at("[worker]\nresident = true\n").unwrap().autostart);
    assert!(!at("[worker]\nautostart = false\n").unwrap().autostart);
    let both = at("[worker]\nresident = true\nautostart = false\n").unwrap();
    assert!(both.resident && !both.autostart);
    assert!(at("[worker]\nautostart = \"false\"\n").is_err());
    // Only a table that loads says no: one that does not load starts a worker as before.
    at("[worker]\nautostart = false\n").unwrap();
    assert!(!autostart(dir.path()));
    at("[worker]\nautostart = false\n[worker\n").unwrap_err();
    assert!(autostart(dir.path()));
    std::fs::remove_file(dir.path().join("config.toml")).unwrap();
    assert!(autostart(dir.path()), "no file");
}

#[test]
fn unknown_embedding_provider_is_refused_at_load() {
    let dir = std::env::temp_dir().join(format!("oboete-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[embedding]\nprovider = \"fastembed\"\n",
    )
    .unwrap();
    let err = load(&dir).unwrap_err().to_string();
    assert!(err.contains("does not exist"), "{err}");
    // `local` loads in every build, the model's files or not (`embed::local_unavailable`).
    std::fs::write(
        dir.join("config.toml"),
        "[embedding]\nprovider = \"local\"\n",
    )
    .unwrap();
    assert!(load(&dir).is_ok());
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

/// Milestone 4 D8: the embedder's own monthly cap, and a url only where Workers AI or a
/// loopback stub listens.
#[test]
fn embedding_monthly_usd_and_url_load_or_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let at = |rest: &str| {
        let text = format!("[embedding]\nprovider = \"workers-ai\"\naccount_id = \"a\"\n{rest}");
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        load(dir.path())
    };
    let cfg = at("").unwrap();
    assert_eq!((cfg.embedding.monthly_usd, cfg.embedding.url), (1.0, None));
    for url in [
        "https://api.cloudflare.com/client/v4/accounts/a/ai/run/@cf/baai/bge-m3",
        "http://127.0.0.1:8787/run",
        "http://localhost:8787/run",
        "http://[::1]:8787/run",
    ] {
        let cfg = at(&format!("url = \"{url}\"\nmonthly_usd = 2.5\n")).unwrap();
        assert_eq!(cfg.embedding.url.as_deref(), Some(url));
        assert_eq!(cfg.embedding.monthly_usd, 2.5);
    }
    for bad in [
        "url = \"https://example.com/run\"",
        "url = \"http://api.cloudflare.com/run\"",
        "url = \"https://api.cloudflare.com.example.com/run\"",
        "url = \"http://127.0.0.1.example.com/run\"",
        "monthly_usd = -1.0",
        "monthly_usd = nan",
        "monthly_usd = 0",
        "monthly_usd = 0.0",
    ] {
        assert!(at(bad).is_err(), "{bad}");
    }
}

/// The owner, 2026-10-09: an HTTP entry may speak Anthropic's Messages API. Absent, `api` is
/// the OpenAI-compatible one and a save leaves it out; an unknown one is refused, by name.
#[test]
fn an_entry_speaks_the_api_it_names_and_an_unknown_one_is_refused() {
    let entry = |rest: &str| {
        format!(
            "[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
                 base_url = \"https://api.anthropic.com/v1\"\nmodel = \"m\"\n{rest}"
        )
    };
    let first = |text: &str| load_text(text).providers.remove(0);
    let api = |text: &str| match first(text) {
        Provider::Openai { api, .. } => api,
        Provider::Cli { .. } => panic!("expected openai"),
    };
    assert_eq!(api(&entry("")), Api::Openai);
    assert_eq!(api(&entry("api = \"openai\"\n")), Api::Openai);
    assert_eq!(api(&entry("api = \"anthropic\"\n")), Api::Anthropic);
    let saved = |text: &str| toml::to_string(&first(text)).unwrap();
    let has_api = |text: &str| saved(text).lines().any(|l| l.starts_with("api ="));
    assert!(!has_api(&entry("api = \"openai\"\n")));
    assert!(saved(&entry("api = \"anthropic\"\n")).contains("api = \"anthropic\""));
    let back: Provider = toml::from_str(&saved(&entry("api = \"anthropic\"\n"))).unwrap();
    assert!(matches!(
        back,
        Provider::Openai {
            api: Api::Anthropic,
            ..
        }
    ));
    // A Messages request always names its output's size; an OpenAI one without a price none.
    assert_eq!(
        first(&entry("api = \"anthropic\"\n")).declared_output(),
        4000
    );
    assert_eq!(first(&entry("")).declared_output(), 0);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        entry("api = \"antrhopic\"\n"),
    )
    .unwrap();
    let err = format!("{:#}", load(dir.path()).unwrap_err());
    assert!(
        err.contains(": a provider's api is \"openai\" (the default) or \"anthropic\""),
        "{err}"
    );
    assert!(!err.contains("antrhopic"), "{err}");
}

/// The owner, 2026-10-09: Anthropic's API right after OpenCode Go, priced, so it stays inside
/// the paid cap with every other paid entry. It is off until the user turns it on: a config
/// that takes the built-in chain would otherwise send to a new recipient on upgrade, which
/// `[chain] off` written before could not name (Codex's security review of #409).
#[test]
fn the_default_anthropic_entry_is_priced_and_speaks_messages() {
    let cfg: Config = toml::from_str("").unwrap();
    let p = find(&cfg, "anthropic");
    assert!(!p.enabled());
    let Provider::Openai {
        api,
        base_url,
        key_file,
        model,
        extra,
        limits,
        subscription,
        ..
    } = p
    else {
        panic!("expected an HTTP entry")
    };
    assert_eq!(*api, Api::Anthropic);
    assert_eq!(base_url, "https://api.anthropic.com/v1");
    assert!(
        key_file
            .as_ref()
            .is_some_and(|f| f.ends_with("ANTHROPIC_API_KEY.md"))
    );
    assert_eq!(model, "claude-haiku-5-5");
    assert_eq!(extra["thinking"]["type"], "disabled");
    assert!(limits.is_paid() && !subscription && p.tier() == 3);
    assert_eq!(
        (
            limits.usd_per_mtok_in,
            limits.usd_per_mtok_out,
            limits.max_request_tokens
        ),
        (0.10, 0.50, Some(100_000))
    );
    assert_eq!(p.declared_output(), limits.max_output_tokens);
}

/// Decision 81: the summaries' language is the computer's when none is set, by the locale's
/// language part; `conversation` asks for each conversation's own; oboete's own words go in
/// Japanese for any spelling of it, and for `conversation` on a Japanese computer.
#[test]
fn the_summary_language_follows_the_computer_unless_set() {
    for (locale, language) in [
        ("ja_JP.UTF-8", "Japanese"),
        ("en_US.UTF-8", "English"),
        ("C", "English"),
        ("", "English"),
        ("zh-Hans-CN", "Chinese"),
        ("fr-FR", "French"),
        ("xx_YY", "English"),
    ] {
        assert_eq!(language_of(locale), language, "{locale}");
    }
    OS_LOCALE.with(|l| *l.borrow_mut() = "de_DE.UTF-8".into());
    assert_eq!(Summary::default().language, "German");
    assert!(!notes_japanese("conversation"));
    let (_, japanese) = parse_inject_with_language("").unwrap();
    assert!(!japanese, "no language set: the computer's, German");
    let none = tempfile::tempdir().unwrap();
    assert!(
        !inject_with_language(none.path()).unwrap().1,
        "no config.toml: German"
    );
    // Left blank, the computer's, as unset (cubic on #864).
    let blank = "[summary]\nlanguage = \"  \"\n";
    assert!(
        !parse_inject_with_language(blank).unwrap().1,
        "blank: German"
    );
    let summary: Summary = toml::from_str("language = \" \"").unwrap();
    assert_eq!(summary.language, "German");
    OS_LOCALE.with(|l| *l.borrow_mut() = "ja_JP.UTF-8".into());
    assert!(notes_japanese("conversation"));
    assert!(
        parse_inject_with_language(blank).unwrap().1,
        "blank: the computer's, Japanese"
    );
    assert!(
        inject_with_language(none.path()).unwrap().1,
        "no config.toml: Japanese"
    );
    assert_eq!(write_in(" French "), "in French");
    assert_eq!(
        write_in("Conversation"),
        "in the language the developer writes in, in the records below"
    );
    for (language, japanese) in [
        ("Japanese", true),
        ("japanese", true),
        (" ja ", true),
        ("ja-JP", true),
        ("日本語", true),
        ("ja.UTF-8", true),
        ("ja@x", true),
        ("English", false),
        ("javanese", false),
        ("jav", false),
    ] {
        assert_eq!(is_japanese(language), japanese, "{language}");
        let text = format!("[summary]\nlanguage = {language:?}\n");
        assert_eq!(
            parse_inject_with_language(&text).unwrap().1,
            japanese,
            "{language}"
        );
    }
}

/// R05, A106: a home can still hold the config.toml oboete v1 wrote: it loads with its providers
/// in order, their models, and the key files by path.
#[test]
fn a_v1_config_loads() {
    let home = tempfile::tempdir().unwrap();
    let text = r#"gemini = "after-subscriptions"

[[providers]]
kind = "openai"
name = "groq"
base_url = "https://api.groq.com/openai/v1"
key_file = "/k/GROQ_KEY.md"
model = "llama-3.3-70b-versatile"

[[providers]]
kind = "cli"
name = "codex"
cli = "codex"
model = "gpt-5.5"

[embedding]
provider = "workers-ai"
account_id = "acct"
key_file = "/k/CF_WORKERS_AI_KEY.md"
"#;
    std::fs::write(home.path().join("config.toml"), text).unwrap();
    let cfg = load(home.path()).unwrap();
    let chain: Vec<(&str, Option<&str>, Option<&Path>)> = cfg
        .providers
        .iter()
        .map(|p| match p {
            Provider::Openai {
                name,
                model,
                key_file,
                ..
            } => (name.as_str(), Some(model.as_str()), key_file.as_deref()),
            Provider::Cli { name, model, .. } => (name.as_str(), model.as_deref(), None),
        })
        .collect();
    assert_eq!(
        chain[..2],
        [
            (
                "groq",
                Some("llama-3.3-70b-versatile"),
                Some(Path::new("/k/GROQ_KEY.md"))
            ),
            ("codex", Some("gpt-5.5"), None),
        ]
    );
    assert_eq!(chain.last().map(|c| c.0), Some("gemini"));
    assert_eq!(cfg.embedding.key_file, Path::new("/k/CF_WORKERS_AI_KEY.md"));
    assert!(!cfg.summary.curate);
}
