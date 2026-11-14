//! Mask secrets before text is stored or sent anywhere. The rules are gitleaks' own
//! (`config/gitleaks.toml`, MIT) and are applied the way gitleaks applies them: a rule's
//! keywords gate its regex (one case-insensitive Aho-Corasick pass), the regex is compiled only
//! when a keyword hits, the secret is the first capture group, and entropy plus allowlists
//! filter the candidates. Rules that depend on a file path are skipped: hook text has none.
//! One line of the file differs from upstream: the global allowlist's `true`, `false` or `null`
//! is anchored as a whole (gitleaks#2293). A copy taken again from upstream must keep it.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::Deserialize;

pub(crate) const MASK: &str = "[REDACTED]";
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
/// (`[redaction]` in config.toml, spec 1.5 and 6.4), or none while the user turns redaction off.
#[derive(Clone)]
pub struct Rules {
    extra: Vec<Extra>,
    /// SHA-256 (lowercase hex) of each exact value the user keeps.
    allow: HashSet<String>,
    version: String,
    /// `[redaction] enabled = false` (owner decision 43): no rule finds anything.
    off: bool,
}

/// The version of the rules while redaction is off, whatever rules and kept values the user has:
/// no hash reads so, so each reader that compares versions sees the switch turn as a change of
/// rules, and turning redaction on again rescans what was stored meanwhile (docs/redaction-switch.md).
const OFF: &str = "off";

#[derive(Clone)]
struct Extra {
    /// `user:` and the user's id, so it can never read as a bundled rule in the ledger.
    id: String,
    regex: Regex,
    keywords: Vec<String>,
    entropy: Option<f64>,
    secret_group: Option<usize>,
}

/// Its version alone: no rule or kept value is shown (slice 3e-4's `raw::Judge` holds one).
impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            extra: Vec::new(),
            allow: HashSet::new(),
            version: bundled_version().to_owned(),
            off: false,
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
                regex: Regex::clone(&regex),
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
        // Checked above when off too, so a rule saved then cannot fail only once it is turned on.
        let off = r.enabled == Some(false);
        let version = if off {
            OFF.to_owned()
        } else if extra.is_empty() && allow.is_empty() {
            bundled_version().to_owned()
        } else {
            version_of(MATCHING, Some(&canonical(r, &allow)))
        };
        Ok(Self {
            extra,
            allow,
            version,
            off,
        })
    }

    /// The `[redaction]` table of `<home>/config.toml`, checked.
    pub fn load(home: &std::path::Path) -> anyhow::Result<Self> {
        Self::new(&crate::config::load_capture(home)?.redaction)
    }

    /// The version a finding came from (the ledger's `ruleset`): a hash of the bundled files and
    /// of the matching code's number (`MATCHING`) when the user adds nothing, so a store upgraded
    /// without settings, by a build with the same matching code, keeps its version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// For doctor: how many rules and kept values the user added.
    pub fn counts(&self) -> (usize, usize) {
        (self.extra.len(), self.allow.len())
    }

    /// Whether the user turned redaction off (owner decision 43).
    pub fn is_off(&self) -> bool {
        self.off
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

/// The most patterns the process keeps compiled: about three times the bundled rules' and
/// allowlists', more than a home's rules take. Past it the map starts again, so a process whose
/// rules change keeps the regexes of the rules it uses, not of every rule it has seen.
const COMPILED_CAP: usize = 1_024;

/// The regexes compiled so far, one map shared by every thread of the process, with one cell for
/// each pattern. The viewer serves each connection on a thread of its own: before M4 each thread
/// kept a map of its own and compiled the rules again; now every connection reuses them. A hit
/// takes the read lock only, to clone its cell; a pattern's cell is compiled once, outside the
/// lock, and a thread that asks for it meanwhile waits for that cell alone.
struct Compiled {
    cells: std::sync::RwLock<HashMap<String, std::sync::Arc<OnceLock<Option<Regex>>>>>,
    cap: usize,
    /// The compiles made, which the tests count.
    compiles: std::sync::atomic::AtomicUsize,
}

impl Compiled {
    fn new(cap: usize) -> Self {
        Self {
            cells: Default::default(),
            cap,
            compiles: Default::default(),
        }
    }

    /// ASCII mode first: that is what gitleaks' Go RE2 means by `\w`/`\s`/`\b`, and it compiles
    /// ~30x faster than Unicode mode for the generic rules (measured: 90 ms vs 3 ms, debug build).
    /// The few patterns ASCII mode rejects (negated classes that could match non-UTF-8) fall back;
    /// a pattern neither compiles is kept as `None`, not compiled again.
    fn get(&self, pattern: &str) -> Option<Regex> {
        use std::sync::PoisonError;
        let found = (self.cells.read().unwrap_or_else(PoisonError::into_inner))
            .get(pattern)
            .cloned();
        let cell = match found {
            Some(cell) => cell,
            None => {
                let mut cells = self.cells.write().unwrap_or_else(PoisonError::into_inner);
                if !cells.contains_key(pattern) && cells.len() >= self.cap {
                    cells.clear();
                }
                std::sync::Arc::clone(cells.entry(pattern.to_owned()).or_default())
            }
        };
        cell.get_or_init(|| {
            (self.compiles).fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            regex::RegexBuilder::new(pattern)
                .unicode(false)
                .build()
                .or_else(|_| Regex::new(pattern))
                .ok()
        })
        .clone()
    }
}

static COMPILED: std::sync::LazyLock<Compiled> =
    std::sync::LazyLock::new(|| Compiled::new(COMPILED_CAP));

/// One thread's regexes while a `Warm` lives: one clone of each regex the thread has used. A clone
/// shares the compiled program and has a cache of its own, which a search fills as it goes. Taken
/// anew for every use, as `Compiled::get` hands them out, each use built that cache from nothing
/// (W1: a fifth of a My Work read's CPU); kept, a regex's cache is built once for the thread's
/// texts.
#[derive(Default)]
struct Warmed {
    /// The `Warm`s alive on this thread.
    held: usize,
    regexes: HashMap<String, Option<Rc<Regex>>>,
    /// The clones taken, which the tests count.
    taken: usize,
}

impl Warmed {
    /// `pattern`'s regex as this thread keeps it, taken with `take` on its first use; `None` is a
    /// pattern that does not compile, and nothing else. Past `cap` the map starts again, as the
    /// process's does.
    fn get(
        &mut self,
        pattern: &str,
        cap: usize,
        take: impl FnOnce() -> Option<Regex>,
    ) -> Option<Rc<Regex>> {
        if let Some(kept) = self.regexes.get(pattern) {
            return kept.clone();
        }
        if self.regexes.len() >= cap {
            self.regexes.clear();
        }
        self.taken += 1;
        let regex = take().map(Rc::new);
        self.regexes.insert(pattern.to_owned(), regex.clone());
        regex
    }
}

thread_local! {
    static WARM: std::cell::RefCell<Warmed> = std::cell::RefCell::default();
}

/// While one lives, its thread keeps the regexes it uses (`Warmed`), and the last one to go takes
/// them with it: what one request scans, not what a resident thread has ever scanned.
#[must_use = "the regexes are kept only while this lives"]
pub(crate) struct Warm(std::marker::PhantomData<*const ()>);

/// A `Warm` for the thread that calls it.
pub(crate) fn warm() -> Warm {
    // A thread whose locals are gone keeps nothing, and `compiled` clones for it as before.
    let _ = WARM.try_with(|w| w.borrow_mut().held += 1);
    Warm(std::marker::PhantomData)
}

impl Drop for Warm {
    fn drop(&mut self) {
        let _ = WARM.try_with(|w| {
            let mut w = w.borrow_mut();
            w.held = w.held.saturating_sub(1);
            if w.held == 0 {
                w.regexes = HashMap::new();
            }
        });
    }
}

/// A pattern's regex, compiled on first use and kept until the map starts again at
/// `COMPILED_CAP` (a scan compiles every rule whose keyword hits): `Compiled::get`. While a `Warm`
/// lives on this thread it is the thread's own clone, the same one at every use; without one, or
/// on a thread whose locals are gone, it is a clone for this use, as it was before `Warm`. `None`
/// is a pattern that does not compile, on either path.
fn compiled(pattern: &str) -> Option<Rc<Regex>> {
    let kept = WARM.try_with(|w| {
        let mut w = w.borrow_mut();
        let held = w.held > 0;
        held.then(|| w.get(pattern, COMPILED_CAP, || COMPILED.get(pattern)))
    });
    match kept {
        Ok(Some(regex)) => regex,
        _ => COMPILED.get(pattern).map(Rc::new),
    }
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
    plain(&crate::hook::strip_blocks(text, false), rules).0
}

/// `text` with what the rules find in it now masked, and nothing else done to it: no block is
/// taken out, and the byte ranges in `kept` (in order, apart) stay as they are, a match that runs
/// over one masked on each side of it. For a text of parts that were each stripped of their
/// blocks and then put between tags of oboete's own (`kept`): the rules read the whole, so a
/// value one finds only beside what another part holds is masked, and the tags are not.
///
/// As `plain`: the spans are found in the text as it is, before any mask (a mask would take the
/// context another rule needs), and then the rules read what the masks left until they find
/// nothing new there (a remainder glued to a mask). Still finding after `MAX_PASSES`, the whole
/// text is one mask.
pub fn outbound_but(text: &str, kept: &[(usize, usize)]) -> String {
    match egress() {
        Some(rules) => outbound_but_with(text, kept, &rules),
        None => MASK.to_string(),
    }
}

pub fn outbound_but_with(text: &str, kept: &[(usize, usize)], rules: &Rules) -> String {
    let (mut text, mut kept) = (text.to_owned(), kept.to_vec());
    for _ in 0..=MAX_PASSES {
        let (next, moved) = masked_but(&text, &ranges(&text, rules), &kept);
        if next == text {
            return text; // nothing found, or only what is kept
        }
        (text, kept) = (next, moved);
    }
    MASK.to_string()
}

/// `text` with each of `found` (in order, apart) masked but for the parts of it in `kept`, and
/// where each of `kept` is in what comes back.
fn masked_but(
    text: &str,
    found: &[(usize, usize)],
    kept: &[(usize, usize)],
) -> (String, Vec<(usize, usize)>) {
    let mut masks = Vec::new();
    for &(from, to) in found {
        let mut start = from;
        for &(tag, end) in kept.iter().filter(|k| k.0 < to && k.1 > from) {
            if start < tag {
                masks.push((start, tag));
            }
            start = start.max(end);
        }
        if start < to {
            masks.push((start, to));
        }
    }
    let (mut out, mut moved, mut at) = (String::with_capacity(text.len()), Vec::new(), 0);
    let mut tags = kept.iter().peekable();
    // A tag is in a run that is copied: it moves by what the masks before it changed.
    let mut place = |out: &String, at: usize, before: usize, moved: &mut Vec<(usize, usize)>| {
        while let Some(&&(tag, end)) = tags.peek().filter(|t| t.0 < before) {
            moved.push((out.len() + tag - at, out.len() + end - at));
            tags.next();
        }
    };
    for (from, to) in masks {
        place(&out, at, from, &mut moved);
        out.push_str(&text[at..from]);
        out.push_str(MASK);
        at = to;
    }
    place(&out, at, usize::MAX, &mut moved);
    out.push_str(&text[at..]);
    (out, moved)
}

/// `outbound_with` of `text[range]`, one part of the whole `text` a window cut (spec 3.1, issue
/// #54): what the gate hides in the whole text (`hidden`) is hidden in the part too, also where
/// the cut splits it. `range` is on character boundaries.
pub fn outbound_part(text: &str, range: std::ops::Range<usize>, rules: &Rules) -> String {
    outbound_range(text, range, hidden(text, rules).as_deref(), rules)
}

/// `outbound_part` with `hidden(text, rules)` already found, for the many parts of one text.
pub fn outbound_range(
    text: &str,
    range: std::ops::Range<usize>,
    hidden: Option<&[(usize, usize)]>,
    rules: &Rules,
) -> String {
    let Some(hidden) = hidden else {
        return MASK.to_string();
    };
    if range == (0..text.len()) {
        return outbound_with(text, rules);
    }
    outbound_with(&masked_part(text, range, hidden), rules)
}

/// `text[range]` with each of `runs` (sorted, apart) replaced by one mask where it falls in it.
pub(crate) fn masked_part(
    text: &str,
    range: std::ops::Range<usize>,
    runs: &[(usize, usize)],
) -> String {
    let (mut part, mut pos) = (String::with_capacity(range.len()), range.start);
    for &(s, e) in runs {
        let (s, e) = (s.max(pos), e.min(range.end));
        if s < e {
            part.push_str(&text[pos..s]);
            part.push_str(MASK);
            pos = e;
        }
    }
    part.push_str(&text[pos..range.end]);
    part
}

/// The byte ranges of `text` that `outbound_with(text)` does not show: the blocks its block
/// removal takes out and every run its scan masks, rescans included, found on the text as the gate
/// sees it (blocks removed tag by tag, trimmed, then masked pass by pass) and given in `text`'s own
/// offsets, sorted and merged. `None` when the gate masks the whole text.
pub fn hidden(text: &str, rules: &Rules) -> Option<Vec<(usize, usize)>> {
    let g = gated(text, rules)?;
    Some(merged_runs([g.blocks, g.masks].concat()))
}

/// A text as the gate sees it: `plain` is what it scans (blocks removed tag by tag, trimmed,
/// nothing masked yet) and `shown` what it shows (`plain` masked pass by pass), each with, for each
/// of its bytes, the range of the text it stands for; `blocks` and `masks` are the ranges of the
/// text its block removal and its scan hide.
struct Gated {
    plain: (String, Vec<(usize, usize)>),
    shown: (String, Vec<(usize, usize)>),
    blocks: Vec<(usize, usize)>,
    masks: Vec<(usize, usize)>,
}

fn gated(text: &str, rules: &Rules) -> Option<Gated> {
    // The text as the gate sees it, and for each of its bytes the range of `text` it stands for.
    let mut work = text.to_owned();
    let mut from: Vec<(usize, usize)> = (0..text.len()).map(|i| (i, i + 1)).collect();
    let mut blocks = Vec::new();
    for tag in crate::hook::STRIP_BLOCKS {
        let found = if *tag == "claude-mem-context" {
            crate::hook::memory_context_blocks(&work)
        } else {
            crate::hook::tag_blocks(&work, tag, false).0
        };
        let (mut next, mut next_from, mut pos) = (String::new(), Vec::new(), 0);
        for (s, e) in found {
            if s < pos {
                continue; // inside a block already taken
            }
            next.push_str(&work[pos..s]);
            next_from.extend_from_slice(&from[pos..s]);
            if s < e {
                blocks.push((from[s].0, from[e - 1].1));
            }
            pos = e;
        }
        next.push_str(&work[pos..]);
        next_from.extend_from_slice(&from[pos..]);
        (work, from) = (next, next_from);
    }
    let start = work.len() - work.trim_start().len();
    let end = work.trim_end().len().max(start);
    let plain = (work[start..end].to_owned(), from[start..end].to_vec());
    let (mut work, mut from) = plain.clone();
    let (mut masks, mut found) = (Vec::new(), 0);
    for pass in 0..=MAX_PASSES + 1 {
        let again = spans(&work, rules);
        if again.is_empty() {
            break;
        }
        found += again.len();
        if pass > MAX_PASSES || found > MAX_FINDINGS {
            return None;
        }
        let (mut next, mut next_from, mut pos) = (String::new(), Vec::new(), 0);
        for (s, e) in merged(&again) {
            next.push_str(&work[pos..s]);
            next_from.extend_from_slice(&from[pos..s]);
            let run = hull(&from[s..e]);
            masks.push(run);
            next.push_str(MASK);
            next_from.extend(std::iter::repeat_n(run, MASK.len()));
            pos = e;
        }
        next.push_str(&work[pos..]);
        next_from.extend_from_slice(&from[pos..]);
        if next == work {
            break; // a rule matching its own mask: nothing new to hide
        }
        (work, from) = (next, next_from);
    }
    Some(Gated {
        plain,
        shown: (work, from),
        blocks,
        masks,
    })
}

/// Byte ranges of a text.
type Runs = Vec<(usize, usize)>;

/// The smallest range of the text that holds each of `from`'s.
fn hull(from: &[(usize, usize)]) -> (usize, usize) {
    from.iter()
        .fold((usize::MAX, 0), |(a, b), &(x, y)| (a.min(x), b.max(y)))
}

/// What `outbound_lines(text)` hides of `text`, in its offsets, unmerged: the blocks the gate
/// removes, and as masks what its scan hides and what its line pass hides in each line, both of
/// what the gate scans (before any mask, so a mask from a rule that spans two lines takes no
/// context a rule anchored to the next line needs: #315) and of what it shows (as the line pass saw
/// it before #315, so nothing it hid then is shown now). Findings of the final whole and line
/// rescans also keep their original ranges, so a cut quote or field cannot reveal them (#329).
fn hidden_lines(text: &str, rules: &Rules) -> Option<(Runs, Runs)> {
    let Gated {
        plain,
        shown,
        mut blocks,
        mut masks,
    } = gated(text, rules)?;
    // Where the whole pass masked nothing, what it shows is what it scans: one sweep.
    let views = if masks.is_empty() {
        vec![plain]
    } else {
        vec![plain, shown]
    };
    for (view, from) in views {
        let mut at = 0;
        for line in view.split('\n') {
            for (s, e) in hidden(line, rules)? {
                let run = hull(&from[at + s..at + e]);
                if run.0 < run.1 {
                    masks.push(run);
                }
            }
            at += line.len() + 1;
        }
    }
    if masks.is_empty() {
        return Some((blocks, masks));
    }
    // The final passes of `lines_with`, with origins retained even after earlier masks grew or
    // joined lines. Project before taking a quote: its cut can remove a cascade's mask context.
    let mut runs = merged_runs(masks.clone());
    for _ in 0..MAX_PASSES {
        let (masked, masked_from) = mask_map(text, &runs);
        let Gated {
            shown: (view, mut from),
            blocks: more_blocks,
            masks: more_masks,
            ..
        } = gated(&masked, rules)?;
        let source = |(s, e)| origin(&masked_from[s..e]);
        blocks.extend(more_blocks.into_iter().map(source).filter(|&(s, e)| s < e));
        masks.extend(more_masks.into_iter().map(source).filter(|&(s, e)| s < e));
        for run in &mut from {
            *run = origin(&masked_from[run.0..run.1]);
        }
        let mut at = 0;
        for line in view.split('\n') {
            let Gated {
                blocks: more_blocks,
                masks: more_masks,
                ..
            } = gated(line, rules)?;
            let source = |(s, e)| origin(&from[at + s..at + e]);
            blocks.extend(more_blocks.into_iter().map(source).filter(|&(s, e)| s < e));
            masks.extend(more_masks.into_iter().map(source).filter(|&(s, e)| s < e));
            at += line.len() + 1;
        }
        masks = merged_runs(masks);
        if masks == runs {
            return Some((blocks, masks));
        }
        runs = masks.clone();
    }
    None // Alternating whole/line cascades past the scan bound mask the view whole.
}

/// The bundled rules alone. Hooks go through `capture` and its settings.
pub fn redact(text: &str) -> String {
    scan(text, &Rules::default()).0
}

/// The cut of the claude-mem import's prompts (`hook::clip`): the bundled rules only, so `clip`'s
/// check for a key block it cuts sees every header (a user's rule could mask one: Codex's
/// security review of the switch), and none while the import's `rules` are off (owner decision
/// 43). The import gates what `clip` kept with its own rules after it.
pub fn redact_unless_off(text: &str, rules: &Rules) -> String {
    if rules.off {
        return text.to_owned();
    }
    redact(text)
}

/// One secret masked in stored text: the rule that found it, where the mask hiding it starts in
/// the stored (masked) text, and the secret's own length as stored (with any JSON escapes in it),
/// both in bytes. Never the value. Two
/// rules on one token share one mask, so they give two findings at the same offset.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub rule: String,
    pub offset: usize,
    pub length: usize,
}

/// `text` masked, with its findings, as it is stored: `plain`, and then what a rule of
/// `LOOKS_AGAIN` finds in what that left, before a value one of its matches ends on
/// (`look_again`, #537).
pub fn scan(text: &str, rules: &Rules) -> (String, Vec<Finding>) {
    let (mut masked, mut found) = plain(text, rules);
    if look_again(&mut masked, &mut found, rules) {
        coalesce(&mut masked, &mut found);
    }
    (masked, found)
}

/// `text` masked, with its findings. `spans` finds every secret with the text's own context;
/// the pass over the masked text after it can only add masks. `scan` without its looks again,
/// and the egress gate's scan (`outbound_with`), which `gated` follows pass by pass.
fn plain(text: &str, rules: &Rules) -> (String, Vec<Finding>) {
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
        if !mask_too(masked, found, again, rules) {
            break; // a rule matching its own mask: nothing new to hide
        }
    }
    found.sort_by_key(|f| f.offset);
}

/// `spans` of `masked` (sorted) masked too, and whether that changed it. Each earlier finding
/// moves by what the runs before it changed, or to the start of a new mask covering it.
fn mask_too(
    masked: &mut String,
    found: &mut Vec<Finding>,
    spans: Vec<(usize, usize, usize)>,
    rules: &Rules,
) -> bool {
    let runs = merged(&spans);
    let (next, more) = mask(masked, spans, rules);
    if next == *masked {
        return false;
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
    true
}

/// The looks again of a scan (#537): what a rule of `LOOKS_AGAIN` finds in `masked` before a
/// value one of its matches ends on (`before`) is masked too, and whether anything was.
///
/// It comes when `plain` is done with the text, and adds masks to what `plain` left. A mask made
/// earlier could take the context another rule needs for a value `plain` masks (a user's rule
/// that reads from a `curl` line to the line after it), and that value would be stored: so
/// everything `plain` masks is masked, whatever is found here.
///
/// The rules then read what the new masks left (`rescan`), and the look is repeated on that
/// while it finds something and `LOOK_AGAIN` lasts: each further look reads the whole text once
/// more, within that count or not at all. Past the count the text stays as it is then, with
/// nothing masked whole (`before_in`).
fn look_again(masked: &mut String, found: &mut Vec<Finding>, rules: &Rules) -> bool {
    let mut left = LOOK_AGAIN;
    let mut any = false;
    loop {
        let more = before(masked, rules, &mut left);
        if more.is_empty() || !mask_too(masked, found, more, rules) {
            return any;
        }
        any = true;
        rescan(masked, found, rules);
        if masked.len() > left {
            return any;
        }
        left -= masked.len();
    }
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
    // ASCII lowercasing keeps byte offsets. While redaction is off the cut keeps what it leaves
    // of a key block as typed (owner decision 43).
    let head = masked[..head_end].to_ascii_lowercase();
    if !rules.off
        && let Some(b) = key_markers(&head, "-----begin").last()
        && key_markers(&head[b..], "-----end").next().is_none()
    {
        head_end = b;
    }
    let tail = masked[tail_start..].to_ascii_lowercase();
    if !rules.off
        && let Some(e) = key_markers(&tail, "-----end").next()
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
    // The cut can bring a rule's context and a value of far lines together: as `scan`.
    look_again(&mut stored, &mut found, rules);
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

/// The matching code's own number, part of every rules version (#538): raised with each change
/// to what this file's code finds or passes over in some text, so that a store scanned before
/// the change is scanned again once. A change that leaves it as it was is never applied to what
/// is stored already (`the_matching_code_is_the_one_its_number_names` fails on any edit of the
/// file before its tests, and on a new version of a crate that matches, to ask). The first is
/// "1": before it the version had no such part.
const MATCHING: &str = "2";

/// The user's rules and kept values in one form: the order of rules, keywords or allowlist
/// entries in the file does not change it; anything that changes what is masked does (Task 7
/// rescans).
fn canonical(r: &crate::config::Redaction, allow: &HashSet<String>) -> String {
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
    serde_json::json!({"rules": rules, "allowlist": kept}).to_string()
}

/// The version of the bundled rules as the matching code `matching` applies them, with the
/// user's rules and kept values (`canonical`) when there are any.
fn version_of(matching: &str, user: Option<&str>) -> String {
    let mut parts = vec![matching, "\0", RULES_TOML, EXTRA_TOML];
    if let Some(user) = user {
        parts.extend(["\0", user]);
    }
    short_hash(&parts.concat())
}

/// The bundled rules' version: a hash of their files and of the matching code's number.
fn bundled_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| version_of(MATCHING, None))
}

/// The bundled rules' version as it was before the matching code had a number (#538): what a
/// store last scanned by such a build holds.
#[cfg(test)]
pub(crate) fn version_before_matching() -> String {
    short_hash(&[RULES_TOML, EXTRA_TOML].concat())
}

/// Secret spans in `text`: (start, end, rule index), unmerged, in `text`'s offsets; a value the
/// user keeps is never one. Every rule looks at the text once; a rule that found or kept
/// something then looks again on its own until it finds nothing new (`fixpoint`).
fn spans(text: &str, rules: &Rules) -> Vec<(usize, usize, usize)> {
    // Every scan, gate and rescan comes here: off, nothing is a secret (owner decision 43).
    if rules.off {
        return Vec::new();
    }
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
/// `text` where what this rule found or kept before is blanked out with vertical tabs of the same
/// length: offsets stay those of `text`, and no other rule's finding is blanked, so no rule takes
/// context another needs. A vertical tab is white space to `\s`, as a space was, but no space or
/// tab: a rule's gap of those (`--token VALUE next`, `KEY=VALUE` and the next line) does not take
/// the blank for its own and the word after it for another value (Codex on #826). Still finding
/// after `MAX_PASSES`, the whole text is one span: a look never stops with a secret left
/// unlooked-at.
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
            work.replace_range(s..e, &"\u{b}".repeat(e - s));
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
        // While this rule judges its values, the regexes of the allowlists are this thread's
        // own (`Warm`), from the first value that gets as far as them: taken anew for every
        // value, each use built its cache from nothing (the review of #985: 100,000 values on
        // one line that go through every entry, 16 s).
        let mut held: Option<Warm> = None;
        for caps in re.captures_iter(text) {
            let all = caps.get(0).expect("group 0");
            let secret = match rule.secret_group {
                Some(g) => caps.get(g),
                None => (1..caps.len())
                    .find_map(|i| caps.get(i))
                    .filter(|m| !m.is_empty()),
            }
            .unwrap_or(all);
            // Capture's own mask, as the user's rules pass it over: uri-userinfo-password has no
            // entropy gate, took `[REDACTED]` as a URI's password, and every rescan tombstoned it.
            if secret.as_str().trim() == MASK {
                continue;
            }
            if let Some(min) = rule.entropy
                && shannon_entropy(secret.as_str()) <= min
            {
                continue;
            }
            if held.is_none() {
                held = Some(warm());
            }
            // The line is read only for an allowlist that targets it: finding a line reads it to
            // both its ends, and one long line of values a rule passes over was read once for
            // each of them (the review of #985: 100,000 on a line of 1.7 MiB, 28 s).
            let allowed = rule
                .allowlists
                .iter()
                .chain(std::iter::once(&r.global))
                .any(|a| {
                    let line = match a.regex_target.as_deref() {
                        Some("line") => line_of(text, all.start()),
                        _ => "",
                    };
                    allows(a, secret.as_str(), all.as_str(), line)
                });
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

/// The bundled rules that look again before a value (`before`, #537): those whose pattern runs
/// greedily up to its value, so that a match takes the last value in its reach and the ones
/// before it are matched only once that one is masked. curl-auth-user's `.*` is the one there
/// is, and a test fails when a bundled rule with such a run is not listed. No other rule does:
/// the part of a match before its value is not a text a rule was written for, and another rule
/// can read it differently (authorization-header, cut off before a token it passes over, takes
/// the scheme's name for the token).
const LOOKS_AGAIN: &[&str] = &["curl-auth-user"];

/// What the looks again of one scan of a text may read past the first look at each match, in
/// bytes. Each reads the part of a match before one of its values from that part's start, so a
/// line of many values costs their number times its length: a line of some 60 values in 8 KiB is
/// read through, a longer one from its end as far back as this reaches.
const LOOK_AGAIN: usize = 256 * 1024;

/// What the rules of `LOOKS_AGAIN` find in `text` before the values their matches end on (#537),
/// in `text`'s offsets: from its unescaped view, which has the lines and the quotes of a stored
/// tool output and so comes first, and from `text` as it is, as `views` reads them. `left` is
/// what these looks may still read, one count for the caller's whole scan of the text
/// (`LOOK_AGAIN`).
fn before(text: &str, rules: &Rules, left: &mut usize) -> Vec<(usize, usize, usize)> {
    // As `spans`: off, nothing is a secret (owner decision 43).
    if rules.off {
        return Vec::new();
    }
    let r = bundled();
    let view = text.contains('\\').then(|| unescaped(text));
    let mut all = Vec::new();
    for i in (0..r.rules.len()).filter(|&i| LOOKS_AGAIN.contains(&r.rules[i].id.as_str())) {
        if let Some((view, at)) = &view {
            let found = before_in(view, rules, i, left);
            all.extend(found.into_iter().map(|(s, e, rule)| (at[s], at[e], rule)));
        }
        all.extend(before_in(text, rules, i, left));
    }
    all.sort_unstable();
    all.dedup();
    all
}

/// `before` in one view of a text, for bundled rule `i`. A match of such a rule ends on the last
/// value in its reach, whatever the rule then does with it: masks it, or passes it over (its
/// allowlist's or the global one's, one at or below its entropy, capture's own mask, one the user
/// keeps). `spans` reaches the value before that one only through `fixpoint`, which blanks what
/// the rule found or the user keeps: a value before one the rule passes over was never looked at
/// (`curl -u admin:<password> https://h.test -u bob:pass` stored the password). Here the rule
/// looks at the part of each match before its value, then at the part before the value it finds
/// there, and so on to the match's start, and every value on the way that it would mask is
/// given, the ones `spans` finds too.
///
/// The first look at the part of a match is not counted: a rule's matches do not overlap, so
/// those are one more read of the text at most. The ones after it are counted against `left`,
/// and a part longer than what is left is not read. From there the text is as it was before
/// #537, and nothing is masked whole: the long runs of passed-over values that exist are the
/// harmless ones (a script of `curl -u $USER:$PASS` lines, which one view reads as one line),
/// and a whole mask there is a tombstone over the output for good (#535's first fix, taken back).
fn before_in(text: &str, rules: &Rules, i: usize, left: &mut usize) -> Vec<(usize, usize, usize)> {
    let r = bundled();
    let rule = &r.rules[i];
    // The rule looks where `spans_in` runs it: at a text that holds one of its keywords.
    let named = r
        .keywords
        .find_overlapping_iter(text)
        .any(|m| r.keyword_rule[m.pattern().as_usize()] == i);
    let Some(re) = rule.regex.as_deref().and_then(compiled).filter(|_| named) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    // The line of the last match judged, by where the match starts: the looks at one match's
    // parts all start there, and finding a line reads it to its end (Codex on this change: 241
    // values before 7 MiB on one line read those 7 MiB 241 times).
    let mut last: Option<(usize, &str)> = None;
    // The parts of `text` to look at: the text itself (`whole`), then parts of matches.
    let mut parts = vec![(0, text.len(), true)];
    while let Some((from, to, whole)) = parts.pop() {
        for caps in re.captures_iter(&text[from..to]) {
            let all = caps.get(0).expect("group 0");
            let secret = match rule.secret_group {
                Some(g) => caps.get(g),
                None => (1..caps.len())
                    .find_map(|i| caps.get(i))
                    .filter(|m| !m.is_empty()),
            }
            .unwrap_or(all);
            let value = secret.as_str();
            // As `spans_in` passes a value over, in its order.
            let passed = value.trim() == MASK
                || rule
                    .entropy
                    .is_some_and(|min| shannon_entropy(value) <= min)
                || {
                    let at = from + all.start();
                    let line = match last {
                        Some((start, line)) if start == at => line,
                        _ => last.insert((at, line_of(text, at))).1,
                    };
                    rule.allowlists
                        .iter()
                        .chain(std::iter::once(&r.global))
                        .any(|a| allows(a, value, all.as_str(), line))
                };
            if !passed && !rules.keeps(value) {
                spans.push((from + secret.start(), from + secret.end(), i));
            }
            // The part of this match before its value. Only a smaller part than this one: an
            // empty value at the end of a match over the whole part would give it again, forever.
            let part = (from + all.start(), from + secret.start());
            let length = part.1 - part.0;
            if length == 0 || part == (from, to) {
                continue;
            }
            if whole {
                parts.push((part.0, part.1, false));
            } else if length <= *left {
                *left -= length;
                parts.push((part.0, part.1, false));
            }
        }
    }
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
/// ranges of the body. Capture scanned each field's string as decoded, not the escapes the body
/// stores it with: in those, `\n` joins lines and a pattern then matches across them (#541).
/// With them, what a rule of `LOOKS_AGAIN` finds before a value its match ends on (`before`, as
/// `scan` looks again), each field with a count of its own.
pub fn field_ranges(body: &str, rules: &Rules) -> Vec<(usize, usize)> {
    literals(body)
        .into_iter()
        .flat_map(|(s, e)| {
            let (field, at) = unescaped(&body[s..e]);
            let mut found = spans(&field, rules);
            found.extend(before(&field, rules, &mut { LOOK_AGAIN }));
            found.sort_unstable();
            merged(&found)
                .into_iter()
                .map(move |(a, b)| (s + at[a], s + at[b]))
        })
        .collect()
}

/// The egress gate on indexed text (field values one per line): whole, then line by line, so a
/// rule anchored to a field's end (`$`) matches each field as capture's did.
pub fn outbound_lines(text: &str) -> String {
    match egress() {
        Some(rules) => lines_with(text, &rules),
        None => MASK.to_string(),
    }
}

/// What the whole text's pass and each line's pass hide (`hidden_lines`: each line both before
/// and after the whole pass's masks), masked at once, then a rescan, whole and line by line, which
/// can only add masks: a mask from a rule that spans two lines takes no context a rule anchored to
/// a line needs on the next one (#315), and nothing the line pass hid before is shown.
pub(crate) fn lines_with(text: &str, rules: &Rules) -> String {
    let Some((_, masks)) = hidden_lines(text, rules) else {
        return MASK.to_string();
    };
    // The blocks stay for the gate to remove, once, as `outbound_with` does.
    outbound_with(
        &masked_part(text, 0..text.len(), &merged_runs(masks)),
        rules,
    )
    .split('\n')
    .map(|line| outbound_with(line, rules))
    .collect::<Vec<_>>()
    .join("\n")
}

/// Selected, untouched display text, with field findings kept until the composed packet's gate.
#[derive(Clone)]
pub(crate) struct Mapped {
    pub text: String,
    hidden: Option<Vec<(usize, usize)>>,
    views: [(String, Vec<(usize, usize)>); 2],
}

impl Default for Mapped {
    fn default() -> Self {
        Self {
            text: String::new(),
            hidden: Some(Vec::new()),
            views: std::array::from_fn(|_| Default::default()),
        }
    }
}

impl Mapped {
    pub fn push_str(&mut self, text: &str) {
        let at = self.text.len();
        self.text.push_str(text);
        for (view, from) in &mut self.views {
            view.push_str(text);
            from.extend((at..at + text.len()).map(|i| (i, i + 1)));
        }
    }

    pub fn append(&mut self, other: Self) {
        let at = self.text.len();
        self.text.push_str(&other.text);
        for ((view, from), (more, origins)) in self.views.iter_mut().zip(other.views) {
            view.push_str(&more);
            from.extend(
                origins
                    .into_iter()
                    .map(|(s, e)| if s < e { (at + s, at + e) } else { (0, 0) }),
            );
        }
        self.hidden = match (self.hidden.take(), other.hidden) {
            (Some(mut runs), Some(more)) => {
                runs.extend(more.into_iter().map(|(s, e)| (at + s, at + e)));
                Some(merged_runs(runs))
            }
            _ => None,
        };
    }

    /// The field-only baseline; packet findings must not make a changed claim count as shown.
    pub fn masked(&self) -> String {
        match &self.hidden {
            Some(runs) => masked_part(&self.text, 0..self.text.len(), runs),
            None => MASK.to_owned(),
        }
    }

    /// The whole packet still sees the original context, even where a field will be masked.
    /// Origins name the range of `self.text` behind each output byte, masks included.
    pub fn outbound(&self, rules: &Rules) -> (String, Vec<(usize, usize)>) {
        let mapped = || {
            let mut runs = self.hidden.clone()?;
            // Keep the field-only packet's findings independently: a packet mask can remove the
            // context a cascade written against a field's mask needs (#329).
            if !runs.is_empty() {
                let (field, field_from) = mask_map(&self.text, &runs);
                runs.extend(
                    hidden(&field, rules)?
                        .into_iter()
                        .map(|(s, e)| origin(&field_from[s..e]))
                        .filter(|&(s, e)| s < e),
                );
            }
            let Gated {
                plain: (clean, from),
                masks,
                ..
            } = gated(&self.text, rules)?;
            runs.extend(masks);
            // Field masking can move its clip or mask whitespace: the original clip and the
            // normalized selected prefix are independently scanned with their original context.
            for (view, from) in &self.views {
                runs.extend(
                    hidden(view, rules)?
                        .into_iter()
                        .map(|(s, e)| origin(&from[s..e]))
                        .filter(|&(s, e)| s < e),
                );
            }
            let runs = projected(&from, &merged_runs(runs));
            let (masked, masked_from) = mask_map(&clean, &runs);
            let (shown, shown_from) = gated(&masked, rules)?.shown;
            let origins = shown_from
                .into_iter()
                .map(|(s, e)| {
                    let (s, e) = origin(&masked_from[s..e]);
                    origin(&from[s..e])
                })
                .collect();
            Some((shown, origins))
        };
        mapped().unwrap_or_else(|| (MASK.to_owned(), vec![(0, self.text.len()); MASK.len()]))
    }
}

/// Gate independent original/line/flat views, then retain the exact prefix `clip` selects after
/// field masking. Its shadow expands masks to their original flat text without restoring blocks.
pub(crate) fn flattened_with(
    text: &str,
    rules: &Rules,
    n: usize,
    clip: fn(&str, usize) -> String,
) -> Mapped {
    let mapped = || {
        let (clean, from) = gated(text, rules)?.plain;
        let identity: Vec<_> = (0..clean.len()).map(|i| (i, i + 1)).collect();
        let (flat, flat_from) = flattened_map(&clean, &identity);
        let (_, masks) = hidden_lines(text, rules)?;
        // Project before flattening: a normalized space may bridge a removed private block,
        // but the removed bytes must not turn that space into a mask.
        let mut runs = projected(&from, &merged_runs(masks));
        let (blocks, masks) = hidden_lines(&flat, rules)?;
        runs.extend(
            blocks
                .into_iter()
                .chain(masks)
                .map(|(s, e)| origin(&flat_from[s..e])),
        );
        let mut runs = merged_runs(runs);
        if runs.len() > MAX_FINDINGS {
            return None;
        }
        let (mut masked, mut masked_from) = mask_map(&clean, &runs);
        for pass in 0..=MAX_PASSES {
            let (blocks, masks) = hidden_lines(&masked, rules)?;
            let mut more: Vec<_> = blocks
                .into_iter()
                .chain(masks)
                .map(|(s, e)| origin(&masked_from[s..e]))
                .collect();
            let (flat, flat_from) = flattened_map(&masked, &masked_from);
            let (blocks, masks) = hidden_lines(&flat, rules)?;
            more.extend(
                blocks
                    .into_iter()
                    .chain(masks)
                    .map(|(s, e)| origin(&flat_from[s..e])),
            );
            let next = merged_runs(runs.iter().copied().chain(more).collect());
            if next == runs {
                break;
            }
            if pass == MAX_PASSES || next.len() > MAX_FINDINGS {
                return None;
            }
            runs = next;
            (masked, masked_from) = mask_map(&clean, &runs);
        }
        let (masked, masked_from) = flattened_map(&masked, &masked_from);
        let shown = clip(&masked, n);
        let clipped = shown != masked;
        let prefix = if clipped {
            shown.strip_suffix('…')?
        } else {
            shown.as_str()
        };
        if !masked.starts_with(prefix) {
            return None;
        }
        let (mut text, mut hidden, mut shadow_from, mut at) =
            (String::new(), Vec::new(), Vec::new(), 0);
        while at < prefix.len() {
            let run = masked_from[at];
            if prefix[at..].starts_with(MASK)
                && masked_from[at..at + MASK.len()].iter().all(|&r| r == run)
            {
                let (original, origins) =
                    flattened_map(&clean[run.0..run.1], &identity[run.0..run.1]);
                let start = text.len();
                if original.is_empty() {
                    text.push(' ');
                    shadow_from.push(run);
                } else {
                    text.push_str(&original);
                    shadow_from.extend(origins);
                }
                hidden.push((start, text.len()));
                at += MASK.len();
            } else {
                let end = at + prefix[at..].chars().next()?.len_utf8();
                text.push_str(&prefix[at..end]);
                shadow_from.extend_from_slice(&masked_from[at..end]);
                at = end;
            }
        }
        let ellipsis = (text.len(), text.len() + '…'.len_utf8());
        if clipped {
            text.push('…');
        }
        let base = clip(&flat, n);
        let base_clipped = base != flat;
        let base_prefix = if base_clipped {
            base.strip_suffix('…')?
        } else {
            base.as_str()
        };
        if !flat.starts_with(base_prefix) {
            return None;
        }
        let mut base_from = inverse(&flat_from[..base_prefix.len()], &shadow_from);
        if base_clipped {
            base_from.extend(std::iter::repeat_n(
                if clipped { ellipsis } else { (0, 0) },
                '…'.len_utf8(),
            ));
        }
        let end = masked_from[..prefix.len()].last().map_or(0, |&(_, e)| e);
        let (mut selected, selected_from) = flattened_map(&clean[..end], &identity[..end]);
        let mut selected_from = inverse(&selected_from, &shadow_from);
        if clipped {
            selected.push('…');
            selected_from.extend(std::iter::repeat_n(ellipsis, '…'.len_utf8()));
        }
        Some(Mapped {
            text,
            hidden: Some(hidden),
            views: [(base, base_from), (selected, selected_from)],
        })
    };
    mapped().unwrap_or(Mapped {
        text: String::new(),
        hidden: None,
        views: std::array::from_fn(|_| Default::default()),
    })
}

/// A normalized view's source bytes back to the selected shadow; absent bytes have no origin.
fn inverse(from: &[(usize, usize)], shadow: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let (mut out, mut at) = (Vec::new(), 0);
    for &(s, e) in from {
        while at < shadow.len() && shadow[at].1 <= s {
            at += 1;
        }
        let end = at + shadow[at..].iter().take_while(|&&(a, _)| a < e).count();
        out.push(if at < end { (at, end) } else { (0, 0) });
    }
    out
}

fn flattened_map(text: &str, from: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let (mut out, mut origins, mut end) = (String::new(), Vec::new(), 0);
    for word in text.split_whitespace() {
        let start = word.as_ptr() as usize - text.as_ptr() as usize;
        if !out.is_empty() {
            out.push(' ');
            origins.push(origin(&from[end..start]));
        }
        end = start + word.len();
        out.push_str(word);
        origins.extend_from_slice(&from[start..end]);
    }
    (out, origins)
}

fn origin(from: &[(usize, usize)]) -> (usize, usize) {
    from.iter()
        .filter(|&&(s, e)| s < e)
        .fold((usize::MAX, 0), |(a, b), &(s, e)| (a.min(s), b.max(e)))
}

/// Hidden source ranges projected onto the retained bytes of a stripped or flattened view.
fn projected(from: &[(usize, usize)], runs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let (mut out, mut run, mut start) = (Vec::new(), 0, None);
    for (i, &(s, e)) in from.iter().enumerate() {
        while run < runs.len() && runs[run].1 <= s {
            run += 1;
        }
        if runs.get(run).is_some_and(|&(a, b)| a < e && s < b) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i));
        }
    }
    if let Some(s) = start {
        out.push((s, from.len()));
    }
    out
}

/// Replace runs once, preserving both ordinary bytes and the source behind each mask.
fn mask_map(text: &str, runs: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let (mut from, mut at) = (Vec::new(), 0);
    for &(s, e) in runs {
        from.extend((at..s).map(|i| (i, i + 1)));
        from.extend(std::iter::repeat_n((s, e), MASK.len()));
        at = e;
    }
    from.extend((at..text.len()).map(|i| (i, i + 1)));
    (masked_part(text, 0..text.len(), runs), from)
}

/// `outbound_lines` of a text joined from `parts` (byte ranges of it: an imported document's
/// title and body under its kind), what `outbound_lines` hides in a part alone hidden too, as
/// search gates each part alone. Each view's hiding (the whole's and each part's, line pass
/// included) is mapped back to the untouched text before anything is masked, so the result shows
/// no byte that `outbound_lines` of the whole or of a part hides, and no view's mask takes away the
/// context another view's rule needs (Codex on 6c19081 and 923bdd1); `outbound_lines` over the
/// result can only add masks.
pub fn outbound_joined(text: &str, parts: &[std::ops::Range<usize>]) -> String {
    match egress() {
        Some(rules) => joined_with(text, parts, &rules),
        None => MASK.to_string(),
    }
}

pub(crate) fn joined_with(text: &str, parts: &[std::ops::Range<usize>], rules: &Rules) -> String {
    let views = std::iter::once(0..text.len()).chain(parts.iter().cloned());
    let Some(runs) = hidden_views(text, views, rules) else {
        return MASK.to_string();
    };
    lines_with(&masked_part(text, 0..text.len(), &runs), rules)
}

/// What `outbound_lines` hides in each of `views` (byte ranges of `text`) gated alone, found on
/// the untouched text and given in its offsets, sorted and merged: no view's mask takes away the
/// context another view's rule needs. `None` when the gate masks a view whole.
fn hidden_views(
    text: &str,
    views: impl IntoIterator<Item = std::ops::Range<usize>>,
    rules: &Rules,
) -> Option<Vec<(usize, usize)>> {
    let mut runs = Vec::new();
    for view in views {
        let (blocks, masks) = hidden_lines(&text[view.clone()], rules)?;
        let found = blocks.into_iter().chain(masks);
        runs.extend(found.map(|(s, e)| (view.start + s, view.start + e)));
    }
    Some(merged_runs(runs))
}

/// `runs` sorted, overlapping or touching ones joined (`merged`).
pub(crate) fn merged_runs(mut runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    runs.sort_unstable();
    merged(&runs.into_iter().map(|(s, e)| (s, e, 0)).collect::<Vec<_>>())
}

/// `outbound_lines` of `text[range]` with what `outbound_lines` of the whole `text` hides there
/// hidden too, also where the range splits it: a claim's quote shown with its record's words
/// around it, so a rule that needs them (a code after a name) hides the quote as it hides the
/// record, and a rule on the quote alone still sees the quote untouched (Codex's security review
/// of Task 7). `range` is on character boundaries.
pub fn outbound_quote(text: &str, range: std::ops::Range<usize>) -> String {
    match egress() {
        Some(rules) => quote_with(text, range, &rules),
        None => MASK.to_string(),
    }
}

fn quote_with(text: &str, range: std::ops::Range<usize>, rules: &Rules) -> String {
    match hidden_views(text, [0..text.len(), range.clone()], rules) {
        Some(runs) => lines_with(&masked_part(text, range, &runs), rules),
        None => MASK.to_string(),
    }
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
mod tests;

#[cfg(test)]
mod fixture_scan;
