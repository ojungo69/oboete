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

/// A Claude Code entry point as a record keeps it: a short lowercase word, or none.
pub(crate) fn entrypoint(value: &Value) -> Option<&str> {
    value.as_str().filter(|e| {
        !e.is_empty()
            && e.len() <= 32
            && e.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Whether a record of `kind` from `agent`, with `body`, is a prompt the owner typed, as far as
/// the record says (O1, O2): its text kept, no other program's mark, and for Claude Code an entry
/// point a person types in. A Claude Code prompt recorded before entry points were kept has none
/// and counts (O5). The session's exclusion and forget are its reader's.
pub(crate) fn typed(kind: &str, agent: &str, body: &Value) -> bool {
    kind == "prompt"
        && body.get("prompt").is_some()
        && body["agent_sent"] != true
        && (agent != "claude"
            || body
                .get("entrypoint")
                .is_none_or(|e| entrypoint(e).is_some_and(|e| TYPED_ENTRYPOINTS.contains(&e))))
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

const BASE: u64 = 1_000_003;

fn step(hash: u64, c: char) -> u64 {
    hash.wrapping_mul(BASE).wrapping_add(c as u64)
}

/// The fingerprints of some prompts, each with its prompts' times, found in agent text written in
/// the `LOOKBACK_MS` before a prompt: one streaming pass, a rolling hash over every position,
/// verified by comparison (claude-mem's `AgentTextIndex`).
pub(crate) struct AgentText {
    by_hash: HashMap<u64, Vec<Vec<char>>>,
    prompts: HashMap<Vec<char>, Vec<i64>>,
    found: HashSet<(Vec<char>, i64)>,
    high: u64,
}

impl AgentText {
    /// For prompts `(time, text)`: the ones a text test can flag by their opening.
    pub(crate) fn new<'a>(prompts: impl IntoIterator<Item = (i64, &'a str)>) -> Self {
        let mut index = AgentText {
            by_hash: HashMap::new(),
            prompts: HashMap::new(),
            found: HashSet::new(),
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

    /// Whether the prompt `text`, typed at `ts`, is an agent's (O3): its opening was in agent text
    /// written in the `LOOKBACK_MS` before it, or it assigns a role.
    pub(crate) fn agent_written(&self, ts: i64, text: &str) -> bool {
        let opening = fingerprint(text).map(|f| (f.chars().collect::<Vec<_>>(), ts));
        opening.is_some_and(|key| self.found.contains(&key)) || assigns_a_role(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn long(head: &str) -> String {
        format!(
            "{head} {}",
            "and the parser keeps its tables between runs ".repeat(6)
        )
    }

    /// O2: a Claude Code prompt counts by the entry point its hook was given; Codex's mark and an
    /// envelope or an omitted prompt never do.
    #[test]
    fn a_prompt_is_typed_by_its_entry_point_and_marks() {
        let prompt = |extra: Value| {
            let mut body = json!({"prompt": "Fix the parser."});
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            body
        };
        for e in ["cli", "claude-desktop", "claude-vscode"] {
            assert!(
                typed("prompt", "claude", &prompt(json!({"entrypoint": e}))),
                "{e}"
            );
        }
        for e in ["sdk-cli", "sdk-ts", "sdk-py", "CLI", ""] {
            assert!(
                !typed("prompt", "claude", &prompt(json!({"entrypoint": e}))),
                "{e}"
            );
        }
        assert!(
            typed("prompt", "claude", &prompt(json!({}))),
            "recorded before marks"
        );
        assert!(!typed(
            "prompt",
            "codex",
            &prompt(json!({"agent_sent": true}))
        ));
        assert!(typed("prompt", "grok", &prompt(json!({}))));
        assert!(!typed(
            "envelope",
            "claude",
            &prompt(json!({"entrypoint": "cli"}))
        ));
        assert!(!typed(
            "prompt",
            "claude",
            &json!({"omitted": true, "entrypoint": "cli"})
        ));
    }

    /// O3: an opening an agent wrote in the nine days before the prompt flags it, in English and
    /// in Japanese; after it, or longer before, does not.
    #[test]
    fn a_prompt_whose_opening_an_agent_wrote_earlier_is_not_typed() {
        let english = long("Review the parser change");
        let japanese = format!(
            "{}{}",
            "パーサーの変更を確認して、テーブルの保存と読み込みが正しく動くかを調べてください。",
            "読み込みの時間も測って結果を表にまとめてください。".repeat(5)
        );
        assert!(fingerprint(&japanese).is_some(), "Japanese is tested too");
        let t = 100 * DAY;
        let mut index = AgentText::new([(t, english.as_str()), (t, japanese.as_str())]);
        index.add(t - DAY, &format!("prompt.txt: {english}"));
        index.add(t - 2 * DAY, &format!("「{japanese}」"));
        assert!(index.agent_written(t, &english));
        assert!(index.agent_written(t, &japanese));
        let mut late = AgentText::new([(t, english.as_str())]);
        late.add(t + 1, &english);
        late.add(t - 10 * DAY, &english);
        assert!(
            !late.agent_written(t, &english),
            "after it, or 10 days before"
        );
    }

    /// O3: a prompt that opens in lower case or is short is never looked for.
    #[test]
    fn a_prompt_that_opens_in_lowercase_or_is_short_is_not_fingerprinted() {
        assert_eq!(fingerprint(&long("review the parser change")), None);
        assert_eq!(fingerprint("Review the parser change."), None);
        let f = fingerprint(&long("Review the parser change")).unwrap();
        assert_eq!(f.chars().count(), FINGERPRINT);
        assert!(f.starts_with("review the parser change and"));
    }

    /// O3: a role-assignment opening over 400 characters is an agent's, with no agent text.
    #[test]
    fn a_role_assignment_opening_over_400_characters_is_an_agents() {
        let index = AgentText::new([]);
        let role = format!(
            "You are the reviewer. {}",
            "Check every change. ".repeat(25)
        );
        assert!(index.agent_written(0, &role));
        assert!(index.agent_written(0, &format!("Task: {}", "x".repeat(400))));
        assert!(!index.agent_written(0, "You are the reviewer. Check it."));
        assert!(!index.agent_written(0, &format!("You were {}", "x".repeat(400))));
    }
}
