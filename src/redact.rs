//! Mask secrets before text is stored or sent anywhere. The rules are gitleaks' own
//! (`config/gitleaks.toml`, MIT) and are applied the way gitleaks applies them: a rule's
//! keywords gate its regex (one case-insensitive Aho-Corasick pass), the regex is compiled only
//! when a keyword hits, the secret is the first capture group, and entropy plus allowlists
//! filter the candidates. Rules that depend on a file path are skipped: hook text has none.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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

struct Bundled {
    rules: Vec<Rule>,
    global: Allow,
    keywords: AhoCorasick,
    /// Aho-Corasick pattern index → rule index.
    keyword_rule: Vec<usize>,
}

fn bundled() -> &'static Bundled {
    static RULES: OnceLock<Bundled> = OnceLock::new();
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
        Bundled {
            rules,
            global: file.allowlist,
            keywords,
            keyword_rule,
        }
    })
}

/// The rules one scan applies: the bundled ones, which cannot be removed, plus the user's
/// (`[redaction]` in config.toml, spec 1.5 and 6.4).
#[derive(Clone)]
pub struct Rules {
    extra: Vec<Extra>,
    /// SHA-256 (lowercase hex) of each exact value the user keeps.
    allow: HashSet<String>,
    version: String,
}

#[derive(Clone)]
struct Extra {
    /// `user:` and the user's id, so it can never read as a bundled rule in the ledger.
    id: String,
    regex: Regex,
    keywords: Vec<String>,
    entropy: Option<f64>,
    secret_group: Option<usize>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            extra: Vec::new(),
            allow: HashSet::new(),
            version: bundled_version().to_owned(),
        }
    }
}

impl Rules {
    /// The user's rules checked here, so a mistake is an error rather than a rule that never runs.
    pub fn new(r: &crate::config::Redaction) -> anyhow::Result<Self> {
        use anyhow::{bail, ensure};
        let mut extra = Vec::new();
        for rule in &r.extra_rules {
            ensure!(
                !rule.id.is_empty()
                    && rule
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                "[redaction] extra rule id {:?}: use letters, digits, '-', '_' or '.'",
                rule.id
            );
            let id = format!("user:{}", rule.id);
            if extra.iter().any(|e: &Extra| e.id == id) {
                bail!("[redaction] extra rule id {:?} is used twice", rule.id);
            }
            // Compiled as the bundled rules are (gitleaks' ASCII `\w`, `\b`), so `\bacme-` still
            // matches after a Japanese character. The regex error quotes the pattern, which may
            // hold the very value to hide.
            let regex = compiled(&rule.regex).ok_or_else(|| {
                anyhow::anyhow!(
                    "[redaction] extra rule {:?}: its regex does not compile",
                    rule.id
                )
            })?;
            ensure!(
                rule.entropy.is_none_or(f64::is_finite),
                "[redaction] extra rule {:?}: entropy must be a finite number",
                rule.id
            );
            if let Some(g) = rule.secret_group {
                ensure!(
                    g < regex.captures_len(),
                    "[redaction] extra rule {:?}: secret_group {g}, but the regex has {} group(s)",
                    rule.id,
                    regex.captures_len() - 1
                );
            }
            extra.push(Extra {
                id,
                regex,
                keywords: rule.keywords.iter().map(|k| k.to_lowercase()).collect(),
                entropy: rule.entropy,
                secret_group: rule.secret_group,
            });
        }
        let mut allow = HashSet::new();
        for (i, a) in r.allowlist.iter().enumerate() {
            // Never the entry itself: a value pasted in place of its hash would be printed.
            ensure!(
                a.len() == 64 && a.bytes().all(|b| b.is_ascii_hexdigit()),
                "[redaction] allowlist entry {}: expected the SHA-256 of a value, 64 hex digits",
                i + 1
            );
            allow.insert(a.to_ascii_lowercase());
        }
        let version = if extra.is_empty() && allow.is_empty() {
            bundled_version().to_owned()
        } else {
            // Canonical: the order of rules, keywords or allowlist entries in the file does not
            // change the version; anything that changes what is masked does (Task 7 rescans).
            let mut rules: Vec<String> = r
                .extra_rules
                .iter()
                .map(|x| {
                    let mut x = x.clone();
                    x.keywords.sort();
                    serde_json::to_string(&x).expect("a rule serializes")
                })
                .collect();
            rules.sort();
            let mut kept: Vec<&String> = allow.iter().collect();
            kept.sort();
            let user = serde_json::json!({"rules": rules, "allowlist": kept}).to_string();
            short_hash(&[RULES_TOML, EXTRA_TOML, "\0", &user].concat())
        };
        Ok(Self {
            extra,
            allow,
            version,
        })
    }

    /// The `[redaction]` table of `<home>/config.toml`, checked.
    pub fn load(home: &std::path::Path) -> anyhow::Result<Self> {
        Self::new(&crate::config::load_capture(home)?.redaction)
    }

    /// The version a finding came from (the ledger's `ruleset`): the bundled files' hash when
    /// the user adds nothing, so a store upgraded without settings keeps its version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// For doctor: how many rules and kept values the user added.
    pub fn counts(&self) -> (usize, usize) {
        (self.extra.len(), self.allow.len())
    }

    /// ponytail: the value is hashed as it appears in the view being scanned, so a false positive
    /// holding `\"` or `\n` inside a flattened tool field needs the hash of that escaped form.
    fn keeps(&self, secret: &str) -> bool {
        !self.allow.is_empty() && self.allow.contains(&short_hash_full(secret))
    }

    fn name(&self, rule: usize) -> &str {
        let b = &bundled().rules;
        match b.get(rule) {
            Some(r) => &r.id,
            None => &self.extra[rule - b.len()].id,
        }
    }
}

/// The rules egress uses: those of `<home>/config.toml` as it is at each call (spec 6.4), so a
/// long-running `oboete mcp` picks up a rule added or a kept value removed. `None` while the
/// table is wrong or unreadable.
struct Egress {
    home: std::path::PathBuf,
    /// The file as last read: `None` when it is absent.
    seen: Option<Option<String>>,
    rules: Option<std::sync::Arc<Rules>>,
}

impl Egress {
    fn rules(&mut self) -> Option<std::sync::Arc<Rules>> {
        let now = match std::fs::read_to_string(self.home.join("config.toml")) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                self.seen = None;
                return None;
            }
        };
        if self.seen.as_ref() != Some(&now) {
            self.rules = crate::config::parse_capture(now.as_deref())
                .and_then(|c| Rules::new(&c.redaction))
                .ok()
                .map(std::sync::Arc::new);
            self.seen = Some(now);
        }
        self.rules.clone()
    }
}

static EGRESS: std::sync::Mutex<Option<Egress>> = std::sync::Mutex::new(None);

/// Called by `main` before any text can leave: the home whose config egress follows. An error
/// when its `[redaction]` table is wrong, so the command stops before sending anything.
pub fn set_home(home: &std::path::Path) -> anyhow::Result<()> {
    Rules::load(home)?;
    *EGRESS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Egress {
        home: home.to_owned(),
        seen: None,
        rules: None,
    });
    Ok(())
}

/// Egress's rules now; the bundled ones where no home was set (tests).
fn egress() -> Option<std::sync::Arc<Rules>> {
    match EGRESS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        Some(e) => e.rules(),
        None => Some(std::sync::Arc::new(Rules::default())),
    }
}

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn short_hash_full(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
/// The user's rules apply here as they are now (spec 6.4), so a rule added after capture still
/// stops the text leaving.
/// A table that went wrong after the process started sends nothing of the text: all of it is
/// masked, and doctor names the mistake.
pub fn outbound(text: &str) -> String {
    match egress() {
        Some(rules) => outbound_with(text, &rules),
        None => MASK.to_string(),
    }
}

pub fn outbound_with(text: &str, rules: &Rules) -> String {
    scan(&crate::hook::strip_blocks(text, false), rules).0
}

/// v1's write path (`hook::clip`, agents not yet in `capture::PORTED`): the bundled rules only.
/// Those agents move to `capture`, and its settings, in Task 2b.
pub fn redact(text: &str) -> String {
    scan(text, &Rules::default()).0
}

/// One secret masked in stored text: the rule that found it, where the mask hiding it starts in
/// the stored (masked) text, and the secret's own length as stored (with any JSON escapes in it),
/// both in bytes. Never the value. Two
/// rules on one token share one mask, so they give two findings at the same offset.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: String,
    pub offset: usize,
    pub length: usize,
}

/// `text` masked, with its findings. `spans` finds every secret with the text's own context;
/// the pass over the masked text after it can only add masks.
pub fn scan(text: &str, rules: &Rules) -> (String, Vec<Finding>) {
    let (mut masked, mut found) = mask(text, spans(text, rules), rules);
    if !found.is_empty() {
        rescan(&mut masked, &mut found, rules);
        coalesce(&mut masked, &mut found);
    }
    (masked, found)
}

/// Past this many findings in one text, the text is one mask.
const MAX_FINDINGS: usize = 1_000;
/// Past this many matches one user rule looks at in one text (found or passed over: blank,
/// masked, below its entropy), the text is one mask too, so a rule that matches at every
/// position (`a()` over a long run of `a`) costs a bounded time.
const MAX_LOOKS: usize = 10 * MAX_FINDINGS;

/// One rule's spans that masking merged into one mask are one finding (their lengths summed), so
/// a broad rule (`.`) gives a ledger row per mask, not per character. Past `MAX_FINDINGS` the
/// whole text is one mask with one finding per rule: a hook never writes an unbounded ledger.
fn coalesce(masked: &mut String, found: &mut Vec<Finding>) {
    found.sort_by(|a, b| (a.offset, &a.rule).cmp(&(b.offset, &b.rule)));
    found.dedup_by(|later, kept| {
        let same = later.offset == kept.offset && later.rule == kept.rule;
        if same {
            kept.length += later.length;
        }
        same
    });
    if found.len() > MAX_FINDINGS {
        *masked = MASK.to_string();
        let mut per: std::collections::BTreeMap<String, usize> = Default::default();
        for f in found.drain(..) {
            *per.entry(f.rule).or_default() += f.length;
        }
        found.extend(per.into_iter().map(|(rule, length)| Finding {
            rule,
            offset: 0,
            length,
        }));
    }
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
fn rescan(masked: &mut String, found: &mut Vec<Finding>, rules: &Rules) {
    for pass in 0..=MAX_PASSES {
        let again = spans(masked, rules);
        if again.is_empty() {
            break;
        }
        if pass == MAX_PASSES {
            found.extend(mask(masked, again, rules).1);
            for f in found.iter_mut() {
                f.offset = 0;
            }
            *masked = MASK.to_string();
            break;
        }
        let runs = merged(&again);
        let (next, more) = mask(masked, again, rules);
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
pub fn scan_capped(text: &str, cap: usize, rules: &Rules) -> (String, Vec<Finding>, Option<usize>) {
    let (masked, mut found) = scan(text, rules);
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
    if let Some(b) = key_markers(&head, "-----begin").last()
        && key_markers(&head[b..], "-----end").next().is_none()
    {
        head_end = b;
    }
    let tail = masked[tail_start..].to_ascii_lowercase();
    if let Some(e) = key_markers(&tail, "-----end").next()
        && key_markers(&tail[..e], "-----begin").next().is_none()
    {
        let end = tail_start + e;
        let rest = &masked[end..];
        // The footer's line ends at a line break, or at `\n` in flattened JSON.
        let eol = [rest.find('\n'), rest.find("\\n")]
            .into_iter()
            .flatten()
            .min();
        tail_start = eol.map_or(masked.len(), |n| end + n);
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
    rescan(&mut stored, &mut found, rules);
    (stored, found, Some(text.len()))
}

/// Offsets of the PEM markers (`-----begin` or `-----end`, in lowercased text) that may open or
/// close a private key: a label naming one, as the bundled private-key rule's header does, or a
/// label not closed by `-----` on its line (cut short upstream, like `-----end rsa priva`). A
/// complete label naming something else (a certificate) is not a secret.
fn key_markers<'a>(s: &'a str, marker: &'a str) -> impl Iterator<Item = usize> + 'a {
    s.match_indices(marker).map(|(i, _)| i).filter(move |&i| {
        // A line ends at a line break, or at `\n` in flattened JSON.
        let rest = &s[i + marker.len()..];
        let line = rest.split(['\n', '\r']).next().unwrap_or("");
        let line = line.split("\\n").next().unwrap_or("");
        line.find("-----")
            .is_none_or(|n| line[..n].contains("private key"))
    })
}

/// The bundled rules' version: a hash of their files.
fn bundled_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| short_hash(&[RULES_TOML, EXTRA_TOML].concat()))
}

/// Secret spans in `text`: (start, end, rule index), unmerged, in `text`'s offsets; a value the
/// user keeps is never one. Every rule looks at the text once; a rule that found or kept
/// something then looks again on its own until it finds nothing new (`fixpoint`).
fn spans(text: &str, rules: &Rules) -> Vec<(usize, usize, usize)> {
    let mut kept = Vec::new();
    let first = views(text, rules, None, &mut kept);
    let mut again: Vec<usize> = first.iter().chain(&kept).map(|x| x.2).collect();
    again.sort_unstable();
    again.dedup();
    let mut all = Vec::new();
    for r in again {
        let own = |x: &&(usize, usize, usize)| x.2 == r;
        let found: Vec<_> = first.iter().filter(own).copied().collect();
        let kept: Vec<_> = kept.iter().filter(own).copied().collect();
        all.extend(fixpoint(text, rules, r, found, &kept));
    }
    all.sort_unstable();
    all.dedup();
    all
}

/// Rule `rule` to its fixpoint, from what it `found` and `kept` in its first look. A rule finds
/// one secret per context per look (curl-auth-user's greedy `.*` takes the last `-u` of a line,
/// curl-auth-header's lazy `.*?` the first header after a `curl`), so each look is at a copy of
/// `text` where what this rule found or kept before is blanked out with spaces of the same length:
/// offsets stay those of `text`, and no other rule's finding is blanked, so no rule takes context
/// another needs. Still finding after `MAX_PASSES`, the whole text is one span: a look never stops
/// with a secret left unlooked-at.
fn fixpoint(
    text: &str,
    rules: &Rules,
    rule: usize,
    mut found: Vec<(usize, usize, usize)>,
    kept: &[(usize, usize, usize)],
) -> Vec<(usize, usize, usize)> {
    let mut work = text.to_owned();
    let mut blank: Vec<(usize, usize)> = Vec::new();
    let mut new = found.clone();
    let mut new_kept = kept.to_vec();
    for _ in 0..=MAX_PASSES {
        for &(s, e, _) in new.iter().chain(&new_kept) {
            work.replace_range(s..e, &" ".repeat(e - s));
            blank.push((s, e));
        }
        new_kept.clear();
        new = views(&work, rules, Some(rule), &mut new_kept);
        let blanked =
            |&(s, e, _): &(usize, usize, usize)| blank.iter().any(|&(a, b)| a <= s && e <= b);
        new.retain(|x| !blanked(x));
        new_kept.retain(|x| !blanked(x));
        if new.is_empty() && new_kept.is_empty() {
            return found;
        }
        found.extend(&new);
    }
    vec![(0, text.len(), rule)]
}

/// One look of every rule, or of rule `only`, at `text` and at its JSON-unescaped view, mapped
/// back onto `text`. A tool
/// field is stored as flattened JSON, where `\"` and `\n` hide the quotes and line breaks rules
/// match on (curl's `-u "user:pass"`, a quoted header), and the whole output reads as one line to
/// a line-scoped allowlist. Scanning both views can only add masks.
fn views(
    text: &str,
    rules: &Rules,
    only: Option<usize>,
    kept: &mut Vec<(usize, usize, usize)>,
) -> Vec<(usize, usize, usize)> {
    let mut all = spans_in(text, rules, only, kept);
    if text.contains('\\') {
        let (view, at) = unescaped(text);
        let mut kept_view = Vec::new();
        all.extend(
            spans_in(&view, rules, only, &mut kept_view)
                .into_iter()
                .map(|(s, e, r)| (at[s], at[e], r)),
        );
        kept.extend(kept_view.into_iter().map(|(s, e, r)| (at[s], at[e], r)));
    }
    all
}

/// `text` with one level of JSON string escapes decoded, and for each byte of the result the
/// offset in `text` where the character it belongs to starts (one more entry: `text.len()`).
/// ponytail: one level; a double-encoded string (JSON inside a JSON string inside a field) keeps
/// its inner `\"`. Decode again if such payloads show up.
fn unescaped(text: &str) -> (String, Vec<usize>) {
    let mut view = String::with_capacity(text.len());
    let mut at = Vec::with_capacity(text.len() + 1);
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let (c, n) = escape(rest).unwrap_or_else(|| {
            let c = rest.chars().next().expect("i is a char boundary");
            (c, c.len_utf8())
        });
        at.extend(std::iter::repeat_n(i, c.len_utf8()));
        view.push(c);
        i += n;
    }
    at.push(text.len());
    (view, at)
}

/// The character a JSON escape at the start of `s` stands for, and the escape's length.
fn escape(s: &str) -> Option<(char, usize)> {
    let c = match s.as_bytes().get(..2)? {
        b"\\\"" => '"',
        b"\\\\" => '\\',
        b"\\/" => '/',
        b"\\n" => '\n',
        b"\\r" => '\r',
        b"\\t" => '\t',
        b"\\b" => '\u{8}',
        b"\\f" => '\u{c}',
        b"\\u" => {
            let hex = s
                .get(2..6)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
            return char::from_u32(u32::from_str_radix(hex, 16).ok()?).map(|c| (c, 6));
        }
        _ => return None,
    };
    Some((c, 2))
}

/// Secret spans in `text`: (start, end, rule index), unmerged, as gitleaks finds them.
/// Secret spans in `text` found by every rule, or by rule `only`, as gitleaks finds them; `kept`
/// gets the spans of the values the user keeps.
fn spans_in(
    text: &str,
    rules: &Rules,
    only: Option<usize>,
    kept: &mut Vec<(usize, usize, usize)>,
) -> Vec<(usize, usize, usize)> {
    let r = bundled();
    let mut spans = user_spans_in(text, rules, only, kept);
    if only.is_some_and(|o| o >= r.rules.len()) {
        spans.sort_unstable();
        return spans;
    }
    let mut hit = vec![false; r.rules.len()];
    // Overlapping: "sk" (twilio) inside "gsk_" (groq) must not hide the longer keyword.
    for m in r.keywords.find_overlapping_iter(text) {
        hit[r.keyword_rule[m.pattern().as_usize()]] = true;
    }
    for (i, (rule, _)) in r
        .rules
        .iter()
        .zip(&hit)
        .enumerate()
        .filter(|(i, (_, h))| **h && only.is_none_or(|o| o == *i))
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
            if allowed {
                continue;
            }
            if rules.keeps(secret.as_str()) {
                kept.push((secret.start(), secret.end(), i));
            } else {
                spans.push((secret.start(), secret.end(), i));
            }
        }
    }
    spans.sort_unstable();
    spans
}

/// The user's rules: a rule without keywords always runs (gitleaks does the same).
fn user_spans_in(
    text: &str,
    rules: &Rules,
    only: Option<usize>,
    kept: &mut Vec<(usize, usize, usize)>,
) -> Vec<(usize, usize, usize)> {
    let r = bundled();
    let mut spans = Vec::new();
    let mut lower: Option<String> = None;
    for (j, x) in rules.extra.iter().enumerate() {
        if only.is_some_and(|o| o != r.rules.len() + j) {
            continue;
        }
        if !x.keywords.is_empty() {
            let lower = lower.get_or_insert_with(|| text.to_lowercase());
            if !x.keywords.iter().any(|k| lower.contains(k.as_str())) {
                continue;
            }
        }
        let rule = r.rules.len() + j;
        let mut regions = vec![(0, text.len())];
        let mut matched = 0;
        let (mut regions_seen, mut looks) = (0, 0);
        'regions: while let Some((from, to)) = regions.pop() {
            // Past the limit of regions or of matches too, the text is masked whole (below).
            regions_seen += 1;
            if regions_seen > MAX_FINDINGS {
                matched = MAX_FINDINGS + 1;
                break;
            }
            for caps in x.regex.captures_iter(&text[from..to]) {
                looks += 1;
                if looks > MAX_LOOKS {
                    matched = MAX_FINDINGS + 1;
                    break 'regions;
                }
                let all = caps.get(0).expect("group 0");
                let secret = match x.secret_group {
                    Some(g) => caps.get(g),
                    None => (1..caps.len())
                        .find_map(|i| caps.get(i))
                        .filter(|m| !m.is_empty()),
                }
                .unwrap_or(all);
                let value = secret.as_str().trim();
                let plain = x
                    .entropy
                    .is_some_and(|min| shannon_entropy(secret.as_str()) <= min);
                if value.is_empty() || value == MASK || plain {
                    // A value this rule does not mask: blanked by the fixpoint, masked in a
                    // rescan, or below its entropy. A greedy pattern that ends on it passes over
                    // one before it, so this match is looked at again up to that value.
                    // Only a smaller region: an empty group at the end of a match over the whole
                    // region would give the same one again, forever.
                    let before = (from + all.start(), from + secret.start());
                    if secret.start() > all.start() && before != (from, to) {
                        regions.push(before);
                    }
                    continue;
                }
                let at = (from + secret.start(), from + secret.end(), rule);
                if rules.keeps(secret.as_str()) {
                    kept.push(at);
                } else {
                    spans.push(at);
                }
                matched += 1;
                if matched > MAX_FINDINGS {
                    break 'regions;
                }
            }
        }
        // A rule that matches past `MAX_FINDINGS` times (`.`, a letter) masks the whole text as
        // one span, before a span per match is held in memory.
        if matched > MAX_FINDINGS {
            spans.retain(|x| x.2 != rule);
            kept.retain(|x| x.2 != rule);
            spans.push((0, text.len(), rule));
        }
    }
    spans.sort_unstable();
    spans
}

/// The byte ranges of `text` the rules find now, merged, from both of its views as capture scans
/// them. What capture masked, or a tombstone starred, is no finding
/// (`a_rescan_finds_nothing_new_in_what_capture_masked`).
pub fn ranges(text: &str, rules: &Rules) -> Vec<(usize, usize)> {
    merged(&spans(text, rules))
}

/// The content of each JSON string literal of `text`, keys too, as byte ranges of `text`.
/// Capture scans a body field by field, so a rule anchored to a field's start or end (`^`, `$`)
/// is only ever matched against one field.
fn literals(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let (mut out, mut open, mut i) = (Vec::new(), None, 0);
    while i < b.len() {
        match (b[i], open) {
            (b'\\', Some(_)) => i += 1,
            (b'"', None) => open = Some(i + 1),
            (b'"', Some(start)) => {
                out.push((start, i));
                open = None;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Task 7's rescan: `ranges` of a stored body, field by field as capture scanned it, as byte
/// ranges of the body.
pub fn field_ranges(body: &str, rules: &Rules) -> Vec<(usize, usize)> {
    literals(body)
        .into_iter()
        .flat_map(|(s, e)| {
            ranges(&body[s..e], rules)
                .into_iter()
                .map(move |(a, b)| (s + a, s + b))
        })
        .collect()
}

/// The egress gate on indexed text (field values one per line): whole, then line by line, so a
/// rule anchored to a field's end (`$`) matches each field as capture's did.
pub fn outbound_lines(text: &str) -> String {
    outbound(text)
        .split('\n')
        .map(outbound)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The egress gate on a stored body, field by field as capture scanned it.
pub fn outbound_fields(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut pos = 0;
    for (s, e) in literals(body) {
        out.push_str(&body[pos..s]);
        out.push_str(&outbound(&body[s..e]));
        pos = e;
    }
    out.push_str(&body[pos..]);
    out
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
fn mask(text: &str, spans: Vec<(usize, usize, usize)>, rules: &Rules) -> (String, Vec<Finding>) {
    let mut out = String::with_capacity(text.len());
    let mut found = Vec::with_capacity(spans.len());
    let mut pos = 0;
    let mut i = 0;
    for (start, end) in merged(&spans) {
        out.push_str(&text[pos..start]);
        while i < spans.len() && spans[i].0 < end {
            let (s, e, rule) = spans[i];
            found.push(Finding {
                rule: rules.name(rule).to_owned(),
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
        let r = bundled();
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
        let (masked, found) = scan(&text, &Rules::default());
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
        let (capped, again, cut) = scan_capped(&text, 256 * 1024, &Rules::default());
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
            let (stored, found, cut) = scan_capped(&text, cap, &Rules::default());
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
        let (stored, _, cut) = scan_capped(&text, cap, &Rules::default());
        assert!(cut.is_some());
        assert!(
            !stored.contains("MIIEowIBAAKCAQ") && !stored.contains("-----BEGIN"),
            "{}",
            &stored[..300]
        );
        // The same block across the tail's cut.
        let text = "h ".repeat(cap) + &block + &" t".repeat(cap / 4 - 100);
        let (stored, _, _) = scan_capped(&text, cap, &Rules::default());
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
        let (stored, found, cut) = scan_capped(&text, cap, &Rules::default());
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
        let (stored, found) = scan(&line, &Rules::default());
        for c in creds {
            assert!(!stored.contains(c), "{c} kept: {stored}");
        }
        check(&stored, &found, creds[0]);
        assert!(spans(&stored, &Rules::default()).is_empty());
        // The same across a cut: head, middle and tail each hold some.
        let cap = 64 * 1024;
        let text = line.clone() + &" ".repeat(3 * cap) + &line + "\n" + &"t".repeat(cap / 4);
        let (stored, found, cut) = scan_capped(&text, cap, &Rules::default());
        assert!(cut.is_some());
        for c in creds {
            assert!(!stored.contains(c), "{c} kept across the cut");
        }
        check(&stored, &found, creds[1]);
        assert!(spans(&stored, &Rules::default()).is_empty());
        // More than MAX_PASSES on one line: the whole text is masked.
        let line: String = (0..MAX_PASSES + 6)
            .map(|i| format!("curl -u '{}' https://x ; ", creds[i % 4]))
            .collect();
        let (stored, found) = scan(&line, &Rules::default());
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
                let (stored, found) = scan(&text, &Rules::default());
                for v in &values[..n] {
                    assert!(!stored.contains(v.as_str()), "n={n} {v} kept");
                }
                check(&stored, &found, &values[0]);
                assert!(spans(&stored, &Rules::default()).is_empty());
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
        let (stored, found, cut) = scan_capped(&text, cap, &Rules::default());
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
            let (stored, _, _) = scan_capped(&text, cap, &Rules::default());
            assert!(!stored.contains("MIIEowIBAAKCAQ"), "{}", &stored[..200]);
        }
    }

    #[test]
    fn a_certificate_across_a_cut_keeps_the_head() {
        let cap = 64 * 1024;
        let cert = format!(
            "-----BEGIN CERTIFICATE-----\n{}-----END CERTIFICATE-----\n",
            "MIIDdzCCAl+gAwIBAgIEAgAAuTANBgkqhkiG9w0BAQUFADBaMQswCQYDVQQGEwJJRTES\n".repeat(900)
        );
        let text = "h ".repeat(cap / 4 - 100) + "kept here\n" + &cert + &"t ".repeat(cap);
        let (stored, _, cut) = scan_capped(&text, cap, &Rules::default());
        assert!(cut.is_some());
        assert!(stored.contains("kept here\n-----BEGIN CERTIFICATE-----\nMIIDdzCC"));
    }

    #[test]
    fn a_key_whose_footer_was_cut_upstream_is_dropped_from_the_tail() {
        let cap = 64 * 1024;
        let body =
            "MIIEowIBAAKCAQEAq9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD\n".repeat(700);
        let text = "h ".repeat(cap)
            + "-----BEGIN RSA PRIVATE KEY-----\n"
            + &body
            + "-----END RSA PRIVA\n"
            + &"log\n".repeat(cap / 16);
        let (stored, _, cut) = scan_capped(&text, cap, &Rules::default());
        assert!(cut.is_some());
        assert!(!stored.contains("MIIEowIBAAKCAQ"));
        assert!(stored.ends_with("log\n"));
        // Flattened into JSON, with a `-----` on the next line: that is not the label's end.
        let flat = serde_json::to_string(
            &(text.replace(
                "-----END RSA PRIVA\n",
                "-----END RSA PRIVA\n----- next -----\n",
            )),
        )
        .unwrap();
        let (stored, _, cut) = scan_capped(&flat, cap, &Rules::default());
        assert!(cut.is_some());
        assert!(!stored.contains("MIIEowIBAAKCAQ"));
        assert!(
            stored.ends_with("log\\n\""),
            "the log after the footer is kept"
        );
    }

    #[test]
    fn cuts_fall_on_character_boundaries() {
        let text = "日本語".repeat(40_000); // 360,000 bytes, three per character
        let (stored, found, cut) = scan_capped(&text, 64 * 1024 + 1, &Rules::default());
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

    fn user(toml: &str) -> anyhow::Result<Rules> {
        Rules::new(&crate::config::parse_capture(Some(toml))?.redaction)
    }

    fn sha(v: &str) -> String {
        short_hash_full(v)
    }

    #[test]
    fn a_broad_rule_gives_a_bounded_ledger() {
        let every = user("[redaction]\nextra_rules = [{ id = \"any\", regex = '.' }]").unwrap();
        let (masked, found) = scan(&"x".repeat(100_000), &every);
        assert_eq!((masked.as_str(), found.len()), (MASK, 1));
        assert_eq!(found[0].length, 100_000);
        // Bounded while matching, not only in the ledger: one span once past the limit.
        let mut kept = Vec::new();
        let text = "x".repeat(10_000);
        assert_eq!(user_spans_in(&text, &every, None, &mut kept).len(), 1);
        let each = user("[redaction]\nextra_rules = [{ id = \"a\", regex = 'a' }]").unwrap();
        let (masked, found) = scan(&"ab".repeat(5_000), &each);
        assert_eq!((masked.as_str(), found.len()), (MASK, 1));
        // Matches passed over count too: an empty group at every position ends in bounded time.
        let empty =
            user("[redaction]\nextra_rules = [{ id = \"e\", regex = 'a()', secret_group = 1 }]")
                .unwrap();
        let mut kept = Vec::new();
        let spans = user_spans_in(&"a".repeat(100_000), &empty, None, &mut kept);
        assert_eq!(
            spans.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(),
            [(0, 100_000)]
        );
    }

    #[test]
    fn a_greedy_rule_that_ends_on_a_masked_value_still_finds_the_one_before() {
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'ACME_[A-Z]+.* otp=\"([^\"]+)\"' }]",
        )
        .unwrap();
        let (masked, found) = scan(r#"ACME_CLIENT otp="123456" otp="654321""#, &rules);
        assert!(
            !masked.contains("123456") && !masked.contains("654321"),
            "{masked}"
        );
        assert_eq!(found.len(), 2);
        // A rescan of stored text, whose mask the rule matches again, looks before it too.
        let (again, _) = scan(r#"ACME_CLIENT otp="123456" otp="[REDACTED]""#, &rules);
        assert!(!again.contains("123456"), "{again}");
        // One below the rule's entropy, which the greedy match ends on, does not hide one before.
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'ACME.*otp=([A-Za-z0-9]+)', entropy = 3.0 }]",
        )
        .unwrap();
        let (masked, _) = scan("ACME otp=Zq8vN3kL7pW2 otp=aaaaaaaa", &rules);
        assert!(
            !masked.contains("Zq8vN3kL7pW2") && masked.contains("aaaaaaaa"),
            "{masked}"
        );
    }

    #[test]
    fn a_rescan_finds_nothing_new_in_what_capture_masked() {
        let rules = Rules::default();
        let ghp = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split: scanners
        let aws = format!("AKIA{}", "Z7Q2XK4M9PL3WR8T");
        let text = format!(
            "curl -u admin:Zq8vN3kL7pW2 https://h.test\n\
             curl -H \"Authorization: Bearer {ghp}\" https://h.test\n\
             password=Zq8vN3kL7pW2xY\naws_access_key_id = {aws}\n"
        );
        let (masked, found) = scan(&text, &rules);
        assert!(found.len() >= 3, "{masked}");
        assert_eq!(ranges(&masked, &rules), vec![], "{masked}");
        // A tombstone's stars are no new finding either.
        let starred = masked.replace(MASK, &"*".repeat(MASK.len()));
        assert_eq!(ranges(&starred, &rules), vec![], "{starred}");
    }

    #[test]
    fn a_rule_whose_group_is_empty_at_the_end_of_its_match_ends() {
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"x\", regex = '(?s).*()', secret_group = 1 }]",
        )
        .unwrap();
        let text = "id acme-123456\nnext line";
        assert_eq!(scan(text, &rules).0, text);
    }

    #[test]
    fn a_user_rule_reads_word_boundaries_as_the_bundled_rules_do() {
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '\\bacme-[0-9]{6}\\b' }, \
             { id = \"emp\", regex = '社員番号[0-9]{4}' }]",
        )
        .unwrap();
        let (masked, _) = scan("日本語acme-123456 と 社員番号1234 です", &rules);
        assert!(
            !masked.contains("123456") && !masked.contains("1234 "),
            "{masked}"
        );
    }

    #[test]
    fn an_allowlisted_false_positive_is_kept_and_an_extra_rule_masks() {
        let kept = token();
        let other = format!("ghp_{}", "a7Kd2LmQ9xT4vB8nR1wZ6yH3jF5sP0cE2gUq"); // split: scanners
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[[providers]]\nkind = \"nonsense\"\n\n[redaction]\nallowlist = [\"{}\"]\n\
                 extra_rules = [{{ id = \"acme\", regex = 'acme-([0-9]{{6}})' }}]\n",
                sha(&kept).to_uppercase()
            ),
        )
        .unwrap();
        // A broken [[providers]] table is not capture's to judge.
        let rules = Rules::load(home.path()).unwrap();
        let text = format!("{kept} and {other} and acme-123456 and acme-12");
        let (masked, found) = scan(&text, &rules);
        assert!(masked.contains(&kept), "{masked}"); // the exact value the user keeps
        assert!(!masked.contains(&other), "{masked}"); // the same rule on another value
        assert!(
            !masked.contains("123456") && masked.contains("acme-12"),
            "{masked}"
        );
        let names: Vec<&str> = found.iter().map(|f| f.rule.as_str()).collect();
        assert!(names.contains(&"user:acme"), "{names:?}");
        assert!(!format!("{found:?}").contains("123456"));
        // The egress gate takes the same rules.
        assert!(!outbound_with("id acme-654321", &rules).contains("654321"));
    }

    #[test]
    fn a_user_rule_without_keywords_always_runs_and_one_with_them_only_on_a_hit() {
        let rules = user(
            "[redaction]\nextra_rules = [\
             { id = \"bare\", regex = 'zq-[0-9]{4}' },\
             { id = \"gated\", regex = 'yk-[0-9]{4}', keywords = [\"PROJ\"] }]",
        )
        .unwrap();
        let (masked, _) = scan("zq-1111 yk-2222", &rules);
        assert!(
            !masked.contains("1111") && masked.contains("yk-2222"),
            "{masked}"
        );
        let (masked, _) = scan("proj: yk-2222", &rules);
        assert!(!masked.contains("2222"), "{masked}");
    }

    #[test]
    fn the_version_follows_what_is_masked_and_not_the_order_in_the_file() {
        let none = user("").unwrap();
        assert_eq!(none.version(), Rules::default().version());
        assert_eq!(none.version(), bundled_version());
        let a = "{ id = \"a\", regex = 'a-[0-9]{4}', keywords = [\"x\", \"y\"] }";
        let a_reordered = "{ id = \"a\", regex = 'a-[0-9]{4}', keywords = [\"y\", \"x\"] }";
        let b = "{ id = \"b\", regex = 'b-[0-9]{4}' }";
        let h1 = sha("one");
        let h2 = sha("two");
        let v = |rules: &str, allow: &str| {
            user(&format!(
                "[redaction]\nextra_rules = [{rules}]\nallowlist = [{allow}]"
            ))
            .unwrap()
            .version()
            .to_owned()
        };
        let base = v(&format!("{a}, {b}"), &format!("\"{h1}\", \"{h2}\""));
        assert_ne!(base, bundled_version());
        assert_eq!(
            base,
            v(
                &format!("{b}, {a_reordered}"),
                &format!("\"{h2}\", \"{h1}\"")
            )
        );
        assert_ne!(base, v(&format!("{a}, {b}"), &format!("\"{h1}\""))); // an entry removed
        assert_ne!(base, v(a, &format!("\"{h1}\", \"{h2}\""))); // a rule removed
    }

    #[test]
    fn a_kept_value_does_not_hide_a_secret_its_rule_passed_over() {
        let kept = format!("dev:{}", "W8eR2tY6uI0pL4k"); // split: secret scanners
        let other = format!("usr:{}", "q9Zx8mL2vB4nR7tYw");
        let text = format!("curl -u {other} https://x ; curl -u {kept} https://x ;");
        let (masked, _) = scan(&text, &Rules::default());
        assert!(
            !masked.contains(&kept) && !masked.contains(&other),
            "{masked}"
        );
        let rules = user(&format!("[redaction]\nallowlist = [\"{}\"]", sha(&kept))).unwrap();
        let (masked, _) = scan(&text, &rules);
        assert!(masked.contains(&kept), "{masked}");
        assert!(!masked.contains(&other), "{masked}");
        // Flattened JSON, where the kept value is found in the unescaped view.
        let json = serde_json::to_string(&format!("\"{text}\"")).unwrap();
        let (masked, _) = scan(&json, &rules);
        assert!(
            masked.contains(&kept) && !masked.contains(&other),
            "{masked}"
        );
    }

    #[test]
    fn a_user_rule_never_takes_context_a_bundled_rule_needs() {
        let a = format!("usr:{}", "q9Zx8mL2vB4nR7tYw"); // split: secret scanners
        let b = format!("dev:{}", "W8eR2tY6uI0pL4k");
        let text = format!("curl -u {a} https://x ; curl -u {b} https://x ;");
        let rules =
            user("[redaction]\nextra_rules = [{ id = \"c\", regex = '\\bcurl\\b' }]").unwrap();
        let (masked, _) = scan(&text, &rules);
        assert!(!masked.contains(&a) && !masked.contains(&b), "{masked}");
        assert!(!masked.contains("curl"), "{masked}");
    }

    #[test]
    fn many_kept_values_never_let_a_secret_through() {
        let kept = format!("dev:{}", "W8eR2tY6uI0pL4k"); // split: secret scanners
        let other = format!("usr:{}", "q9Zx8mL2vB4nR7tYw");
        let rules = user(&format!("[redaction]\nallowlist = [\"{}\"]", sha(&kept))).unwrap();
        for n in [MAX_PASSES - 1, MAX_PASSES, MAX_PASSES + 1, MAX_PASSES + 2] {
            let line = format!("curl -u {other} https://x ; ")
                + &format!("curl -u {kept} https://x ; ").repeat(n);
            let json = serde_json::to_string(&line).unwrap();
            for text in [&line, &json] {
                let (masked, _) = scan(text, &rules);
                assert!(!masked.contains(&other), "n={n}: {masked}");
            }
        }
    }

    #[test]
    fn a_bundled_rule_never_takes_context_a_user_rule_needs() {
        let bearer = format!("q9Zx8mL2vB4nR7tYw{}", "K3pS6dJ0"); // split: secret scanners
        let text = format!("Authorization: Bearer {bearer}; otp=654321");
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"otp\", regex = 'Bearer [A-Za-z0-9]+; otp=([0-9]{6})' }]",
        )
        .unwrap();
        let (masked, found) = scan(&text, &rules);
        assert!(
            !masked.contains(&bearer) && !masked.contains("654321"),
            "{masked}"
        );
        assert!(found.iter().any(|f| f.rule == "user:otp"), "{found:?}");
    }

    #[test]
    fn a_rule_never_takes_context_another_rule_of_its_group_needs() {
        let rules = user(
            "[redaction]\nextra_rules = [{ id = \"name\", regex = 'ACME_[A-Z]+' }, \
             { id = \"otp\", regex = 'ACME_[A-Z]+.* otp=([0-9]{6})' }]",
        )
        .unwrap();
        let (masked, _) = scan("ACME_CLIENT otp=123456 otp=654321", &rules);
        assert!(
            !masked.contains("123456") && !masked.contains("654321"),
            "{masked}"
        );
    }

    #[test]
    fn egress_follows_the_file_as_it_changes() {
        let home = tempfile::tempdir().unwrap();
        let mut e = Egress {
            home: home.path().to_owned(),
            seen: None,
            rules: None,
        };
        let text = "id acme-123456";
        let now = |e: &mut Egress| e.rules().map(|r| outbound_with(text, &r));
        assert_eq!(now(&mut e).as_deref(), Some(text)); // no file: the bundled rules
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        assert!(!now(&mut e).unwrap().contains("123456")); // a rule added while running
        std::fs::write(
            &config,
            "[redaction]\nextra_rules = [{ id = \"x\", regex = '(' }]\n",
        )
        .unwrap();
        assert_eq!(now(&mut e), None); // wrong: nothing leaves
        std::fs::remove_file(&config).unwrap();
        assert_eq!(now(&mut e).as_deref(), Some(text));
    }

    #[test]
    fn a_settings_error_never_prints_the_value() {
        let v = format!("hunter{}", "2secret"); // split: secret scanners
        for toml in [
            format!("[redaction]\nallowlist = [\"{v}\"]"),
            format!("[redaction]\nextra_rules = [{{ id = \"x\", regex = '({v}' }}]"),
            format!("[redaction]\nallowlist = [{v}]"),
            format!("[redaction]\nallowlist = \"{v}\""),
            format!("[redaction]\nallowlist = 'prefix\"{v}'"),
            format!(
                "[redaction]\nextra_rules = [{{ id = \"x\", regex = 'x', entropy = \"{v}\" }}]"
            ),
            format!("redaction = \"{v}\""),
            format!("[ redaction ]\nallowlist = \"{v}\""),
            format!("[\"redaction\"]\nallowlist = \"{v}\""),
            format!("[[redaction.extra_rules]]\nid = \"x\"\nregex = 'x'\nentropy = \"{v}\""),
            format!("[capture]\ntool_output = \"{v}\""),
        ] {
            let e = format!("{:#}", user(&toml).err().expect(&toml));
            assert!(!e.contains(&v), "{toml}: {e}");
            // The whole config, as doctor and `oboete mcp` read it.
            let home = tempfile::tempdir().unwrap();
            std::fs::write(home.path().join("config.toml"), &toml).unwrap();
            if let Err(e) = crate::config::load(home.path()) {
                assert!(!format!("{e:#}").contains(&v), "{toml}: {e:#}");
            }
        }
        let e = format!(
            "{:#}",
            user("[capture]\nstore_prompts = \"yes\"").err().unwrap()
        );
        assert!(e.contains("line 2 is not valid"), "{e}");
    }

    #[test]
    fn a_wrong_redaction_table_is_an_error_that_names_the_mistake() {
        for (toml, says) in [
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = '(' }]",
                "regex",
            ),
            (
                "[redaction]\nextra_rules = [{ id = \"a b\", regex = 'x' }]",
                "id",
            ),
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = 'x' }, { id = \"x\", regex = 'y' }]",
                "twice",
            ),
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = 'x', secret_group = 1 }]",
                "secret_group",
            ),
            ("[redaction]\nallowlist = [\"abc\"]", "SHA-256"),
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = 'x', entropy = nan }]",
                "finite",
            ),
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = 'x', entropy = inf }]",
                "finite",
            ),
            ("[redaction]\nextra_rule = []", "line 2 is not valid"),
            (
                "[redaction]\nextra_rules = [{ id = \"x\", regex = 'x', keyword = [\"k\"] }]",
                "line 2 is not valid",
            ),
            ("[redactions]\nallowlist = []", "line 1 is not valid"),
            (
                "[capture]\n[captures]\nstore_prompts = false",
                "line 2 is not valid",
            ),
        ] {
            let e = format!("{:#}", user(toml).err().expect(toml));
            assert!(e.contains(says), "{toml}: {e}");
        }
        // The tables other commands read are no mistake here.
        let others = "[[providers]]\nname = \"x\"\n[summary]\nmax = 1\n[embedding]\nprovider = \"none\"\n\
                      [backup]\ndir = \"elsewhere\"\n";
        assert!(user(others).is_ok());
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
