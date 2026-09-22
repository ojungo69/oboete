//! Mask secrets before text is stored or sent anywhere. The rules are gitleaks' own
//! (`config/gitleaks.toml`, MIT) and are applied the way gitleaks applies them: a rule's
//! keywords gate its regex (one case-insensitive Aho-Corasick pass), the regex is compiled only
//! when a keyword hits, the secret is the first capture group, and entropy plus allowlists
//! filter the candidates. Rules that depend on a file path are skipped: hook text has none.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::Deserialize;

const MASK: &str = "[REDACTED]";
const RULES_TOML: &str = include_str!("../config/gitleaks.toml");
const EXTRA_TOML: &str = include_str!("../config/oboete-rules.toml");

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    allowlist: Allow,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    /// Only read by the tests; kept so a failing rule can be named.
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    entropy: Option<f64>,
    #[serde(default, rename = "secretGroup")]
    secret_group: Option<usize>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    allowlists: Vec<Allow>,
}

#[derive(Deserialize, Default)]
struct Allow {
    #[serde(default)]
    regexes: Vec<String>,
    #[serde(default)]
    stopwords: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default, rename = "regexTarget")]
    regex_target: Option<String>,
    #[serde(default)]
    condition: Option<String>,
}

struct Rules {
    rules: Vec<Rule>,
    global: Allow,
    keywords: AhoCorasick,
    /// Aho-Corasick pattern index → rule index.
    keyword_rule: Vec<usize>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let file: File = toml::from_str(RULES_TOML).expect("bundled gitleaks.toml parses");
        let extra: File = toml::from_str(EXTRA_TOML).expect("bundled oboete-rules.toml parses");
        let rules: Vec<Rule> = file
            .rules
            .into_iter()
            .chain(extra.rules)
            .filter(|r| r.regex.is_some() && r.path.is_none())
            .collect();
        let mut patterns = Vec::new();
        let mut keyword_rule = Vec::new();
        for (i, r) in rules.iter().enumerate() {
            for k in &r.keywords {
                patterns.push(k.to_lowercase());
                keyword_rule.push(i);
            }
        }
        let keywords = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&patterns)
            .expect("keyword automaton");
        Rules {
            rules,
            global: file.allowlist,
            keywords,
            keyword_rule,
        }
    })
}

thread_local! {
    static COMPILED: RefCell<HashMap<String, Regex>> = RefCell::new(HashMap::new());
}

/// Regexes are compiled on first use and kept for the process (a hook compiles a handful at most).
/// ASCII mode first: that is what gitleaks' Go RE2 means by `\w`/`\s`/`\b`, and it compiles
/// ~30x faster than Unicode mode for the generic rules (measured: 90 ms vs 3 ms, debug build).
/// The few patterns ASCII mode rejects (negated classes that could match non-UTF-8) fall back.
fn compiled(pattern: &str) -> Option<Regex> {
    COMPILED.with(|c| {
        if let Some(re) = c.borrow().get(pattern) {
            return Some(re.clone());
        }
        let re = regex::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .or_else(|_| Regex::new(pattern))
            .ok()?;
        c.borrow_mut().insert(pattern.to_string(), re.clone());
        Some(re)
    })
}

pub fn redact(text: &str) -> String {
    let r = rules();
    let mut hit = vec![false; r.rules.len()];
    // Overlapping: "sk" (twilio) inside "gsk_" (groq) must not hide the longer keyword.
    for m in r.keywords.find_overlapping_iter(text) {
        hit[r.keyword_rule[m.pattern().as_usize()]] = true;
    }
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (rule, _) in r.rules.iter().zip(&hit).filter(|(_, h)| **h) {
        let Some(re) = rule.regex.as_deref().and_then(compiled) else {
            continue;
        };
        for caps in re.captures_iter(text) {
            let whole = caps.get(0).expect("group 0");
            let secret = match rule.secret_group {
                Some(g) => caps.get(g),
                None => (1..caps.len())
                    .find_map(|i| caps.get(i))
                    .filter(|m| !m.is_empty()),
            }
            .unwrap_or(whole);
            if let Some(min) = rule.entropy
                && shannon_entropy(secret.as_str()) <= min
            {
                continue;
            }
            let line = line_of(text, whole.start());
            let allowed = rule
                .allowlists
                .iter()
                .chain(std::iter::once(&r.global))
                .any(|a| allows(a, secret.as_str(), whole.as_str(), line));
            if !allowed {
                spans.push((secret.start(), secret.end()));
            }
        }
    }
    if spans.is_empty() {
        return text.to_string();
    }
    spans.sort_unstable();
    // Overlapping matches (a short and a long rule on one token) mask their union.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (start, end) in merged {
        out.push_str(&text[pos..start]);
        out.push_str(MASK);
        pos = end;
    }
    out.push_str(&text[pos..]);
    out
}

/// gitleaks' allowlist: OR = any regex or stopword hit; AND = every configured check must hold
/// (a path check can never hold here, so such lists never allow).
fn allows(a: &Allow, secret: &str, whole: &str, line: &str) -> bool {
    let target = match a.regex_target.as_deref() {
        Some("match") => whole,
        Some("line") => line,
        _ => secret,
    };
    let regex_hit = || {
        a.regexes
            .iter()
            .any(|p| compiled(p).is_some_and(|re| re.is_match(target)))
    };
    let lower = secret.to_lowercase();
    let stopword_hit = || a.stopwords.iter().any(|w| lower.contains(w.as_str()));
    if a.condition
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case("and"))
    {
        !a.paths.is_empty() && false
            || (a.paths.is_empty()
                && (a.regexes.is_empty() || regex_hit())
                && (a.stopwords.is_empty() || stopword_hit())
                && !(a.regexes.is_empty() && a.stopwords.is_empty()))
    } else {
        regex_hit() || stopword_hit()
    }
}

fn line_of(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    &text[start..end]
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts: HashMap<char, usize> = HashMap::new();
    let mut n = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_default() += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|&c| {
            let p = c as f64 / n as f64;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_regex_compiles() {
        let started = std::time::Instant::now();
        let r = rules();
        let parse_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let mut broken_allowlists = 0;
        for rule in &r.rules {
            let p = rule.regex.as_deref().unwrap();
            assert!(compiled(p).is_some(), "rule {} does not compile", rule.id);
            for a in &rule.allowlists {
                // A broken allowlist regex only means fewer exemptions (masking stays on).
                broken_allowlists += a.regexes.iter().filter(|p| compiled(p).is_none()).count();
            }
        }
        for p in &r.global.regexes {
            if compiled(p).is_none() {
                eprintln!("global allowlist regex does not compile: {p}");
                broken_allowlists += 1;
            }
        }
        eprintln!(
            "rules {} parse {parse_ms} ms, compile all {} ms, broken allowlist regexes {broken_allowlists}",
            r.rules.len(),
            started.elapsed().as_millis()
        );
        assert!(r.rules.len() > 200);
        assert!(
            broken_allowlists <= 4,
            "known: curl-auth-user's [^]] (x2) and two global `{{\\d+}}` patterns"
        );
    }

    #[test]
    fn masks_known_shapes_and_leaves_prose() {
        let r = redact("key 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 and more");
        assert_eq!(r, "key [REDACTED] and more");
        assert_eq!(
            redact("no secrets here, task-1 ok, the api key is stored elsewhere"),
            "no secrets here, task-1 ok, the api key is stored elsewhere"
        );
        let pem = "x -----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAq9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD\nq9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD==\n-----END RSA PRIVATE KEY----- y";
        assert_eq!(redact(pem), "x [REDACTED] y");
        // AWS's documented example ids are allowlisted by gitleaks; a real-shaped one is not.
        assert_eq!(redact("AKIAIOSFODNN7EXAMPLE"), "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(redact("id AKIAQ7ZX6ML2VB4NR3TY here"), "id [REDACTED] here");
        // generic-api-key keeps the assignment, masks the value; low-entropy values stay.
        assert_eq!(
            redact("api_key = \"q9Zx8mL2vB4nR7tY1wK3pS6d\""),
            "api_key = \"[REDACTED]\""
        );
        assert_eq!(
            redact("api_key = \"aaaaaaaaaaaaaaaaaaaa\""),
            "api_key = \"aaaaaaaaaaaaaaaaaaaa\""
        );
        // Stopwords in the value are allowed through (gitleaks' generic allowlist).
        assert_eq!(
            redact("token = \"example_token_value_123\""),
            "token = \"example_token_value_123\""
        );
        assert_eq!(
            redact("Authorization: Bearer ghp_q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g ok"),
            "Authorization: Bearer [REDACTED] ok"
        );
        assert_eq!(
            redact(
                "ANTHROPIC_API_KEY=sk-ant-api03-q9Zx8mL2vB4nR7tY1wK3pS6dq9Zx8mL2vB4nR7tY1wK3pS6d-AA"
            ),
            "ANTHROPIC_API_KEY=[REDACTED]"
        );
    }

    #[test]
    fn overlapping_rules_mask_the_whole_token() {
        // gitlab-pat (20 chars after the prefix) and gitlab-pat-routable (the full token)
        // start at the same place; the longer match must win, not leave a suffix.
        assert_eq!(
            redact("token glpat-Q9zX8mL2vB4nR7tY1wK3pS6dJ0a.1a2b3c4d5 end"),
            "token [REDACTED] end"
        );
    }

    #[test]
    fn multibyte_text_is_untouched_and_boundaries_are_safe() {
        let s = "日本語の説明。トークンは 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 です。";
        let r = redact(s);
        assert_eq!(r, "日本語の説明。トークンは [REDACTED] です。");
    }
}

#[cfg(test)]
mod fixture_scan {
    use super::*;

    /// Reports what the rules mask in the fixture of record; skipped when it is not checked out.
    #[test]
    fn fixture_masks_are_plausible() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../free-mem/test/fixtures/events-1000.jsonl"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("fixture missing, skipped");
            return;
        };
        let started = std::time::Instant::now();
        let mut masked = 0;
        let mut samples = Vec::new();
        for line in text.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let payload = v["payload"].to_string();
            let r = redact(&payload);
            if r != payload {
                masked += 1;
                if samples.len() < 8 {
                    let at = r.find(MASK).unwrap();
                    let lo = r[..at].char_indices().rev().nth(60).map_or(0, |(i, _)| i);
                    let hi = r[at..]
                        .char_indices()
                        .nth(40)
                        .map_or(r.len(), |(i, _)| at + i);
                    samples.push(r[lo..hi].to_string());
                }
            }
        }
        eprintln!(
            "fixture: {} lines, {masked} masked, {} ms\n{}",
            text.lines().count(),
            started.elapsed().as_millis(),
            samples.join("\n---\n")
        );
    }
}
