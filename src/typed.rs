//! Which prompts the owner typed (docs/my-work-model.md O1 to O3, parity row X21): what My Work's
//! goals, rules and grouping read as the owner's words. A record says where its prompt came from
//! (`agent_sent`, a Claude Code prompt's `entrypoint`); two text tests, claude-mem's in every
//! script, find what an agent wrote first (claude-mem 15.2 `owner-prompt-filter.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// The Claude Code entry points of a session a person types in: the terminal, the desktop app
/// and the VS Code extension. `claude -p` is `sdk-cli`, the Agent SDK `sdk-ts` or `sdk-py`.
pub(crate) const TYPED_ENTRYPOINTS: &[&str] = &["cli", "claude-desktop", "claude-vscode"];

/// A Claude Code entry point as a record keeps it: a word of at most 32 lowercase letters, digits,
/// `-` and `_`, as Claude Code's own are (`remote_trigger`, `claude-code-github-action`), or none.
pub(crate) fn entrypoint(value: &Value) -> Option<&str> {
    value.as_str().filter(|e| {
        !e.is_empty()
            && e.len() <= 32
            && (e.bytes())
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    })
}

/// Whether a record of `kind` from `agent`, written by `source`, with `body`, is a prompt the owner
/// typed, as far as the record says (O1, O2): its text kept, no other program's mark, and for
/// Claude Code an entry point a person types in. A Claude Code prompt recorded before entry points
/// were kept has none, and counts when a hook wrote it (O5): a session only the transcript import
/// found ran no hooks, as the Agent SDK's do not. The session's exclusion and forget are its
/// reader's.
pub(crate) fn typed(kind: &str, agent: &str, source: &str, body: &Value) -> bool {
    kind == "prompt"
        && body.get("prompt").is_some()
        && body["agent_sent"] != true
        && (agent != "claude"
            || match body.get("entrypoint") {
                Some(e) => entrypoint(e).is_some_and(|e| TYPED_ENTRYPOINTS.contains(&e)),
                None => source != "transcript",
            })
}

/// A prompt shorter than this, normalized, is not looked for in agent text.
const CHECKABLE: usize = 160;
/// The opening that is looked for: the owner often types a line and pastes an agent's report.
const FINGERPRINT: usize = 120;
/// How far back agent text is read for a prompt (O3).
pub(crate) const LOOKBACK_MS: i64 = 9 * 24 * 60 * 60 * 1000;

/// `text` as the tests compare it: NFKC, lower case, each run of anything but the letters and
/// digits of any script one space. claude-mem keeps a to z and 0 to 9 only, so a Japanese prompt
/// is never tested there.
fn normalized(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut gap = false;
    for c in text.nfkc().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// Whether `text` opens in lower case: a letter that has an upper case, written in its lower.
fn opens_in_lowercase(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_lowercase() && c.to_uppercase().ne(std::iter::once(c)))
}

/// The opening of `prompt` to look for in agent text, or none when it is not tested that way: too
/// short, or opening in lower case, as agents do not open a prompt that way and the owner does.
pub(crate) fn fingerprint(prompt: &str) -> Option<String> {
    let n = normalized(prompt);
    if n.len() < CHECKABLE || opens_in_lowercase(prompt) {
        return None;
    }
    Some(n[..FINGERPRINT].iter().collect())
}

/// Whether `prompt` opens by giving the agent a role and runs past 400 characters: a launcher
/// that hands a prompt over through a file leaves no copy in any transcript.
pub(crate) fn assigns_a_role(prompt: &str) -> bool {
    let role = prompt.strip_prefix("You are ").is_some_and(|rest| {
        ["the ", "a ", "an ", "one ", "on ", "in "]
            .iter()
            .any(|w| rest.starts_with(w))
    });
    (role || prompt.starts_with("Task: ") || prompt.starts_with("Branch: "))
        && prompt.chars().count() > 400
}

/// Whether `prompt` is a review prompt Codex builds itself (`/review`, `codex review --base` or
/// `--commit`, 0.166.0's five templates): the whole text as a template renders it, its branch,
/// commit and title filled in, never the owner's words. Text given to `codex review` is sent as
/// it was typed and stays the owner's, even where it opens and closes as a template does.
pub(crate) fn codex_review_preset(prompt: &str) -> bool {
    const FINDINGS: &str = " Provide prioritized, actionable findings.";
    let p = prompt.trim();
    if p == "Review the current code changes (staged, unstaged, and untracked files) and provide \
             prioritized findings."
    {
        return true;
    }
    let word = |s: &str| !s.is_empty() && !s.contains(char::is_whitespace);
    if let Some(rest) = p.strip_prefix("Review the code changes introduced by commit ") {
        // `{sha}.` or `{sha} ("{title}").`, then the closing sentence.
        let Some(commit) = rest
            .strip_suffix(FINDINGS)
            .and_then(|r| r.strip_suffix('.'))
        else {
            return false;
        };
        // A title is Codex's `--title` as given, line breaks too.
        return word(commit)
            || commit
                .split_once(" (\"")
                .is_some_and(|(sha, title)| word(sha) && title.ends_with("\")"));
    }
    let Some(rest) = p.strip_prefix("Review the code changes against the base branch '") else {
        return false;
    };
    let Some((branch, _)) = rest.split_once("'. ") else {
        return false;
    };
    let head = format!("Review the code changes against the base branch '{branch}'.");
    let upstream = format!(
        "{head} Start by finding the merge diff between the current branch and {branch}'s \
         upstream e.g. (`git merge-base HEAD \"$(git rev-parse --abbrev-ref \
         \"{branch}@{{upstream}}\")\"`), then run `git diff` against that SHA to see what changes \
         we would merge into the {branch} branch.{FINDINGS}"
    );
    let merge_base = |sha: &str| {
        format!(
            "{head} The merge base commit for this comparison is {sha}. Run `git diff {sha}` to \
             inspect the changes relative to {branch}.{FINDINGS}"
        )
    };
    p == upstream
        || p.strip_prefix(&format!(
            "{head} The merge base commit for this comparison is "
        ))
        .and_then(|r| r.split_once(". Run `git diff "))
        .is_some_and(|(sha, _)| p == merge_base(sha))
}

/// The agent text a record of `kind` holds for the text tests (O3): a reply's text, or a tool's
/// input with JSON's escapes of line breaks and tabs undone, as claude-mem's readers undo a line
/// break's, so a prompt an agent wrote into a command or a file is found across its lines.
pub(crate) fn agent_text(kind: &str, body: &Value) -> Option<String> {
    match kind {
        "reply" => body["assistant"].as_str().map(str::to_owned),
        "tool" => body["input"].as_str().map(|input| {
            input
                .replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\t", "\t")
        }),
        _ => None,
    }
}

/// When the agent text a record of `kind` with `body` holds was written, for the text tests (O3,
/// C3): a tool's input when its call began (#692), since a call that starts another agent and
/// waits for it records that agent's ask before its own end; else, and for a call that ran over
/// `LONGEST_CALL_MS` (#709), the record's time `ts`.
pub(crate) fn written_at(kind: &str, body: &Value, ts: i64) -> i64 {
    match kind {
        "tool" => (body["started"].as_i64())
            .filter(|started| ts - started <= LONGEST_CALL_MS)
            .unwrap_or(ts),
        _ => ts,
    }
}

/// How long after the last ask a tool's record is read for a call that began before it (#692),
/// and how long a call may run to be compared at its start (#709).
// shortcut: a call that ran longer than a day is compared at its end; widen it if delegations ever
// wait that long.
pub(crate) const LONGEST_CALL_MS: i64 = 24 * 60 * 60 * 1000;

const BASE: u64 = 1_000_003;

fn step(hash: u64, c: char) -> u64 {
    hash.wrapping_mul(BASE).wrapping_add(c as u64)
}

/// The fingerprints of some prompts, each with its prompts' times, found in agent text written in
/// the `LOOKBACK_MS` before a prompt: one streaming pass, a rolling hash over every position,
/// verified by comparison (claude-mem's `AgentTextIndex`).
/// A conversation, by its agent and session.
type Conversation = (String, String);

/// The conversations whose tool input held a prompt's opening, each with the last such input's time
/// and record seq (#692).
type Inputs = HashMap<Conversation, (i64, i64)>;

pub(crate) struct AgentText {
    by_hash: HashMap<u64, Vec<Vec<char>>>,
    prompts: HashMap<Vec<char>, Vec<i64>>,
    found: HashSet<(Vec<char>, i64)>,
    /// C3: for each prompt found, the conversations whose tool input held its opening, each with
    /// the last such input's time and record seq.
    senders: HashMap<(Vec<char>, i64), Inputs>,
    high: u64,
}

impl AgentText {
    /// For prompts `(time, text)`: the ones a text test can flag by their opening.
    pub(crate) fn new<'a>(prompts: impl IntoIterator<Item = (i64, &'a str)>) -> Self {
        let mut index = AgentText {
            by_hash: HashMap::new(),
            prompts: HashMap::new(),
            found: HashSet::new(),
            senders: HashMap::new(),
            high: (1..FINGERPRINT).fold(1u64, |h, _| h.wrapping_mul(BASE)),
        };
        for (ts, text) in prompts {
            let Some(f) = fingerprint(text) else {
                continue;
            };
            let f: Vec<char> = f.chars().collect();
            let times = index.prompts.entry(f.clone()).or_default();
            if times.is_empty() {
                let hash = f.iter().fold(0, |h, &c| step(h, c));
                index.by_hash.entry(hash).or_default().push(f);
            }
            times.push(ts);
        }
        index
    }

    /// Whether no prompt can be flagged by its opening, so no agent text need be read.
    pub(crate) fn is_empty(&self) -> bool {
        self.prompts.is_empty()
    }

    /// One piece of agent text (a reply, a tool's input) written at `ts`.
    pub(crate) fn add(&mut self, ts: i64, text: &str) {
        self.scan(ts, text, None);
    }

    /// A tool's input written at `ts` in conversation `from` (agent, session), record `seq`: it
    /// may have sent a prompt (C3).
    pub(crate) fn add_from(&mut self, ts: i64, text: &str, from: &Conversation, seq: i64) {
        self.scan(ts, text, Some((from, seq)));
    }

    fn scan(&mut self, ts: i64, text: &str, from: Option<(&Conversation, i64)>) {
        if self.is_empty() || text.chars().count() < FINGERPRINT {
            return;
        }
        let chunk = normalized(text);
        if chunk.len() < FINGERPRINT {
            return;
        }
        let mut hash = chunk[..FINGERPRINT].iter().fold(0, |h, &c| step(h, c));
        for start in 0.. {
            if let Some(bucket) = self.by_hash.get(&hash) {
                for f in bucket {
                    if chunk[start..start + FINGERPRINT] != f[..] {
                        continue;
                    }
                    for &p in &self.prompts[f] {
                        if ts < p && p - ts <= LOOKBACK_MS {
                            self.found.insert((f.clone(), p));
                            if let Some((from, seq)) = from {
                                let at = (self.senders.entry((f.clone(), p)).or_default())
                                    .entry(from.clone())
                                    .or_insert((ts, seq));
                                if ts > at.0 {
                                    *at = (ts, seq);
                                }
                            }
                        }
                    }
                }
            }
            let next = start + FINGERPRINT;
            if next >= chunk.len() {
                break;
            }
            let outgoing = (chunk[start] as u64).wrapping_mul(self.high);
            hash = step(hash.wrapping_sub(outgoing), chunk[next]);
        }
    }

    /// C3: the conversation, other than `own`, whose tool input written last in the `LOOKBACK_MS`
    /// before the prompt `text` typed at `ts` held its opening, with that input's record seq.
    pub(crate) fn sender(
        &self,
        ts: i64,
        text: &str,
        own: &Conversation,
    ) -> Option<(&Conversation, i64)> {
        let f: Vec<char> = fingerprint(text)?.chars().collect();
        (self.senders.get(&(f, ts))?.iter())
            .filter(|(from, _)| *from != own)
            .max_by_key(|(from, at)| (at.0, *from))
            .map(|(from, at)| (from, at.1))
    }

    /// Whether the prompt `text`, typed at `ts`, is an agent's (O3): its opening was in agent text
    /// written in the `LOOKBACK_MS` before it, it assigns a role, or it is Codex's review template.
    pub(crate) fn agent_written(&self, ts: i64, text: &str) -> bool {
        let opening = fingerprint(text).map(|f| (f.chars().collect::<Vec<_>>(), ts));
        opening.is_some_and(|key| self.found.contains(&key))
            || assigns_a_role(text)
            || codex_review_preset(text)
    }
}

#[cfg(test)]
mod tests;
