//! Code gates on a curator's drafts (spec 3.2, 3.3, D4, MUST-M1, MUST-M2; milestone 3 Task 8):
//! the status, speaker, scope and supersedes a claim may have, read from the window's own lines
//! with no model call (spec 3.5: the none tier is these gates only). A draft that fails a gate is
//! kept lower or not at all, never raised. The claim op holds what passed, so a rebuild needs no
//! gate.

use crate::claims::{Claim, Evidence};
use crate::curate::{Draft, Line, Role, Window};
use crate::redact::Rules;
use std::collections::{HashMap, HashSet};

/// A reply to a proposal that holds one of these accepts it (spec 3.3). Latin ones match whole
/// words, the others anywhere in the turn.
// ponytail: seeded by hand; milestone 3 Task 8 part 2 measures them on the dev split (counts
// only) and Task 13 tunes them.
const ACCEPT: &[&str] = &[
    "はい",
    "うん",
    "ええ",
    "いいよ",
    "いいです",
    "いいね",
    "それでいい",
    "それでお願い",
    "それで進め",
    "それで大丈夫",
    "そうして",
    "そうしよう",
    "お願い",
    "進めて",
    "やって",
    "採用",
    "賛成",
    "了解",
    "承知",
    // Polite yeses; their ません is exempt from the negation check (`POLITE_YES`).
    "問題ありません",
    "構いません",
    "かまいません",
    "差し支えありません",
    "オッケー",
    "ok",
    "okay",
    "yes",
    "yeah",
    "yep",
    "sure",
    "agreed",
    "lgtm",
    "go ahead",
    "do it",
    "sounds good",
    "please do",
    "proceed",
];

/// A turn that holds one of these promotes nothing, even with an acceptance in it (spec 3.3).
const NEGATE: &[&str] = &[
    "ない",
    "やめ",
    "止め",
    "ダメ",
    "だめ",
    "違う",
    "ちがう",
    "待って",
    "まって",
    "不要",
    "却下",
    "反対",
    "いいえ",
    "ません",
    "かねます",
    "no",
    "not",
    "cannot",
    "never",
    "stop",
    "wait",
    "instead",
];

/// Polite phrases that end in a negative form and mean yes (or are only politeness): taken out
/// before `NEGATE` is looked for.
const POLITE_YES: &[&str] = &[
    "問題ありません",
    "構いません",
    "かまいません",
    "差し支えありません",
    "すみません",
];
/// MUST-M4: a run of this many characters shared with a tool line of the window is a paste.
// ponytail: Task 13 tunes it on the dev split with τ.
const PASTE_CHARS: usize = 40;

/// MUST-M4: the share of a proposal's character trigrams found in the window's tool lines at or
/// above which it is a paraphrase that keeps the words. `None` until Task 13 records a τ that
/// keeps the three canaries at 0% and `decided` recall at 0.80 or more on the dev split; until
/// then the provenance fallback holds (`after_a_tool`).
const TAU: Option<f64> = None;

/// What an acceptance says besides its acceptance words, which a bare one says nothing but.
const FILLER: &[&str] = &[
    // Before "ます", which would leave its し.
    "します",
    "です",
    "ます",
    "ください",
    "please",
    "thanks",
    "ありがとう",
];

/// What a passing run prints, as whole words, in a tool line that did not fail and reports no
/// failure (MUST-M1's `done`): a piped command exits 0 whatever it ran.
// ponytail: a few markers; a test runner's own summary line per tool when these miss real runs.
const PASSED: &str = r"\b(?:test result: ok|passed|build succeeded)\b";

/// Why a `supersedes` entry is dropped when it names neither a sibling of the draft's repository,
/// a candidate shown for it, nor what the draft's session carried in.
const OUTSIDE: &str = "supersedes only its repository's candidates and claims in the window, and \
                       what its session carried in";

/// A claim's body in characters (spec 6.5): a longer one is dropped, never cut.
const MAX_BODY_CHARS: usize = 1_000;

/// The most records a claim's status stands on that its op names (#167): a window can hold many
/// passing runs, and the op must stay within the op cap.
const MAX_SUPPORT: usize = 16;

/// What the gates let through, and what they did to the rest.
#[derive(Debug, Default, PartialEq)]
pub struct Gated {
    /// Each with its evidence: the quote's first, then a change's reason's when another line
    /// gives it (spec 3.3).
    pub kept: Vec<(Draft, Vec<Evidence>)>,
    /// Drafts not kept, and supersedes entries taken out of a kept one: the draft's id and why.
    pub dropped: Vec<(String, &'static str)>,
    /// Kept drafts changed on the way (a lower status, the speaker of the quote's line, repo
    /// scope): the id and why.
    pub lowered: Vec<(String, &'static str)>,
}

/// What the windows before this one say of its sessions, each named by its key.
#[derive(Clone, Copy, Default)]
pub struct Before<'a> {
    /// Those whose previous window ended on an untainted proposal: an acceptance that opens this
    /// window answers it (#144).
    pub ended: &'a [String],
    /// Those that were past a tool call in their turn when this window began: a reply that opens
    /// it is one after a tool call (MUST-M4).
    pub began: &'a [String],
}

/// The gates over a window's located drafts (each with its evidence and its line's index in
/// `w.lines`), with `shown` the candidates the curator was shown, each with its repository,
/// `carried` what each session carried in, with the session and the claim's repository, `before`
/// what the windows before this one say of its sessions, `offered` the sessions shown the options
/// of the reply their first line answers, with the options' labels (a pick that opens this window
/// names one, #252), and `rules` the egress gate's.
pub fn check(
    w: &Window,
    shown: &[(String, Claim)],
    carried: &[(String, Option<String>, Claim)],
    before: Before,
    offered: &[(String, Vec<String>)],
    drafts: Vec<(Draft, Evidence, usize)>,
    rules: &Rules,
) -> Gated {
    let mut g = Gated::default();
    let tools = tool_text(w);
    // Whether each kept draft stands on the user's own words, and its line's repository, for its
    // supersedes.
    let mut own = Vec::new();
    for (mut d, e, i) in drafts {
        let line = &w.lines[i];
        // As the claims consumer stores them: a non-strict curator's "Proposed" is gated as the
        // `unverified` it becomes, never let past a check for "proposed".
        let (kind, status) = crate::claims::normalize(&d.kind, &d.status);
        (d.kind, d.status) = (kind.into(), status.into());
        d.body = crate::redact::outbound_with(&d.body, rules);
        if d.body.chars().count() > MAX_BODY_CHARS {
            g.dropped.push((d.id, "over_cap"));
            continue;
        }
        let mut evidence = vec![e];
        if d.kind == "change" {
            if bare_file_count(&d.body) {
                g.dropped.push((d.id, "a bare file count"));
                continue;
            }
            // The record's words with their evidence, or none: a reason no line of the claim's
            // own session and repository gives is the curator's guess. Another line's reason
            // keeps that line as evidence, so removing it takes the reason's anchor too.
            let why = d.why.trim().to_owned();
            let own = |l: &&Line| l.key == line.key && l.repo == line.repo;
            let given = w
                .lines
                .iter()
                .filter(own)
                .find_map(|l| crate::curate::locate(w, &l.id, &why));
            d.why = match given {
                Some(at) => {
                    // Unless the quote's own range holds it: masking the reason alone must
                    // take its anchor too.
                    let q = &evidence[0];
                    let covered = (at.device.as_str(), at.seq) == (q.device.as_str(), q.seq)
                        && q.offset <= at.offset
                        && at.offset + at.length <= q.offset + q.length;
                    if !covered {
                        evidence.push(at);
                    }
                    crate::redact::outbound_with(&why, rules)
                }
                None => String::new(),
            };
            if d.why.is_empty() {
                d.why = "unknown".into();
            }
        } else {
            d.why.clear();
        }
        let mut lower = |d: &Draft, why| g.lowered.push((d.id.clone(), why));
        if d.scope != "repo" {
            d.scope = "repo".into();
            lower(&d, "global scope only through oboete pref add");
        }
        let speaker = speaker(line, &d.quote, &d.speaker);
        if d.speaker != speaker {
            d.speaker = speaker.into();
            lower(&d, "the speaker is the quote's line");
        }
        // On an `Answer` line the sentence is the answer's, not the serialized call's: a paste's
        // run is compared as the owner wrote it.
        let span = match line.role {
            Role::Answer => sentence_of(turn_said(line, &d.quote), &d.quote),
            _ => sentence_of(&line.text, &d.quote),
        };
        // A user turn that asks, or whose end is in a later window, promotes nothing, whatever
        // else the window holds (a passing run answers no question).
        // An answer's end is known on every part of a split call: it is read from the whole call.
        let asked = speaker == "user"
            && match line.role {
                Role::Answer => question(turn_said(line, &d.quote)),
                _ => question(&line.text) || continues(w, line),
            };
        // The user's own words carry the claim, unless they were pasted from a tool line; a bare
        // "yes" accepts only what it answers, and a pick ("1") only the option it names. Any part
        // of a user turn that says nothing but acceptance and filler is bare too ("please" from
        // "Yes, please.").
        let pick = (speaker == "user" && line.role == Role::User)
            .then(|| picks(&d.quote))
            .flatten();
        let is_bare = bare(&d.quote)
            || pick.is_some()
            || speaker == "user" && letters(&unsaid(turn_said(line, &d.quote))) == 0;
        // A bare quote's or a pick's few characters are in most tool text ("1", "ok" in "token"):
        // its decision comes from the reply it answers, not from its words.
        let paste = speaker == "user" && !is_bare && pasted(&norm(span), &tools);
        let own_words = speaker == "user" && !asked && !is_bare && !paste;
        let answers = speaker == "user"
            && is_bare
            && match &pick {
                Some(label) => picks_an_option(w, i, label, offered),
                None => answers_a_reply(w, i, &tools, before),
            };
        d.tainted = speaker == "assistant proposal" && tainted(w, i, span, &tools, before.began);
        let acceptance = (speaker == "assistant proposal" && !d.tainted)
            .then(|| accepted(w, i))
            .flatten();
        let accepts = acceptance.is_some();
        // The passing runs a done claim not in the user's own words stands on.
        let runs: Vec<&Line> = if d.status == "done" && !own_words {
            passing_runs(w, line).collect()
        } else {
            Vec::new()
        };
        let below = match d.status.as_str() {
            "decided" if paste => Some("a span pasted from a tool line is not the user's words"),
            "decided" if d.tainted => {
                Some("a proposal from tool content needs the user's own words")
            }
            "decided" if !own_words && !answers && !accepts => {
                Some("decided needs the user's words or an acceptance right after")
            }
            // A bare "yes" names nothing done, whatever ran (spec 3.3).
            "done" if asked || is_bare || !own_words && runs.is_empty() => {
                Some("done needs the user's words or a passing run")
            }
            "retracted" if !own_words => Some("retracted needs the user's words"),
            _ => None,
        };
        if let Some(why) = below {
            d.status = "proposed".into();
            lower(&d, why);
        }
        // #167: a status the quote's own words do not carry stands on the record that raised it:
        // the acceptance, the reply a bare answer accepts (none when that reply is in an earlier
        // window), or the passing runs. Each is found only where the words are not the user's own.
        let raised: Vec<&Line> = match d.status.as_str() {
            "decided" => acceptance
                .or_else(|| answers.then(|| turn_before(w, i)).flatten().map(|(_, l)| l))
                .into_iter()
                .collect(),
            "done" => runs,
            _ => Vec::new(),
        };
        let mut seqs: Vec<i64> = raised.iter().map(|l| l.seq).collect();
        seqs.dedup();
        // shortcut: at most `MAX_SUPPORT` runs, the window's first; a claim whose kept runs are
        // all removed drops while a later one stands. Keep the nearest if that is seen.
        seqs.truncate(MAX_SUPPORT);
        d.support = seqs
            .into_iter()
            .map(|seq| crate::claims::Support {
                device: w.device.clone(),
                seq,
            })
            .collect();
        g.kept.push((d, evidence));
        own.push((own_words, pick.is_some(), line.repo.as_deref(), &line.key));
    }
    // Once every status is settled: what a sibling is, after the gates. Siblings of one kind
    // whose quotes overlap in one sentence are one claim (`claims::uid`, `curate::number`),
    // settled when any of them is.
    let unsettled = |s: &str| matches!(s, "proposed" | "unverified");
    let uids: Vec<String> = g
        .kept
        .iter()
        .map(|(d, e)| crate::claims::uid(&d.kind, &e[0]))
        .collect();
    let settled: HashSet<&str> = g
        .kept
        .iter()
        .zip(&uids)
        .filter(|((d, _), _)| !unsettled(&d.status))
        .map(|(_, uid)| uid.as_str())
        .collect();
    let siblings: HashMap<String, (String, String, Option<&str>, &str)> = g
        .kept
        .iter()
        .zip(&own)
        .zip(&uids)
        .map(|(((d, _), &(_, _, repo, _)), uid)| {
            let status = if unsettled(&d.status) && settled.contains(uid.as_str()) {
                "decided".to_owned()
            } else {
                d.status.clone()
            };
            (d.id.clone(), (status, d.kind.clone(), repo, uid.as_str()))
        })
        .collect();
    for (((d, _), (own_words, picked, repo, key)), uid) in g.kept.iter_mut().zip(own).zip(&uids) {
        let mut out = Vec::new();
        d.supersedes.retain(|to| {
            // Another repository's claim would leave that repository's current tips, and a
            // sibling that is the same claim would supersede itself.
            let target = match siblings.get(to) {
                Some((status, kind, r, u)) if *u != uid && *r == repo => {
                    Some((status.as_str(), kind.as_str(), "repo"))
                }
                Some(_) => None,
                None => shown
                    .iter()
                    .find(|(r, c)| c.uid == *to && Some(r.as_str()) == repo)
                    .map(|(_, c)| c)
                    .or_else(|| {
                        carried
                            .iter()
                            .find(|(k, r, c)| c.uid == *to && k == key && r.as_deref() == repo)
                            .map(|(_, _, c)| c)
                    })
                    .map(|c| (c.status.as_str(), c.kind.as_str(), c.scope.as_str())),
            };
            let why = match target {
                None => Some(OUTSIDE),
                Some((_, _, "global")) => Some("a global claim changes only by the owner"),
                Some((_, "lesson", _)) if !own_words => {
                    Some("retiring a lesson needs the user's words")
                }
                // An option is one of a choice: a pick settles the proposal that offered it, or
                // overturns what was decided, never the reply's other claims (acc6: a "１" took
                // the reply's change and lesson out with its proposal).
                Some((_, kind, _)) if picked && kind != d.kind => {
                    Some("a pick replaces only a claim of its kind")
                }
                Some((status, ..)) if unsettled(&d.status) && !unsettled(status) => {
                    Some("a proposal supersedes nothing settled")
                }
                _ => None,
            };
            out.extend(why);
            why.is_none()
        });
        // One entry per reason: a draft may name a thousand values, and the window op that lists
        // them must stay within the op cap.
        out.sort_unstable();
        out.dedup();
        g.dropped
            .extend(out.into_iter().map(|why| (d.id.clone(), why)));
    }
    // An unsettled sibling of a settled claim (one the gates lowered) goes first, so the newest
    // derivation of the uid, the one `activate` keeps, is settled.
    g.kept.sort_by_cached_key(|(d, e)| {
        !(unsettled(&d.status) && settled.contains(crate::claims::uid(&d.kind, &e[0]).as_str()))
    });
    g
}

/// A claim's speaker is its quote's line's (spec 3.2), never the curator's word: tool and file
/// content is never the user's. On an `AskUserQuestion` line only the answers are the user's.
fn speaker(line: &Line, quote: &str, given: &str) -> &'static str {
    match line.role {
        Role::User => "user",
        Role::Answer if answer_of(line, quote).is_some() => "user",
        // Its questions and the options the owner did not pick are the assistant's proposals.
        Role::Answer => "assistant proposal",
        Role::Tool { .. } => "tool result",
        // A non-strict curator's case variants are read as the label they are; a label it does
        // not recognize is inferred, which an acceptance never promotes.
        Role::Assistant => match given.trim().to_lowercase().as_str() {
            "assistant proposal" | "user" => "assistant proposal",
            _ => "assistant inferred",
        },
        Role::Other => "assistant inferred",
    }
}

/// The answer (or the note the owner added to one) of an `AskUserQuestion` line that holds
/// `quote`, as decoded or as the output's JSON text shows it.
fn answer_of<'l>(line: &'l Line, quote: &str) -> Option<&'l str> {
    if quote.trim().is_empty() {
        return None;
    }
    line.answers
        .iter()
        .find(|a| {
            let shown = serde_json::to_string(a).unwrap_or_default();
            a.contains(quote) || shown[1..shown.len() - 1].contains(quote)
        })
        .map(String::as_str)
}

/// What the user said on a user turn, for the checks that read a whole turn (a question, bare
/// acceptance): a prompt's whole text, or on an `AskUserQuestion` line the answer that holds
/// `quote` (its questions and other options are the assistant's).
fn turn_said<'l>(line: &'l Line, quote: &str) -> &'l str {
    match line.role {
        Role::Answer => answer_of(line, quote).unwrap_or(""),
        _ => line.source_text(),
    }
}

/// Whether a user turn accepts: a prompt holds an acceptance, or every answer (and note) of an
/// `AskUserQuestion` does, so a pick of "no" to one of its questions accepts nothing.
fn turn_accepts(line: &Line) -> bool {
    match line.role {
        Role::Answer => !line.answers.is_empty() && line.answers.iter().all(|a| acceptance(a)),
        _ => acceptance(&line.text),
    }
}

/// Spec 3.3: a turn that ends in a question mark never promotes.
/// Whether `line` is a part of its event that the window cut before its end: whether the turn
/// ends in a question mark is in a later window, so its words promote nothing here.
// ponytail: a decision in the first part of a prompt longer than a window stays proposed; the
// gates would need the event's end to do better.
fn continues(w: &Window, line: &Line) -> bool {
    w.to_offset.is_some() && line.seq == w.to_seq
}

fn question(text: &str) -> bool {
    text.trim_end().ends_with(['?', '？'])
}

/// The next turn of the same session after line `i`, past tool calls and harness lines, when it is
/// the user's in the same repository and accepts: no other reply of the assistant comes between
/// (spec 3.3, "right after").
fn accepted(w: &Window, i: usize) -> Option<&Line> {
    let line = &w.lines[i];
    w.lines[i + 1..]
        .iter()
        .filter(|l| l.key == line.key)
        .find(|l| matches!(l.role, Role::User | Role::Assistant | Role::Answer))
        // After a checkout change the turn is in another repository: not this proposal's answer.
        .filter(|l| l.role != Role::Assistant && l.repo == line.repo && turn_accepts(l))
}

/// The turn before line `i` in its session, past tool calls and harness lines, and its index.
fn turn_before(w: &Window, i: usize) -> Option<(usize, &Line)> {
    let key = &w.lines[i].key;
    w.lines[..i]
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, l)| &l.key == key)
        .find(|(_, l)| matches!(l.role, Role::User | Role::Assistant | Role::Answer))
}

/// User line `i` accepts, and the turn before it in the same session, past tool calls and harness
/// lines, is a reply of the assistant in the same repository that tool content did not taint: the
/// other end of `accepted`. A reply before a checkout change is another repository's, and the
/// claim anchors on this line. With no turn before it in the window, the session's previous
/// window must have ended on an untainted proposal of this repository (`before.ended`, #144).
fn answers_a_reply(w: &Window, i: usize, tools: &str, before: Before) -> bool {
    let key = &w.lines[i].key;
    turn_accepts(&w.lines[i])
        && match turn_before(w, i) {
            Some((j, l)) => {
                l.role == Role::Assistant
                    && l.repo == w.lines[i].repo
                    && !tainted(w, j, said(&l.text), tools, before.began)
            }
            None => before.ended.contains(key),
        }
}

/// MUST-M4: a proposal whose words came from a tool line of the window: pasted, paraphrased (with
/// a τ), or, until Task 13 records one, made after a tool call in its turn (the provenance
/// fallback). `span` is what it said.
fn tainted(w: &Window, i: usize, span: &str, tools: &str, began: &[String]) -> bool {
    let span = norm(span);
    pasted(&span, tools)
        || match TAU {
            Some(tau) => share(&span, tools) >= tau,
            None => after_a_tool(w, i, began),
        }
}

/// A tool call of line `i`'s session between the start of its turn (the last user line before it,
/// a typed prompt or the owner's answer to `AskUserQuestion`) and it. A turn that began in a
/// window before this one has no start here: `began` names the sessions that were past a tool
/// call in it when this window began (a live window goes at once after a tool call, decision 56,
/// so the reply made from the call's output opens the next).
// ponytail: `pasted` and `share` still read this window's tool lines alone; a τ (Task 13) needs
// the earlier window's tool text for a turn that began there.
fn after_a_tool(w: &Window, i: usize, began: &[String]) -> bool {
    let key = &w.lines[i].key;
    for l in w.lines[..i].iter().rev().filter(|l| &l.key == key) {
        match l.role {
            Role::User | Role::Answer => return false,
            Role::Tool { .. } => return true,
            _ => {}
        }
    }
    began.contains(key)
}

/// A span that shares a run of `PASTE_CHARS` characters with the tool text (the whole span, when
/// shorter): a paste.
fn pasted(span: &[char], tools: &str) -> bool {
    let n = span.len().min(PASTE_CHARS);
    n > 0
        && span
            .windows(n)
            .any(|run| tools.contains(&run.iter().collect::<String>()))
}

/// The share of `span`'s character trigrams that the tool text holds: a paraphrase keeps most.
fn share(span: &[char], tools: &str) -> f64 {
    fn grams(t: &[char]) -> std::collections::HashSet<[char; 3]> {
        t.windows(3).map(|g| [g[0], g[1], g[2]]).collect()
    }
    let own = grams(span);
    if own.is_empty() {
        return 0.0;
    }
    let theirs = grams(&tools.chars().collect::<Vec<_>>());
    own.iter().filter(|g| theirs.contains(*g)).count() as f64 / own.len() as f64
}

/// Text as MUST-M4 compares it: NFKC, lower case, whitespace collapsed.
fn norm(text: &str) -> Vec<char> {
    use unicode_normalization::UnicodeNormalization;
    let folded = text.nfkc().collect::<String>().to_lowercase();
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect()
}

/// Every tool line of the window, as MUST-M4 compares them: each text normalized on its own and
/// joined by a NUL, which no user span holds, so no run crosses from one text into the next (#197).
fn tool_text(w: &Window) -> String {
    let tools: Vec<String> = w
        .lines
        .iter()
        .filter(|l| matches!(l.role, Role::Tool { .. }))
        // The whole output too: a shrink shows only its head and tail (Task 12).
        .flat_map(|l| [l.text.as_str(), l.source_text()])
        .map(|t| norm(t).into_iter().collect())
        .collect();
    tools.join("\0")
}

/// What a line says, without its `[role]` head.
fn said(line: &str) -> &str {
    line.strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or(line, |(_, text)| text)
}

/// The sentence of `line` that `quote` is in (its first occurrence), without its end mark, for
/// MUST-M4's measures.
fn sentence_of<'a>(line: &'a str, quote: &'a str) -> &'a str {
    let text = said(line);
    let Some(at) = text.find(quote) else {
        return quote;
    };
    let ends = |c: &char| crate::curate::SENTENCE_ENDS.contains(c);
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(_, c)| ends(c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let after = at + quote.len();
    let end = text[after..]
        .char_indices()
        .find(|(_, c)| ends(c))
        .map_or(text.len(), |(i, _)| after + i);
    text[start..end].trim()
}

/// A turn that accepts: an acceptance word, no negation, not a question (spec 3.3).
fn acceptance(text: &str) -> bool {
    !question(text) && !negated(text) && holds(text, ACCEPT)
}

/// A turn that says no: a negation, a polite yes aside.
fn negated(text: &str) -> bool {
    // A polite yes can end in ません too ("問題ありません"): those are not a negation.
    let mut plain = text.to_owned();
    for yes in POLITE_YES {
        plain = plain.replace(yes, "");
    }
    holds(&plain, NEGATE)
}

/// A quote that only accepts: it holds an acceptance word, and once its acceptance and filler
/// words are gone at most a few characters are left, so none of the decision is in the user's
/// words (spec 3.3). A short statement with no acceptance word ("直った") is the user's own.
// ponytail: four characters; the dev split's counts (Task 8 part 2) tune it.
fn bare(quote: &str) -> bool {
    holds(quote, ACCEPT) && letters(&unsaid(quote)) <= 4
}

/// The option a quote picks when it says nothing else once its acceptance and filler words are
/// gone: "1", "１", "②", "B", "1番で", "案A" (#252), labelled as `curate::option_label` labels the
/// reply's options.
// ponytail: a few affixes; the dev split's picks tune them.
fn picks(quote: &str) -> Option<String> {
    let rest = unsaid(quote);
    let mut words = rest.split_whitespace();
    let (Some(rest), None) = (words.next(), words.next()) else {
        return None;
    };
    let rest = rest.strip_prefix('案').unwrap_or(rest);
    let rest = rest.trim_end_matches(['番', '目', 'で']);
    let mut c = rest.chars();
    match (c.next(), c.next(), c.next()) {
        (Some(n), None, _) if ('①'..='⑳').contains(&n) => {
            Some((n as u32 - '①' as u32 + 1).to_string())
        }
        (Some(l), None, _) if l.is_ascii_alphabetic() => Some(l.to_string()),
        (Some(a), b, None) if a.is_ascii_digit() && b.is_none_or(|b| b.is_ascii_digit()) => {
            Some(rest.to_owned())
        }
        _ => None,
    }
}

/// User line `i` picks option `label` of the reply it answers: the turn before it in the same
/// session, past tool calls and harness lines, is a reply of the assistant in the same repository
/// that lists it, or with no turn before it in the window, the options its prompt carried from
/// the reply the session's previous window ended on do (`offered`, #252). Not in a turn that asks
/// or says no. A bare yes may answer any proposal of a reply; a pick names one of its options, so
/// the reply may follow a tool call (MUST-M4's provenance fallback is not applied to it,
/// docs/milestone-3.md).
fn picks_an_option(w: &Window, i: usize, label: &str, offered: &[(String, Vec<String>)]) -> bool {
    let line = &w.lines[i];
    !question(&line.text)
        && !negated(&line.text)
        && match turn_before(w, i) {
            Some((_, l)) => {
                l.role == Role::Assistant
                    && l.repo == line.repo
                    && said(&l.text)
                        .lines()
                        .filter_map(crate::curate::option_label)
                        .any(|o| o == label)
            }
            None => offered
                .iter()
                .any(|(key, labels)| *key == line.key && labels.iter().any(|o| o == label)),
        }
}

/// The words `text` has once its acceptance and filler words are gone, lowercase, each between
/// spaces.
fn unsaid(text: &str) -> String {
    let mut rest = format!(" {} ", words(&text.to_lowercase()).join(" "));
    for p in ACCEPT.iter().chain(FILLER) {
        if p.is_ascii() {
            // Neighbours share a space: "yes yes" needs a second pass.
            let spaced = format!(" {p} ");
            while rest.contains(&spaced) {
                rest = rest.replace(&spaced, " ");
            }
        } else {
            rest = rest.replace(p, "");
        }
    }
    rest
}

fn letters(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// A command run of `line`'s session and repository (a run before a checkout change tested
/// another one) that did not fail, printed a pass and no failure (MUST-M1): each `failed` it
/// prints is a `0 failed`, and it reports no error (`1 error`, a line that starts `error:`). Its
/// output only, decoded when structured: an input such as `echo passed` ran nothing, and a read
/// of a file that says tests passed ran none.
fn passing_runs<'w>(w: &'w Window, line: &Line) -> impl Iterator<Item = &'w Line> {
    static NONE_FAILED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ERRORS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static PASS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let none = NONE_FAILED.get_or_init(|| regex::Regex::new(r"\b0 failed").unwrap());
    let pass = PASS.get_or_init(|| regex::Regex::new(PASSED).unwrap());
    let errors = ERRORS.get_or_init(|| {
        regex::Regex::new(r"(?m)\b[1-9]\d*\s+errors?\b|^\s*error(?:\[|:)").unwrap()
    });
    w.lines.iter().filter(move |l| {
        l.key == line.key
            && l.repo == line.repo
            && l.role == (Role::Tool { failed: false })
            && runs(&l.text)
            && !exited_nonzero(l.source_text())
            && {
                let text = decoded(l.source_text()).to_lowercase();
                pass.is_match(&text)
                    && text.matches("failed").count() == none.find_iter(&text).count()
                    && !errors.is_match(&text)
            }
    })
}

/// Whether a run's output says it exited nonzero, whatever the hook's flag: agy prints "The
/// command exited with code 2", Cursor's output carries `exitCode`.
fn exited_nonzero(output: &str) -> bool {
    static EXIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\bexit(?:ed)?(?: with)? (?:code|status):? ?[1-9]").unwrap()
    });
    let code = serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|v| {
            ["exitCode", "exit_code", "returncode"]
                .iter()
                .find_map(|k| v.get(*k).and_then(serde_json::Value::as_i64))
        });
    code.is_some_and(|c| c != 0) || EXIT.is_match(output)
}

/// Whether a tool line is a command run: an agent's shell tool by its whole name (Claude's and
/// Pi's `bash`, Cursor's `Shell`, Codex's `exec_command`, Gemini's `run_shell_command`, Grok's
/// `run_terminal_command`, agy's `run_command`, and the Windows and older shells' names), not a
/// read, a search or an MCP tool that executes something else (`mcp__cloudflare_api__execute`).
fn runs(text: &str) -> bool {
    let name = text
        .strip_prefix("[tool ")
        .and_then(|t| t.split([' ', ']']).next())
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "bash",
        "shell",
        "powershell",
        "exec_command",
        "local_shell",
        "run_shell_command",
        "run_terminal_command",
        "run_command",
    ]
    .contains(&name.as_str())
}

/// A tool's output as text: a structured one (Claude's Bash gives `{"stdout", "stderr", …}`) as
/// its strings, one to a line, so an error at the start of stderr starts a line.
fn decoded(output: &str) -> String {
    fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    match serde_json::from_str::<serde_json::Value>(output) {
        Ok(v) if v.is_object() || v.is_array() => {
            let mut out = Vec::new();
            strings(&v, &mut out);
            out.join("\n")
        }
        _ => output.to_owned(),
    }
}

/// Whether `text` holds one of `list`: a Latin entry as whole words (a word ending in `n't` is
/// `not`), any other anywhere.
fn holds(text: &str, list: &[&str]) -> bool {
    let lower = text.to_lowercase();
    let words = words(&lower);
    let spaced = format!(" {} ", words.join(" "));
    let not = list.contains(&"not") && words.iter().any(|w| w.ends_with("n't"));
    not || list.iter().any(|p| {
        if p.is_ascii() {
            spaced.contains(&format!(" {p} "))
        } else {
            lower.contains(p)
        }
    })
}

/// `text`'s words: letters, digits and apostrophes, split at anything else. A typographic
/// apostrophe (`don’t`, as a phone or a word processor types it) is read as `'`, and full-width
/// Latin (`ＯＫ`, as a Japanese input method types it) as its ASCII.
fn words(text: &str) -> Vec<String> {
    let folded: String = text
        .chars()
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '\u{02bc}' => '\'',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            c => c,
        })
        .collect();
    folded
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A change that says only how many files it touched (spec 3.3: never a claim).
fn bare_file_count(body: &str) -> bool {
    static BARE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    BARE.get_or_init(|| {
        regex::Regex::new(
            // The count and a diff stat's own fields only: any other word explains the change.
            r"(?i)^\s*\d+\s*(?:files?\s+(?:changed|modified)(?:\s*,\s*\d+\s+(?:insertions?|deletions?)\s*\([+-]\))*|ファイル(?:を)?(?:変更|修正)(?:しました)?)\s*[.。]?\s*$",
        )
        .unwrap()
    })
    .is_match(body)
}

#[cfg(test)]
mod tests;
