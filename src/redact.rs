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
    /// The rule a finding names in the redaction ledger.
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

/// The one gate for text that leaves this machine: summary providers now; embeddings, sync and
/// judges later (docs/research/search-sync-proposal-2026-09-23.md §4.8). Closed `<private>`-style
/// blocks go, then gitleaks redaction. Apply it per field, not to a joined transcript: a stray
/// `<private>` in one tool output must not pair with a `</private>` many events later.
pub fn outbound(text: &str) -> String {
    redact(&crate::hook::strip_blocks(text, false))
}

pub fn redact(text: &str) -> String {
    scan(text).0
}

/// One secret masked in stored text: the rule that found it, where the mask hiding it starts in
/// the stored (masked) text, and the secret's own length, both in bytes. Never the value. Two
/// rules on one token share one mask, so they give two findings at the same offset.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: String,
    pub offset: usize,
    pub length: usize,
}

/// `text` masked, with its findings.
pub fn scan(text: &str) -> (String, Vec<Finding>) {
    let (mut masked, mut found) = mask(text, spans(text));
    if !found.is_empty() {
        rescan(&mut masked, &mut found);
    }
    (masked, found)
}

/// Passes over masked text before `rescan` masks the whole text; each pass must mask something new.
const MAX_PASSES: usize = 64;

/// Scan `masked` again until a pass finds nothing. A rule whose secret needs context finds one
/// secret per context per pass (curl-auth-user's greedy `.*` the last `-u` on a line,
/// curl-auth-header's lazy `.*?` the first header after a `curl`), and a cut can leave a line
/// shorter than the one a line-scoped allowlist judged (v1's `clip` scanned twice for that). Each
/// earlier finding moves by what the runs before it changed, or to the start of a new mask
/// covering it. Still finding after `MAX_PASSES`, the whole text becomes one mask that every
/// finding points at: a mask of part of it could take the context (`curl`) that the rest needs.
fn rescan(masked: &mut String, found: &mut Vec<Finding>) {
    for pass in 0..=MAX_PASSES {
        let again = spans(masked);
        if again.is_empty() {
            break;
        }
        if pass == MAX_PASSES {
            found.extend(mask(masked, again).1);
            for f in found.iter_mut() {
                f.offset = 0;
            }
            *masked = MASK.to_string();
            break;
        }
        let runs = merged(&again);
        let (next, more) = mask(masked, again);
        if next == *masked {
            break; // a rule matching its own mask: nothing new to hide
        }
        for f in found.iter_mut() {
            let mut shift = 0isize;
            let mut at = f.offset;
            for &(s, e) in &runs {
                if e <= f.offset {
                    shift += MASK.len() as isize - (e - s) as isize;
                } else {
                    if s <= f.offset {
                        at = s;
                    }
                    break;
                }
            }
            f.offset = (at as isize + shift) as usize;
        }
        *masked = next;
        found.extend(more);
    }
    found.sort_by_key(|f| f.offset);
}

/// `text` whole when it is at most `cap` bytes. Above that, only the first and last `cap / 2`
/// bytes of its masked form are kept, around a marker that gives the full size (spec 2.4), and
/// the third value is that full size. The whole text is scanned and masked to a fixpoint before
/// anything is cut ("redacted in full", spec 2.2): a rule can need context far from its secret
/// (curl-auth-user reads a whole line, and a cut can drop a closing quote it needs). A cut inside
/// a mask moves to the mask's edge; a private key block that a cut splits (a BEGIN without its
/// END, or an END without its BEGIN, in any case) is dropped from the part that holds it; then the
/// stored text is scanned once more, so line-scoped allowlists judge the lines as they are stored
/// (as v1's `clip` did).
pub fn scan_capped(text: &str, cap: usize) -> (String, Vec<Finding>, Option<usize>) {
    let (masked, mut found) = scan(text);
    if text.len() <= cap || masked.len() <= cap {
        return (masked, found, None);
    }
    let runs: Vec<(usize, usize)> = found
        .iter()
        .map(|f| (f.offset, f.offset + MASK.len()))
        .collect();
    let half = cap / 2;
    let mut head_end = masked.floor_char_boundary(half);
    let mut tail_start = masked.ceil_char_boundary(masked.len() - half);
    // A mask across a cut is kept whole.
    for &(s, e) in &runs {
        if s < head_end && e > head_end {
            head_end = e;
        }
        if s < tail_start && e > tail_start {
            tail_start = s;
        }
    }
    // ASCII lowercasing keeps byte offsets.
    let head = masked[..head_end].to_ascii_lowercase();
    if let Some(b) = head.rfind("-----begin")
        && !head[b..].contains("-----end")
    {
        head_end = b;
    }
    let tail = masked[tail_start..].to_ascii_lowercase();
    if let Some(e) = tail.find("-----end")
        && !tail[..e].contains("-----begin")
    {
        let end = tail_start + e;
        tail_start = masked[end..].find('\n').map_or(masked.len(), |n| end + n);
    }
    // A cut the key-block rule moved into a mask moves out of it, to the side that drops it.
    for &(s, e) in &runs {
        if s < head_end && e > head_end {
            head_end = s;
        }
        if s < tail_start && e > tail_start {
            tail_start = e;
        }
    }
    if head_end >= tail_start {
        // The cuts met (a key block spans the middle): keep it whole.
        return (masked, found, None);
    }
    let marker = format!("\n…[cut: {} bytes in full]…\n", text.len());
    let moved = head_end + marker.len();
    found.retain(|f| f.offset + MASK.len() <= head_end || f.offset >= tail_start);
    for f in &mut found {
        if f.offset >= tail_start {
            f.offset = f.offset - tail_start + moved;
        }
    }
    let mut stored = masked[..head_end].to_string() + &marker + &masked[tail_start..];
    rescan(&mut stored, &mut found);
    (stored, found, Some(text.len()))
}

/// The version of the rules a finding came from: a hash of the bundled rule files.
pub fn ruleset() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest([RULES_TOML, EXTRA_TOML].concat().as_bytes());
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
    })
}

/// Secret spans in `text`: (start, end, rule index), unmerged, as gitleaks finds them.
fn spans(text: &str) -> Vec<(usize, usize, usize)> {
    let r = rules();
    let mut hit = vec![false; r.rules.len()];
    // Overlapping: "sk" (twilio) inside "gsk_" (groq) must not hide the longer keyword.
    for m in r.keywords.find_overlapping_iter(text) {
        hit[r.keyword_rule[m.pattern().as_usize()]] = true;
    }
    let mut spans = Vec::new();
    for (i, (rule, _)) in r
        .rules
        .iter()
        .zip(&hit)
        .enumerate()
        .filter(|(_, (_, h))| **h)
    {
        let Some(re) = rule.regex.as_deref().and_then(compiled) else {
            continue;
        };
        for caps in re.captures_iter(text) {
            let all = caps.get(0).expect("group 0");
            let secret = match rule.secret_group {
                Some(g) => caps.get(g),
                None => (1..caps.len())
                    .find_map(|i| caps.get(i))
                    .filter(|m| !m.is_empty()),
            }
            .unwrap_or(all);
            if let Some(min) = rule.entropy
                && shannon_entropy(secret.as_str()) <= min
            {
                continue;
            }
            let line = line_of(text, all.start());
            let allowed = rule
                .allowlists
                .iter()
                .chain(std::iter::once(&r.global))
                .any(|a| allows(a, secret.as_str(), all.as_str(), line));
            if !allowed {
                spans.push((secret.start(), secret.end(), i));
            }
        }
    }
    spans.sort_unstable();
    spans
}

/// Overlapping spans (a short and a long rule on one token) as the runs one mask covers.
fn merged(spans: &[(usize, usize, usize)]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &(start, end, _) in spans {
        match runs.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => runs.push((start, end)),
        }
    }
    runs
}

/// `text` with each run of sorted `spans` replaced by one mask, and a finding per span at the
/// offset of its mask in the result.
fn mask(text: &str, spans: Vec<(usize, usize, usize)>) -> (String, Vec<Finding>) {
    let r = rules();
    let mut out = String::with_capacity(text.len());
    let mut found = Vec::with_capacity(spans.len());
    let mut pos = 0;
    let mut i = 0;
    for (start, end) in merged(&spans) {
        out.push_str(&text[pos..start]);
        while i < spans.len() && spans[i].0 < end {
            let (s, e, rule) = spans[i];
            found.push(Finding {
                rule: r.rules[rule].id.clone(),
                offset: out.len(),
                length: e - s,
            });
            i += 1;
        }
        out.push_str(MASK);
        pos = end;
    }
    out.push_str(&text[pos..]);
    (out, found)
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

    fn token() -> String {
        format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g") // split: secret scanners
    }

    /// Every finding points at a mask in the stored text, and none holds the value.
    fn check(stored: &str, found: &[Finding], secret: &str) {
        assert!(!stored.contains(secret));
        assert!(!format!("{found:?}").contains(secret));
        for f in found {
            assert_eq!(&stored[f.offset..f.offset + MASK.len()], MASK, "{f:?}");
        }
    }

    #[test]
    fn every_byte_is_scanned_and_the_ledger_never_holds_the_value() {
        let key = token();
        let text = "x".repeat(200_000) + " Authorization: Bearer " + &key; // past v1's 12,000
        let (masked, found) = scan(&text);
        // Two rules may find the token (github-pat and a bearer rule): one mask, a finding each.
        assert!(
            found
                .iter()
                .any(|f| f.rule == "github-pat" && f.length == key.len()),
            "{found:?}"
        );
        assert!(found.iter().all(|f| f.offset == found[0].offset));
        assert_eq!(masked.matches(MASK).count(), 1);
        check(&masked, &found, &key);
        let (capped, again, cut) = scan_capped(&text, 256 * 1024);
        assert_eq!((capped, again, cut), (masked, found, None));
    }

    #[test]
    fn a_secret_across_a_cut_is_masked_in_what_is_kept() {
        let key = token();
        let cap = 64 * 1024;
        for at in [cap / 2 - 10, cap / 2 + 3, 5 * cap - cap / 2 - 10] {
            // The token straddles the head's cut, sits just past it, or straddles the tail's.
            let mut text = "y ".repeat(5 * cap / 2);
            text.replace_range(at..at + key.len() + 7, &format!("Bearer {key}"));
            let (stored, found, cut) = scan_capped(&text, cap);
            assert_eq!(cut, Some(text.len()));
            assert!(stored.len() < cap + 100);
            for i in 8..=key.len() {
                assert!(
                    !stored.contains(&key[i - 8..i]),
                    "fragment ending at {i}, token at {at}"
                );
            }
            check(&stored, &found, &key);
        }
    }

    #[test]
    fn a_key_block_cut_in_half_is_dropped_from_the_part_that_holds_it() {
        let cap = 64 * 1024;
        let block = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            "MIIEowIBAAKCAQEAq9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD\n".repeat(200)
        );
        // BEGIN before the head's cut, END past the margin: the rule never sees the whole block.
        let text = "h ".repeat(cap / 4 - 100) + &block + &"t ".repeat(cap);
        let (stored, _, cut) = scan_capped(&text, cap);
        assert!(cut.is_some());
        assert!(
            !stored.contains("MIIEowIBAAKCAQ") && !stored.contains("-----BEGIN"),
            "{}",
            &stored[..300]
        );
        // The same block across the tail's cut.
        let text = "h ".repeat(cap) + &block + &" t".repeat(cap / 4 - 100);
        let (stored, _, _) = scan_capped(&text, cap);
        assert!(!stored.contains("MIIEowIBAAKCAQ") && !stored.contains("-----END"));
    }

    #[test]
    fn a_rule_that_needs_context_far_from_the_cut_still_masks() {
        // curl-auth-user reads the whole line: `curl` is far outside any window around the cut.
        let cap = 64 * 1024;
        let pass = "usr:q9Zx8mL2vB4nR7tYw";
        let text = "p\n".repeat(cap)
            + "curl"
            + &" ".repeat(40_000)
            + &format!("-u '{pass}'\n")
            + &"t\n".repeat(cap / 8);
        let (stored, found, cut) = scan_capped(&text, cap);
        assert!(cut.is_some());
        assert!(
            !stored.contains("q9Zx8mL2vB4nR7tYw"),
            "the tail kept the password"
        );
        check(&stored, &found, pass);
    }

    #[test]
    fn every_credential_on_a_greedy_line_is_masked() {
        // curl-auth-user's `.*` reaches the last `-u` of a line: one pass finds one credential.
        let creds = [
            "usr:q9Zx8mL2vB4nR7tYw",
            "adm:K3pS6dJ0aF5hU2cE",
            "ops:Z6bD4kM7oQ1sV3xa",
            "dev:W8eR2tY6uI0pL4k",
        ];
        let line: String = creds
            .iter()
            .map(|c| format!("curl -u '{c}' https://x ; "))
            .collect();
        let (stored, found) = scan(&line);
        for c in creds {
            assert!(!stored.contains(c), "{c} kept: {stored}");
        }
        check(&stored, &found, creds[0]);
        assert!(spans(&stored).is_empty());
        // The same across a cut: head, middle and tail each hold some.
        let cap = 64 * 1024;
        let text = line.clone() + &" ".repeat(3 * cap) + &line + "\n" + &"t".repeat(cap / 4);
        let (stored, found, cut) = scan_capped(&text, cap);
        assert!(cut.is_some());
        for c in creds {
            assert!(!stored.contains(c), "{c} kept across the cut");
        }
        check(&stored, &found, creds[1]);
        assert!(spans(&stored).is_empty());
        // More than MAX_PASSES on one line: the whole text is masked.
        let line: String = (0..MAX_PASSES + 6)
            .map(|i| format!("curl -u '{}' https://x ; ", creds[i % 4]))
            .collect();
        let (stored, found) = scan(&line);
        for c in creds {
            assert!(!stored.contains(c), "{c} kept past the pass limit");
        }
        assert_eq!(stored, MASK);
        check(&stored, &found, creds[2]);
    }

    #[test]
    fn every_header_after_one_curl_is_masked() {
        // curl-auth-header's lazy `.*?` finds one header per `curl` per pass, and only within five
        // lines of it; generic-basic-auth needs no `curl`.
        let values: Vec<String> = (0..70)
            .map(|i| format!("dXNyOnE5Wng4bUwy{i:02}dkI0blI3dFl3"))
            .collect();
        for n in [3, 66, 67, 70] {
            for sep in [" ", " \\\n  "] {
                let sets: String = values[..n]
                    .iter()
                    .map(|v| {
                        format!("-H 'Authorization: Basic {v}' https://x.invalid/{sep}--next ")
                    })
                    .collect();
                let text = format!("curl {sets}");
                let (stored, found) = scan(&text);
                for v in &values[..n] {
                    assert!(!stored.contains(v.as_str()), "n={n} {v} kept");
                }
                check(&stored, &found, &values[0]);
                assert!(spans(&stored).is_empty());
            }
        }
    }

    #[test]
    fn a_credential_whose_quote_the_cut_would_drop_is_masked_first() {
        // Cut first, the head would end inside the quotes and curl-auth-user would not match.
        let cap = 64 * 1024;
        let half = cap / 2;
        let first = "curl -u 'alice:q9Zx8mL2vB4nR7tY1wK3pS6d";
        let text = " ".repeat(half - first.len())
            + first
            + "' ; "
            + &" ".repeat(cap)
            + "curl -u 'bobby:r8Wy7nK3uC5oQ6sX2vJ4pR9e'\n"
            + &"t".repeat(half);
        let (stored, found, cut) = scan_capped(&text, cap);
        assert!(cut.is_some());
        check(&stored, &found, "q9Zx8mL2vB4nR7tY1wK3pS6d");
        assert!(stored.len() <= cap + 64);
    }

    #[test]
    fn a_key_block_in_lowercase_is_dropped_too() {
        let cap = 64 * 1024;
        let block = format!(
            "-----begin rsa private key-----\n{}\n-----End RSA Private Key-----",
            "MIIEowIBAAKCAQEAq9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD\n".repeat(700)
        );
        for text in [
            "h ".repeat(cap / 4 - 100) + &block + &"t ".repeat(cap),
            "h ".repeat(cap) + &block + &" t".repeat(cap / 4 - 100),
        ] {
            let (stored, _, _) = scan_capped(&text, cap);
            assert!(!stored.contains("MIIEowIBAAKCAQ"), "{}", &stored[..200]);
        }
    }

    #[test]
    fn cuts_fall_on_character_boundaries() {
        let text = "日本語".repeat(40_000); // 360,000 bytes, three per character
        let (stored, found, cut) = scan_capped(&text, 64 * 1024 + 1);
        assert!(cut.is_some() && found.is_empty());
        assert!(stored.starts_with('日') && stored.ends_with('語'));
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
