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
/// waits for it records that agent's ask before its own end; else the record's time `ts`.
pub(crate) fn written_at(kind: &str, body: &Value, ts: i64) -> i64 {
    match kind {
        "tool" => body["started"].as_i64().unwrap_or(ts),
        _ => ts,
    }
}

/// How long after the last ask a tool's record is read for a call that began before it (#692).
// shortcut: a call that ran longer than a day is not read for its start; widen it if delegations
// ever wait that long.
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

    fn prompt(extra: Value) -> Value {
        let mut body = json!({"prompt": "Fix the parser."});
        (body.as_object_mut().unwrap()).extend(extra.as_object().unwrap().clone());
        body
    }

    /// C3: a prompt's sender is the conversation, other than its own, whose tool input held its
    /// opening last before it in the lookback; a reply names none.
    #[test]
    fn a_prompts_sender_is_the_last_other_tool_input_that_held_its_opening() {
        let text = long("Port the lexer to the new parser");
        let key = |s: &str| ("claude".to_owned(), s.to_owned());
        let mut index = AgentText::new([(10 * DAY, text.as_str())]);
        index.add(10 * DAY - 4_000, &format!("I will send it: {text}"));
        assert_eq!(index.sender(10 * DAY, &text, &key("own")), None);
        assert!(index.agent_written(10 * DAY, &text));
        index.add_from(10 * DAY - 5_000, &format!("echo '{text}'"), &key("a"), 5);
        index.add_from(
            10 * DAY - 2_000,
            &format!("codex exec '{text}'"),
            &key("b"),
            7,
        );
        index.add_from(
            10 * DAY - 1_000,
            &format!("cat <<EOF\n{text}\nEOF"),
            &key("own"),
            9,
        );
        assert_eq!(
            index.sender(10 * DAY, &text, &key("own")),
            Some((&key("b"), 7))
        );
        // Past the lookback, or after the prompt, names none.
        let mut late = AgentText::new([(10 * DAY, text.as_str())]);
        late.add_from(10 * DAY + 1, &text, &key("a"), 1);
        late.add_from(10 * DAY - LOOKBACK_MS - 1, &text, &key("b"), 2);
        assert_eq!(late.sender(10 * DAY, &text, &key("own")), None);
    }

    /// O2: a Claude Code prompt counts by its entry point: the terminal, the desktop app and the
    /// VS Code extension; `claude -p`, the Agent SDK and Claude Code's other programs never (O5:
    /// one recorded before entry points were kept counts when a hook wrote it).
    #[test]
    fn a_headless_claude_prompt_is_not_typed() {
        for source in ["hook", "transcript"] {
            for e in ["cli", "claude-desktop", "claude-vscode"] {
                let body = prompt(json!({"entrypoint": e}));
                assert!(typed("prompt", "claude", source, &body), "{e}");
            }
            for e in [
                "sdk-cli",
                "sdk-ts",
                "sdk-py",
                "mcp",
                "remote_trigger",
                "claude-code-github-action",
                "CLI",
                "",
            ] {
                let body = prompt(json!({"entrypoint": e}));
                assert!(!typed("prompt", "claude", source, &body), "{e}");
            }
        }
        assert_eq!(entrypoint(&json!("remote_trigger")), Some("remote_trigger"));
        for source in ["hook", "oboete-v1"] {
            assert!(
                typed("prompt", "claude", source, &prompt(json!({}))),
                "{source}"
            );
        }
        assert!(
            !typed("prompt", "claude", "transcript", &prompt(json!({}))),
            "a session only the transcript import found ran no hooks"
        );
    }

    /// O1: an envelope, a prompt kept private whole and an omitted one are no typed prompt.
    #[test]
    fn an_envelope_a_private_prompt_and_an_omitted_one_are_not_typed() {
        assert!(!typed(
            "envelope",
            "claude",
            "hook",
            &prompt(json!({"entrypoint": "cli"}))
        ));
        assert!(!typed(
            "prompt",
            "claude",
            "hook",
            &json!({"omitted": true, "entrypoint": "cli"})
        ));
        let private =
            json!({"session_id": "s", "cwd": "/w", "prompt": "<private>a secret</private>"});
        let captured = crate::capture::events(
            "claude",
            "UserPromptSubmit",
            &private,
            0,
            &Default::default(),
        );
        assert!(captured.is_empty(), "nothing is recorded of it");
    }

    /// O2: a Codex prompt Claude Code's Codex plugin sent is marked another program's, as an exec
    /// run's is.
    #[test]
    fn a_codex_prompt_the_codex_plugin_sent_is_not_typed() {
        let plugin = json!({"source": "vscode", "originator": "Claude Code"});
        assert!(crate::transcript::codex_agent_sent(&plugin));
        assert!(!typed(
            "prompt",
            "codex",
            "hook",
            &prompt(json!({"agent_sent": true}))
        ));
        assert!(typed("prompt", "codex", "hook", &prompt(json!({}))));
    }

    /// O5: a Grok prompt carries no mark, so it counts unless a text test flags it.
    #[test]
    fn a_grok_prompt_counts_unless_a_text_test_flags_it() {
        let text = long("Review the parser change");
        assert!(typed("prompt", "grok", "hook", &json!({"prompt": text})));
        let mut index = AgentText::new([(10 * DAY, text.as_str())]);
        assert!(!index.agent_written(10 * DAY, &text));
        index.add(
            9 * DAY,
            &format!("grok --prompt-file p.txt <<EOF\n{text}\nEOF"),
        );
        assert!(index.agent_written(10 * DAY, &text));
    }

    /// O3: an opening an agent wrote in the nine days before the prompt flags it, in English and
    /// in Japanese.
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
    }

    /// O3: agent text written after the prompt, or more than nine days before it, does not flag it.
    #[test]
    fn agent_text_after_the_prompt_or_from_10_days_before_does_not_flag_it() {
        let text = long("Review the parser change");
        let t = 100 * DAY;
        let mut index = AgentText::new([(t, text.as_str())]);
        index.add(t + 1, &text);
        index.add(t - 10 * DAY, &text);
        assert!(!index.agent_written(t, &text));
        index.add(t - 9 * DAY, &text);
        assert!(index.agent_written(t, &text));
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

    /// O3: a prompt an agent wrote into a tool call, a command or a file, is found across its
    /// lines; a reply is read as written; other records hold no agent text.
    #[test]
    fn a_prompt_written_into_a_tool_call_is_found_across_its_lines() {
        let text = format!("Review the parser change.\nKeep {}", long("its tables"));
        let input = json!({"command": format!("cat > p.txt <<'EOF'\n{text}\nEOF")}).to_string();
        let t = 100 * DAY;
        let mut index = AgentText::new([(t, text.as_str())]);
        index.add(t - DAY, &input);
        assert!(
            !index.agent_written(t, &text),
            "escaped, the line break reads as an n"
        );
        index.add(
            t - DAY,
            &agent_text("tool", &json!({"input": input})).unwrap(),
        );
        assert!(index.agent_written(t, &text));
        let reply = agent_text("reply", &json!({"assistant": "Done."}));
        assert_eq!(reply.as_deref(), Some("Done."));
        assert_eq!(agent_text("prompt", &json!({"prompt": text})), None);
    }

    /// Every event of `raw` from `from` on, device by device, in pages.
    fn each_event(raw: &crate::raw::Raw, from: i64, mut each: impl FnMut(&crate::raw::Event)) {
        for device in raw.devices().unwrap() {
            let mut seq = 0;
            loop {
                let page = raw.after(&device, seq, 2_000).unwrap();
                let Some(last) = page.last() else {
                    break;
                };
                seq = last.seq;
                for r in &page {
                    if let crate::raw::Item::Event(e) = &r.item
                        && e.ts >= from
                    {
                        each(e);
                    }
                }
            }
        }
    }

    /// Slice 1's measurement (docs/my-work-model.md): on a copy of a store, never the live one,
    /// how many of the last 31 days' prompts each test flags, by agent, and how long the 40 days
    /// of agent text before them take to stream. By hand: `OBOETE_MEASURE_HOME=<copy> cargo test
    /// --release --bin oboete -- --ignored --nocapture measure_the_marks`.
    #[test]
    #[ignore]
    fn measure_the_marks_on_a_copy_of_a_store() {
        let home = std::env::var("OBOETE_MEASURE_HOME").expect("a copy of a store");
        let raw = crate::raw::open(std::path::Path::new(&home)).unwrap();
        let now = crate::db::now_ms();
        let started = std::time::Instant::now();
        let mut prompts = Vec::new();
        each_event(&raw, now - 31 * DAY, |e| {
            if e.kind == "prompt" {
                let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
                let text = crate::hook::compact(&body["prompt"]);
                prompts.push((
                    e.agent.clone(),
                    e.ts,
                    typed(&e.kind, &e.agent, &e.source, &body),
                    text,
                ));
            }
        });
        let read = started.elapsed();
        let marked = prompts.iter().filter(|p| p.2).map(|p| (p.1, p.3.as_str()));
        let mut index = AgentText::new(marked);
        let started = std::time::Instant::now();
        let (mut testing, mut pieces, mut chars) = (std::time::Duration::ZERO, 0usize, 0usize);
        each_event(&raw, now - 40 * DAY, |e| {
            let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
            if let Some(text) = agent_text(&e.kind, &body) {
                let at = std::time::Instant::now();
                index.add(e.ts, &text);
                testing += at.elapsed();
                pieces += 1;
                chars += text.chars().count();
            }
        });
        let streamed = started.elapsed();
        let mut by_agent = std::collections::BTreeMap::<&str, [usize; 7]>::new();
        for (agent, ts, typed, text) in &prompts {
            let row = by_agent.entry(agent).or_default();
            row[0] += 1;
            if !typed {
                row[1] += 1;
                continue;
            }
            row[2] += 1;
            let opening = fingerprint(text);
            row[3] += usize::from(opening.is_some());
            let found = opening.is_some_and(|f| index.found.contains(&(f.chars().collect(), *ts)));
            row[4] += usize::from(found);
            row[5] += usize::from(assigns_a_role(text));
            row[6] += usize::from(!index.agent_written(*ts, text));
        }
        println!(
            "31 days of prompts by agent: all, counted out by the marks, typed by the marks, \
             checkable, opening in agent text, role assignment, typed after the tests"
        );
        for (agent, row) in &by_agent {
            println!("{agent}: {row:?}");
        }
        let peak = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let peak = peak.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("");
        println!(
            "prompts read in {read:?}; {pieces} pieces of agent text ({chars} characters) of 40 \
             days streamed in {streamed:?}, {testing:?} of it in the tests; {peak}"
        );
    }
}
