//! Parity row X10 (docs/my-work.md): claude-mem 14.0's My Work read from the stores, as its
//! `buildHumanRequests` (`request-projection.mjs`) and `statusFor` (`view-model.js`) make it at
//! v14.0.1. A request is an agent's session, or a `request_id` its work state names; its copy is
//! the session summaries the curators write, so a read makes no model call.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::cards::Card;
use crate::raw::{Item, Raw, WorkWrite};
use crate::redact::Rules;
use crate::turns::TurnSummary;

/// The curator chain's role for My Work's jobs (docs/my-work-model.md A1), in the ledger and for a
/// CLI entry's system prompt.
pub(crate) const ROLE: &str = "my_work";

/// What is read, at most, as claude-mem's agent view reads it (`readAgentRecords`,
/// `buildAgentView`): the newest 240 work items, 300 observations (cards here), 100 summaries,
/// of what no exclusion takes (`link`, `newest`).
const WORK: usize = 240;
const CARDS: usize = 300;
const SUMMARIES: usize = 100;
/// The cards of a conversation the judges are shown at most (J2, claude-mem's).
const CONTEXT_CARDS: usize = 8;
/// The sessions found by their prompts alone: those of the newest 2,000 live prompts no exclusion
/// takes, beyond claude-mem, which reads a session only for its work, observations or summaries.
/// Every prompt of a session read is read, as claude-mem reads every prompt of the sessions it
/// loaded, but with no bound of the newest 12,000: the sessions are bounded, and a bound would
/// show a later prompt of a long session as its ask (Codex on #574).
const PROMPTS: usize = 2_000;
/// An ask and a follow-up, in characters, and the follow-ups kept (`requestText`, scope updates).
const ASK: usize = 1_200;
const FOLLOW_UP: usize = 500;
const FOLLOW_UPS: usize = 6;
/// A card's title, summary and next step, in characters (claude-mem's copy).
const TITLE: usize = 100;
const SUMMARY: usize = 240;
const NEXT: usize = 160;
/// claude-mem's `uniqueCall`: a write and the one tool call that made it, this far apart at most.
const CALL_MS: i64 = 5_000;
/// claude-mem's `STALE_MS`: no update for this long is "No recent update".
const QUIET_MS: i64 = 6 * 3_600_000;
/// The cards a request names, at most (claude-mem's activity of a request).
const ACTIVITY: usize = 40;
/// A request's recorded updates, at most (claude-mem's `presentWork`).
const UPDATES: usize = 8;
/// The characters past a shown part of a prompt that the gate reads too, so that a value the cut
/// splits is whole when the rules read it (`hook::clip`'s overlap).
const OVERLAP: usize = 4_000;

/// claude-mem's `fieldText` keys of a work item's next step, outcome and progress (`agent-view.mjs`).
const NEXT_KEYS: &[&str] = &[
    "next_step",
    "nextStep",
    "next",
    "next_action",
    "remaining",
    "pending",
    "then_run",
];
const OUTCOME_KEYS: &[&str] = &["outcome", "result", "completed", "verified", "verification"];
const PROGRESS_KEYS: &[&str] = &[
    "human_summary",
    "summary",
    "progress",
    "phase",
    "stage",
    "status_note",
];
const REQUEST_KEYS: &[&str] = &["human_request", "request", "user_request"];

/// The page's time range: claude-mem's past 24 hours, past 7 days and all recorded work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Range {
    Day,
    Week,
    All,
}

impl Range {
    /// `range` as the route takes it; absent is a day.
    pub(crate) fn of(range: Option<&str>) -> Option<Self> {
        match range {
            None | Some("day") => Some(Self::Day),
            Some("week") => Some(Self::Week),
            Some("all") => Some(Self::All),
            Some(_) => None,
        }
    }

    fn since(self, now: i64) -> i64 {
        match self {
            Self::Day => now - 86_400_000,
            Self::Week => now - 7 * 86_400_000,
            Self::All => i64::MIN,
        }
    }
}

/// An agent's session, by its agent and session as the stores hold them, which the gate only
/// shows (two that a rule masks alike stay two; Codex on #574): claude-mem's conversation
/// (`conversationKey`).
type Key = (String, String);

/// A live prompt as a request shows it.
#[derive(Clone)]
struct Said {
    id: String,
    device: String,
    seq: i64,
    ts: i64,
    /// claude-mem's `requestText` of it before any gate: what tells an ask from an
    /// acknowledgement, never shown as it is (`shown`); none for a prompt with nothing of a
    /// person's in it.
    text: Option<String>,
}

/// The first `max` characters of `text` as a request shows them, as `hook::clip` keeps a field: the
/// gate reads `OVERLAP` more, and what is kept is gated once more, so that a line-scoped allowlist
/// judges the line as it is shown, not the longer one; then a key block the cut leaves open goes,
/// its markers read in any case. The gate reads no more of a long prompt (a delegated review's
/// runs to 20,000 characters).
pub(crate) fn shown(text: &str, max: usize, rules: &Rules) -> String {
    let window: String = text.chars().take(max + OVERLAP).collect();
    let gated = crate::redact::outbound_with(&window, rules);
    let mut kept = if gated.chars().count() > max {
        crate::redact::outbound_with(&clip(&gated, max), rules)
    } else if window.len() < text.len() {
        gated
    } else {
        return gated;
    };
    // ASCII lowercasing keeps byte offsets.
    let lower = kept.to_ascii_lowercase();
    if !rules.is_off()
        && let Some(begin) = lower.rfind("-----begin")
        && !lower[begin..].contains("-----end")
    {
        kept.truncate(begin);
    }
    clip(&kept, max)
}

/// A session summary, with the records it summarizes.
struct Turn {
    s: TurnSummary,
    span: (i64, i64),
}

/// A card, with the records its window holds and its modified files as it stored them, before the
/// gate (a copy is compared with those: #840).
struct Shown {
    c: Card,
    span: (i64, i64),
    files: Vec<String>,
}

/// A prompt's records on its device: from the prompt through the record before the next prompt of
/// its conversation there, claude-mem's prompt (`prompt_number`), which needs no summary.
type Span = (String, i64, i64);

/// Whether prompt span `s` holds any of `device`'s records `from` to `to`.
fn holds(s: &Span, device: &str, (from, to): (i64, i64)) -> bool {
    s.0 == device && s.1 <= to && from <= s.2
}

#[derive(Default)]
struct Conversation {
    repos: BTreeSet<String>,
    /// The repository of its first prompt that names one, as stored: the key linked worktrees
    /// share, which grouping keeps a request within (docs/my-work-model.md C2).
    repo: Option<String>,
    /// The oldest first.
    prompts: Vec<Said>,
    /// Where each of its live prompts is, a forgotten or expired one too, whose text is not read:
    /// what ends one prompt's records and starts the next's (Greptile on #574).
    bounds: Vec<(String, i64)>,
    /// The newest first, as `turns::page` reads them.
    turns: Vec<Turn>,
    /// The newest first, as `cards::page` reads them.
    cards: Vec<Shown>,
}

impl Conversation {
    /// claude-mem's `originalFor`: the first prompt that reads as a person's request. A session
    /// none of whose prompts asks (one resumed with "再開して") has its oldest reply's `request`,
    /// the curator's (the design's 1, beyond claude-mem's), from its first prompt on.
    fn original(&self, local: &str) -> Option<Original> {
        if let Some(p) = (self.prompts.iter()).find(|p| p.text.as_deref().is_some_and(asks)) {
            return Some(Original {
                id: Some(p.id.clone()),
                ts: p.ts,
                text: p.text.clone().unwrap_or_default(),
                from: "prompt",
            });
        }
        let (t, request) = (self.turns.iter().rev()).find_map(|t| {
            let request = t.s.fields.get("request").filter(|r| !r.trim().is_empty())?;
            Some((t, request))
        })?;
        Some(Original {
            id: Some(t.s.id(local)),
            ts: self.prompts.first().map_or(t.s.ts, |p| p.ts),
            text: request.trim().to_owned(),
            from: "summary",
        })
    }

    /// Its prompts' spans, each through the record before the next prompt on its device.
    fn spans(&self) -> Vec<Span> {
        let mut at: Vec<(&str, i64)> = (self.bounds.iter())
            .map(|(device, seq)| (device.as_str(), *seq))
            .collect();
        at.sort_unstable();
        at.dedup();
        (at.iter().enumerate())
            .map(|(i, &(device, seq))| {
                let end = match at.get(i + 1) {
                    Some(&(next, s)) if next == device => s - 1,
                    _ => i64::MAX,
                };
                (device.to_owned(), seq, end)
            })
            .collect()
    }

    /// The prompts that no request owns and that hold a card, and the cards before any prompt,
    /// once: what of the conversation is counted among the unlinked (claude-mem counts each
    /// prompt with observations).
    fn units(&self, owned: &HashSet<Span>) -> usize {
        let spans = self.spans();
        let card_in = |s: &Span| (self.cards.iter()).any(|c| holds(s, &c.c.device, c.span));
        let with_cards = (spans.iter())
            .filter(|s| !owned.contains(*s) && card_in(s))
            .count();
        let before =
            (self.cards.iter()).any(|c| !spans.iter().any(|s| holds(s, &c.c.device, c.span)));
        with_cards + usize::from(before)
    }
}

/// When a write was made and its place in the order the writes fold in: claude-mem's `atEpoch`
/// and `sourceOrder` of a work state event.
type Stamp = (i64, usize);

/// A list's state or one of its tasks, folded as claude-mem folds them: the latest value of each
/// key, a null clearing it.
struct Work {
    repo: String,
    list: String,
    task: Option<String>,
    fields: Map<String, Value>,
    /// The stamp of the write that set each of `fields`.
    set: HashMap<String, Stamp>,
    /// The list's own fields, which a task's request fields fall back to, and their stamps.
    list_fields: Map<String, Value>,
    list_set: HashMap<String, Stamp>,
    updated: i64,
    /// Its last write's place among the writes, which orders writes of one millisecond.
    last: usize,
    /// The oldest first.
    writes: Vec<WorkWrite>,
    /// The writes of the list's own fields, for a task; none for the list itself.
    list_writes: Vec<WorkWrite>,
    /// The conversation of the newest write that one call made (claude-mem's attribution), and
    /// that call's record.
    by: Option<(Key, String, i64)>,
}

impl Work {
    /// claude-mem's `fieldText` of its own fields: the first of `keys` that holds text, as the
    /// gate shows it beside its key, as work state's own lines show it (a rule may match a value
    /// only with its key), whole before the cut: a field is at most 2,000 characters (L3). What the
    /// cut keeps is gated beside its key once more, so that a line-scoped allowlist judges the line
    /// as it is shown (`shown`).
    fn text(&self, keys: &[&str], max: usize, rules: &Rules) -> Option<String> {
        let beside = |k: &str, t: &str| crate::work_state::beside(k, t, rules);
        keys.iter()
            .find_map(|k| Some((*k, self.fields.get(*k).and_then(meaningful)?)))
            .map(|(k, t)| beside(k, &cut(&beside(k, &t), max)))
    }

    /// A request field: its own, else its list's.
    fn either(&self, key: &str) -> Option<&Value> {
        (self.fields.get(key).filter(|v| !v.is_null()))
            .or_else(|| self.list_fields.get(key).filter(|v| !v.is_null()))
    }

    fn request_id(&self) -> Option<String> {
        match self.either("request_id")? {
            v @ (Value::String(_) | Value::Number(_)) => {
                Some(crate::work_state::text(v).trim().to_owned()).filter(|s| !s.is_empty())
            }
            _ => None,
        }
    }

    /// Its recorded request when it reads as one (claude-mem's `explicitRequestText`), as the gate
    /// shows it beside its key.
    fn request_text(&self, rules: &Rules) -> Option<String> {
        let own = |f: &Map<String, Value>| {
            REQUEST_KEYS
                .iter()
                .find_map(|k| Some((*k, f.get(*k).and_then(meaningful)?)))
        };
        let (key, text) = own(&self.fields).or_else(|| own(&self.list_fields))?;
        let text = request_text(&text, usize::MAX).filter(|t| asks(t))?;
        Some(crate::work_state::beside(key, &text, rules))
    }

    fn status(&self) -> String {
        self.fields
            .get("status")
            .map_or_else(String::new, |s| normalized(&crate::work_state::text(s)))
    }

    /// The request's own completion and status as this item records them (claude-mem's lifecycle
    /// records), each its own field else its list's, with the stamp of the write that set it, not
    /// the item's newest (Greptile on #574), and whether it is `request_complete`'s, which comes
    /// first at the same stamp.
    fn lifecycle(&self) -> Vec<(Stamp, bool, String)> {
        let field = |key: &str| match self.fields.get(key).filter(|v| !v.is_null()) {
            Some(v) => Some((v, self.set.get(key))),
            None => (self.list_fields.get(key).filter(|v| !v.is_null()))
                .map(|v| (v, self.list_set.get(key))),
        };
        let mut out = Vec::new();
        if let Some((Value::Bool(done), at)) = field("request_complete") {
            let status = if *done { "done" } else { "open" };
            out.push((at.copied().unwrap_or_default(), true, status.into()));
        }
        if let Some((Value::String(s), at)) = field("request_status")
            && !s.trim().is_empty()
        {
            out.push((at.copied().unwrap_or_default(), false, normalized(s)));
        }
        out
    }
}

/// A recorded text, as claude-mem's `meaningful` reads one: not empty, nor a bare boolean, null or
/// number written as text.
fn meaningful(v: &Value) -> Option<String> {
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(true|false|null|undefined|\d+)$").unwrap());
    let t = v.as_str()?.trim();
    (!t.is_empty() && !BARE.is_match(t)).then(|| t.to_owned())
}

/// claude-mem's `normalizeRecordedStatus`: lower-cased, `_` and spaces read as `-`, and the
/// aliases of needs you and, since claude-mem 15.1, of needs review.
fn normalized(status: &str) -> String {
    let s = status.trim().to_lowercase().replace(['_', ' '], "-");
    match s.as_str() {
        "needs-input" | "waiting-on-user" | "awaiting-approval" | "needs-approval" => {
            "needs-you".into()
        }
        "review" | "in-review" | "awaiting-review" | "ready-for-review" => "needs-review".into(),
        _ => s,
    }
}

/// The work state of `writes`, folded into claude-mem's candidates: each list's tasks, and the
/// list itself when its state has a key besides `task`, newest first.
fn fold(writes: Vec<WorkWrite>) -> Vec<Work> {
    /// A list's state or one of its tasks: what its writes left, when, and the writes.
    #[derive(Default)]
    struct Kept {
        fields: Map<String, Value>,
        /// The stamp of the write that set each of `fields`.
        set: HashMap<String, Stamp>,
        updated: i64,
        last: usize,
        writes: Vec<WorkWrite>,
    }
    impl Kept {
        fn put(&mut self, w: WorkWrite, order: usize) {
            for (k, v) in &w.fields {
                match v {
                    Value::Null => {
                        self.fields.remove(k);
                        self.set.remove(k);
                    }
                    v => {
                        self.fields.insert(k.clone(), v.clone());
                        self.set.insert(k.clone(), (w.ts, order));
                    }
                }
            }
            self.updated = w.ts;
            self.last = order;
            self.writes.push(w);
        }
    }
    struct List {
        repo: String,
        list: String,
        state: Kept,
        tasks: Vec<(String, Kept)>,
        at: HashMap<String, usize>,
    }
    let mut lists: Vec<List> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for (order, w) in writes.into_iter().enumerate() {
        let i = *at
            .entry((w.repo.clone(), w.list.clone()))
            .or_insert_with(|| {
                lists.push(List {
                    repo: w.repo.clone(),
                    list: w.list.clone(),
                    state: Kept::default(),
                    tasks: Vec::new(),
                    at: HashMap::new(),
                });
                lists.len() - 1
            });
        let l = &mut lists[i];
        let task = match w.fields.get("task") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(t) => Some(crate::work_state::text(t)),
        };
        match task {
            None => l.state.put(w, order),
            Some(t) => {
                let j = *l.at.entry(t.clone()).or_insert_with(|| {
                    l.tasks.push((t, Kept::default()));
                    l.tasks.len() - 1
                });
                l.tasks[j].1.put(w, order);
            }
        }
    }
    let mut out = Vec::new();
    for l in lists {
        for (name, task) in l.tasks {
            out.push(Work {
                repo: l.repo.clone(),
                list: l.list.clone(),
                task: Some(name),
                fields: task.fields,
                set: task.set,
                list_fields: l.state.fields.clone(),
                list_set: l.state.set.clone(),
                updated: task.updated,
                last: task.last,
                writes: task.writes,
                list_writes: l.state.writes.clone(),
                by: None,
            });
        }
        if l.state.fields.keys().any(|k| k != "task") {
            out.push(Work {
                repo: l.repo,
                list: l.list,
                task: None,
                fields: l.state.fields.clone(),
                set: l.state.set.clone(),
                list_fields: l.state.fields,
                list_set: l.state.set,
                updated: l.state.updated,
                last: l.state.last,
                writes: l.state.writes,
                list_writes: Vec::new(),
                by: None,
            });
        }
    }
    // Newest first, and of writes in one millisecond the later, as claude-mem reads its events
    // newest first.
    out.sort_by_key(|w| std::cmp::Reverse((w.updated, w.last)));
    out
}

/// A call of oboete's `work_state_write` in a live record: its conversation, record, repository,
/// and the list and fields it sent.
struct Call {
    key: Key,
    device: String,
    seq: i64,
    repo: Option<String>,
    list: String,
    fields: Map<String, Value>,
}

/// The call of oboete's `work_state_write` that live record `(device, seq)` is, if it is one: its
/// conversation, repository, and the list and fields it sent.
fn call(raw: &Raw, (device, seq): (&str, i64)) -> Result<Option<Call>> {
    // An expired record (docs/retention.md) keeps its labels but not what it sent, so, as a
    // forgotten one, it is no call: the work it wrote counts among the unlinked.
    let Some(Item::Event(e)) = (raw.after(device, seq - 1, 1)?.pop())
        .filter(|r| r.seq == seq)
        .map(|r| r.item)
    else {
        return Ok(None);
    };
    let Ok(body) = serde_json::from_str::<Value>(&e.body) else {
        return Ok(None);
    };
    let ours = body["tool"]
        .as_str()
        .and_then(crate::curate::served)
        .is_some_and(|(server, name)| crate::curate::oboete(server) && name == "work_state_write");
    if !crate::raw::is_live(&e.source) || !ours {
        return Ok(None);
    }
    let sent: Option<Value> = match &body["input"] {
        Value::String(s) => serde_json::from_str(s).ok(),
        v => Some(v.clone()),
    };
    let Some(sent) = sent else { return Ok(None) };
    let (Some(list), Some(fields)) = (sent["list"].as_str(), sent["fields"].as_object()) else {
        return Ok(None);
    };
    Ok(Some(Call {
        key: (e.agent.clone(), e.session.clone()),
        device: device.to_owned(),
        seq,
        repo: e.repo.clone(),
        list: list.trim().to_owned(),
        fields: fields.clone(),
    }))
}

/// The calls that sent `write`: of its repository, list and fields, within `CALL_MS`, each with
/// its conversation and record. The tool records near a write are found by time through the fts
/// consumer's index `raw_docs_tools`, and each is read once (CodeRabbit on #574). The hook gates
/// a call's record and capture gates the write each its own way (a rule can mask a value beside
/// its key but not in the call's text), so when no call's list and fields are the write's, the
/// calls are compared again as work state keeps both now (`settings`); a call that is the write's
/// as stored is never joined by another masked alike (#574).
fn senders(
    raw: &Raw,
    near: &mut rusqlite::CachedStatement<'_>,
    read: &mut HashMap<(String, i64), Option<Call>>,
    (write, settings): (&WorkWrite, &crate::capture::Settings),
) -> Result<Vec<(Key, String, i64)>> {
    let span = params![write.ts - CALL_MS, write.ts + CALL_MS];
    let rows: Vec<(String, i64)> = near
        .query_map(span, |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let (mut sent, mut other) = (Vec::new(), Vec::new());
    for record in rows {
        if !read.contains_key(&record) {
            let c = call(raw, (&record.0, record.1))?;
            read.insert(record.clone(), c);
        }
        if let Some(c) = &read[&record]
            && c.repo.as_deref() == Some(write.repo.as_str())
        {
            if c.list == write.list && c.fields == write.fields {
                sent.push((c.key.clone(), c.device.clone(), c.seq));
            } else {
                other.push(record);
            }
        }
    }
    // shortcut: a mask that breaks the call's text as JSON, or a run the record's gate alone
    // masks (`base64_runs`), still leaves the write with no sender; read the call from the hook's
    // payload before its gate if such writes are seen unlinked.
    if sent.is_empty() && !other.is_empty() {
        let kept = |list: &str, fields: &Map<String, Value>| {
            crate::capture::work_state(list, fields, settings)
        };
        let written = kept(&write.list, &write.fields);
        sent = (other.iter().filter_map(|record| read[record].as_ref()))
            .filter(|c| kept(&c.list, &c.fields) == written)
            .map(|c| (c.key.clone(), c.device.clone(), c.seq))
            .collect();
    }
    Ok(sent)
}

/// claude-mem's attribution: each item is the work of the conversation of its newest write that
/// exactly one call sent (`senders`), none before the worker has made `raw_docs_tools`. The
/// newest `WORK` items of `work`, newest first, that no `excluded` conversation wrote: only the
/// link names an excluded session's work, so its items are linked and left out until `WORK` are
/// kept, and take no part in the bound (Codex on #574). An item no call ties to a session may be
/// an excluded session's too, so while a session is excluded it is left out as well; before the
/// index every item is such. An item shows what each of its writes left, a task the state of its
/// list too, so while a session is excluded it is also left out unless a call of a session not
/// excluded, and none of an excluded one's, sent each of those writes (Codex on #574).
fn link(
    (raw, k, rules): (&Raw, &Connection, &Rules),
    work: Vec<Work>,
    excluded: &HashSet<Key>,
) -> Result<Vec<Work>> {
    if !crate::consumer::manifest::exists(k, "index", "raw_docs_tools")? {
        let shown = if excluded.is_empty() { WORK } else { 0 };
        return Ok(work.into_iter().take(shown).collect());
    }
    let settings = crate::capture::Settings {
        rules: rules.clone(),
        ..crate::capture::Settings::default()
    };
    let mut near = k.prepare_cached(
        "SELECT device, seq FROM raw_docs INDEXED BY raw_docs_tools
         WHERE kind = 'tool' AND ts BETWEEN ?1 AND ?2",
    )?;
    let mut read = HashMap::new();
    let mut kept = Vec::new();
    for mut w in work {
        if kept.len() == WORK {
            break;
        }
        for write in w.writes.iter().rev() {
            if let [c] = &senders(raw, &mut near, &mut read, (write, &settings))?[..] {
                w.by = Some(c.clone());
                break;
            }
        }
        // shortcut: while a session is excluded, work of no known session, and a write no call
        // sent, are taken as possibly that session's in every repository, not only those it
        // touched; narrow it when a home with an exclusion loses work it should show.
        let mut shown =
            (w.by.as_ref()).map_or(excluded.is_empty(), |(key, ..)| !excluded.contains(key));
        if shown && !excluded.is_empty() {
            for write in w.writes.iter().chain(&w.list_writes) {
                let sent = senders(raw, &mut near, &mut read, (write, &settings))?;
                if sent.is_empty() || sent.iter().any(|(key, ..)| excluded.contains(key)) {
                    shown = false;
                    break;
                }
            }
        }
        if shown {
            kept.push(w);
        }
    }
    Ok(kept)
}

/// The newest `bound` of the records `page` reads that `keep` takes, newest first: the reader
/// pages on from each page's last record past the rest, so what an excluded session wrote takes
/// no part in a bound and pushes out no older record (Codex on #574).
fn newest<T, P>(
    bound: usize,
    mut page: impl FnMut(Option<&P>) -> Result<(Vec<T>, bool)>,
    position: impl Fn(&T) -> P,
    keep: impl Fn(&T) -> bool,
) -> Result<Vec<T>> {
    let (mut kept, mut before) = (Vec::new(), None);
    loop {
        let (records, more) = page(before.as_ref())?;
        before = records.last().map(&position);
        kept.extend(records.into_iter().filter(&keep));
        if kept.len() >= bound || !more {
            kept.truncate(bound);
            return Ok(kept);
        }
    }
}

/// What a request takes of a conversation: all of it, or the prompts its writes were made in,
/// with their replies and cards.
enum Owns {
    All,
    Prompts(HashSet<Span>),
}

/// A request's original ask: a prompt, the oldest reply's `request` of a session none of whose
/// prompts asks, or the recorded request of a `request_id` of no session.
#[derive(Clone)]
struct Original {
    /// The ID `get` takes: the prompt's record or the summary; none for a work item's request.
    id: Option<String>,
    ts: i64,
    /// Before the gate, which `answer` puts it through when it shows it.
    text: String,
    /// `prompt`, `summary` or `work`.
    from: &'static str,
}

/// A request's identity (#570): an agent's session, a `request_id`, as the stores hold them, or a
/// group grouping made, by its anchor conversation (docs/my-work-model.md C6); the answer gates it.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Id {
    Conversation(Key),
    Explicit(String),
    Group(Key),
}

impl Id {
    /// As the stores hold it: what a check is filed under (rule 6), and what a masked id's
    /// fingerprint is of.
    fn held(&self) -> String {
        match self {
            Id::Conversation((agent, session)) => format!("c:{agent}:{session}"),
            Id::Explicit(e) => format!("r:{e}"),
            Id::Group((agent, session)) => format!("g:{agent}:{session}"),
        }
    }
}

struct Request {
    id: Id,
    original: Original,
    explicit_text: Option<String>,
    /// A group's title, the newest grouping gave it (C6), and the grouping ops behind the group.
    title: Option<String>,
    group_ops: Vec<(String, i64)>,
    owns: Vec<(Key, Owns)>,
    work: Vec<usize>,
    /// J4: the held id of the request a judge's open suggestion on its anchor names, the one the
    /// picked anchor is in now.
    suggested: Option<String>,
}

impl Request {
    fn new(id: Id, original: Original) -> Self {
        Self {
            id,
            original,
            explicit_text: None,
            title: None,
            group_ops: Vec::new(),
            owns: Vec::new(),
            work: Vec::new(),
            suggested: None,
        }
    }

    fn take(&mut self, key: &Key, own: Owns) {
        match self.owns.iter_mut().find(|(k, _)| k == key) {
            None => self.owns.push((key.clone(), own)),
            Some((_, Owns::All)) => {}
            Some((_, have @ Owns::Prompts(_))) => match own {
                Owns::All => *have = Owns::All,
                Owns::Prompts(more) => {
                    if let Owns::Prompts(p) = have {
                        p.extend(more);
                    }
                }
            },
        }
    }
}

/// The requests of `range` at `now`, the newest update first, and how many work records no
/// request takes (claude-mem's "N work records need a linked human request"), without the cards'
/// deliverables (`listed`, the route's).
#[cfg(test)]
pub(crate) fn read(home: &Path, range: Range, now: i64) -> Result<Value> {
    Ok(read_held(home, range, now, None)?.0)
}

/// `read` as the page's list shows it: each request's card with its deliverables (docs/
/// deliverables.md, slice 2b), a copy's link tag made by `tag`.
pub(crate) fn listed(home: &Path, range: Range, now: i64, tag: Tag) -> Result<Value> {
    Ok(read_held(home, range, now, Some(tag))?.0)
}

/// What makes a copy's link tag (the viewer's `file_tag`).
pub(crate) type Tag<'a> = &'a dyn Fn(&crate::deliverables::Row) -> Result<String>;

/// The held id of the request `anchor` anchors (C6), as `Held::id` gives it.
pub(crate) fn anchored(anchor: &Key) -> String {
    Id::Group(anchor.clone()).held()
}

/// `read` of every range, with what each request's check is filed under, in the answer's order:
/// the review route's (rule 6).
pub(crate) fn reviewing(home: &Path, now: i64) -> Result<(Value, Vec<Held>)> {
    read_held(home, Range::All, now, None)
}

/// What My Work reads of the stores before it makes requests: each conversation with its
/// prompts, summaries and cards, the work and who wrote it, and the request ids each
/// conversation's work names. The page's read and grouping's (docs/my-work-model.md C1) share it.
struct Gathered {
    convs: BTreeMap<Key, Conversation>,
    work: Vec<Work>,
    named: HashMap<Key, BTreeSet<String>>,
}

fn gather(raw: &Raw, k: &Connection, rules: &Rules) -> Result<Gathered> {
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    // An excluded repository's sessions take no part, nor its work (the design's 1).
    let excluded_repos = raw.exclusions_now()?;
    let excluded: HashSet<Key> = (raw.sessions_in(&excluded_repos)?.iter())
        .filter_map(|s| s.split_once('\0'))
        .map(|(a, s)| (a.to_owned(), s.to_owned()))
        .collect();
    let mut convs: BTreeMap<Key, Conversation> = BTreeMap::new();
    // A summary's and a card's span and session as their tables hold them: the readers gate the
    // labels they return. A row gone since its page was read (a forget, a purge, a recuration) is
    // none, and takes no part.
    type SummaryRow = ((i64, i64), Key);
    let summaries = newest(
        SUMMARIES,
        |before| {
            let (page, more) = crate::turns::page(k, raw, (None, None), before, SUMMARIES, rules)?;
            let rows = (page.into_iter())
                .map(|s| {
                    let row: Option<SummaryRow> = k
                        .query_row(
                            "SELECT from_seq, through, agent, session FROM turns
                             WHERE device = ?1 AND op_seq = ?2",
                            params![s.device, s.op_seq],
                            |r| Ok(((r.get(0)?, r.get(1)?), (r.get(2)?, r.get(3)?))),
                        )
                        .optional()?;
                    Ok((s, row))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((rows, more))
        },
        |(s, _)| s.position(),
        |(_, row)| row.as_ref().is_some_and(|(_, key)| !excluded.contains(key)),
    )?;
    for (s, (span, key)) in (summaries.into_iter()).filter_map(|(s, row)| Some((s, row?))) {
        let c = convs.entry(key).or_default();
        c.repos.extend(s.repo.clone());
        c.turns.push(Turn { s, span });
    }
    let cards = newest(
        CARDS,
        |before| {
            let (page, more) = crate::cards::page(k, raw, (None, None), before, CARDS, rules)?;
            let rows = (page.into_iter())
                .map(|c| {
                    type Row = ((i64, i64), Option<String>, Option<String>, String);
                    let row: Option<Row> = k
                        .query_row(
                            "SELECT from_seq, to_seq, agent, session, files_modified FROM cards
                             WHERE device = ?1 AND op_seq = ?2 AND n = ?3",
                            params![c.device, c.op_seq, c.n],
                            |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?, r.get(3)?, r.get(4)?)),
                        )
                        .optional()?;
                    let Some((span, agent, session, files)) = row else {
                        return Ok((c, None));
                    };
                    // As `cards::read` parses them before it gates each.
                    let files: Vec<String> = serde_json::from_str(&files).unwrap_or_default();
                    Ok((c, Some((span, agent.zip(session), files))))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((rows, more))
        },
        |(c, _)| c.position(),
        |(_, row)| {
            (row.as_ref())
                .is_some_and(|(_, key, _)| key.as_ref().is_some_and(|k| !excluded.contains(k)))
        },
    )?;
    for (card, (span, key, files)) in (cards.into_iter()).filter_map(|(c, row)| Some((c, row?))) {
        let Some(key) = key else { continue };
        let c = convs.entry(key).or_default();
        c.repos.extend(card.repo.clone());
        c.cards.push(Shown {
            c: card,
            span,
            files,
        });
    }
    let writes = (raw.native_work_writes()?.into_iter())
        .filter(|w| !excluded_repos.contains(&w.repo))
        .collect();
    let work = link((raw, k, rules), fold(writes), &excluded)?;
    // Every prompt of the sessions read (`PROMPTS`): the work's, the summaries', the cards', and
    // those of the newest live prompts whose text is there, each prompt kept only as `at`
    // returns it: what a forget took or an expiry left without text takes no part (Codex on #574).
    let labels = raw.prompt_labels()?;
    let mut sessions: HashSet<Key> = (labels.iter())
        .filter(|(.., key, shown)| *shown && !excluded.contains(key))
        .take(PROMPTS)
        .map(|(_, _, key, _)| key.clone())
        .collect();
    sessions.extend(convs.keys().cloned());
    sessions.extend(work.iter().filter_map(|w| Some(w.by.as_ref()?.0.clone())));
    let mut seqs: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (device, seq, key, _) in &labels {
        if sessions.contains(key) {
            seqs.entry(device).or_default().push(*seq);
        }
    }
    let mut said = Vec::new();
    for (device, seqs) in seqs {
        for r in raw.at(device, &seqs)? {
            let Item::Event(e) = r.item else { continue };
            let e = *e;
            let text = crate::curate::long_text(&e).unwrap_or_default();
            let p = Said {
                id: format!("{device}:{}", r.seq),
                device: device.to_owned(),
                seq: r.seq,
                ts: e.ts,
                text: request_text(&text, usize::MAX),
            };
            said.push(((e.agent, e.session), e.repo, p));
        }
    }
    said.sort_by(|(.., a), (.., b)| (a.ts, &a.device, a.seq).cmp(&(b.ts, &b.device, b.seq)));
    for (key, repo, p) in said {
        let c = convs.entry(key).or_default();
        c.repos.extend(repo.as_deref().map(gate));
        if c.repo.is_none() {
            c.repo = repo;
        }
        c.prompts.push(p);
    }
    // A conversation known only by its work (its prompts private, expired or forgotten, or none
    // recorded) is still the one its writes came from, as claude-mem credits a request to the
    // session of the call that wrote it (#596).
    for (key, ..) in work.iter().filter_map(|w| w.by.as_ref()) {
        convs.entry(key.clone()).or_default();
    }
    // Where each conversation's prompts are, a forgotten or expired one's too, whose text is not
    // read: by its row until a purge, then by what the forget keeps (Greptile on #574).
    for (device, seq, key, _) in &labels {
        if let Some(c) = convs.get_mut(key) {
            c.bounds.push((device.clone(), *seq));
        }
    }
    let forgotten = raw.forgotten_prompts()?;
    if !forgotten.is_empty() {
        let hashes: HashMap<String, Key> = (convs.keys())
            .map(|k| (crate::forget::session(&k.0, &k.1), k.clone()))
            .collect();
        for (hash, device, seq) in forgotten {
            if let Some(c) = hashes.get(&hash).and_then(|k| convs.get_mut(k)) {
                c.bounds.push((device, seq));
            }
        }
    }

    // claude-mem's `buildHumanRequests`: the request ids each conversation's work names, then
    // each item's request, then the conversations' own records.
    let mut named: HashMap<Key, BTreeSet<String>> = HashMap::new();
    for w in &work {
        if let (Some((key, ..)), Some(id)) = (&w.by, w.request_id()) {
            named.entry(key.clone()).or_default().insert(id);
        }
    }
    Ok(Gathered { convs, work, named })
}

fn read_held(
    home: &Path,
    range: Range,
    now: i64,
    cards: Option<Tag>,
) -> Result<(Value, Vec<Held>)> {
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok((json!({"requests": [], "unlinked": 0}), Vec::new()));
    };
    // The page's list (`listed`) is read in the readers' hold from its first read to its answer
    // (#439), as a request's page is: a forget registered before it leaves out what it took, and
    // one registered meanwhile waits. `reviewing`, which the owner's actions read under the
    // reviews' lock, takes none, as a purge holds the readers' hold while it waits for that lock
    // (Codex on slice 2).
    let _read = match cards {
        Some(_) => Some(crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?),
        None => None,
    };
    let rules = Rules::load(home)?;
    let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live)?;
    let Built {
        convs,
        work,
        requests,
        unlinked,
    } = build(&raw, &k, &rules, &reading)?;
    let since = range.since(now);
    let mut answers = Vec::new();
    let mut key = None;
    let reviews = reviews(home);
    let hidden = crate::my_work_ops::hidden(&raw, &k, &reading)?;
    // V4: the current goals once, for each card's goal (L3).
    let goals = (crate::my_work_goals::newest(&k, raw.device(), &hidden, now)?)
        .map(|(_, items)| items)
        .unwrap_or_default();
    let mut holders: HashMap<&Key, usize> = HashMap::new();
    for (key, _) in requests.iter().flat_map(|r| &r.owns) {
        *holders.entry(key).or_default() += 1;
    }
    // Slice 2 of docs/subagents.md: every request's conversations, for their subagents, whatever
    // the range, as Mark finished looks a subagent up (the page reads all and filters itself;
    // Codex and cubic on #850).
    let mut listed: BTreeSet<Key> = BTreeSet::new();
    for r in &requests {
        listed.extend(r.owns.iter().map(|(key, _)| key.clone()));
        let stores = (&raw, &k, &rules, &hidden);
        let found = answer(
            r,
            (&convs, &work, &reviews),
            stores,
            (home, &mut key),
            (since, now),
        )?;
        if let Some((v, mut h)) = found {
            h.sole = (r.owns.iter())
                .filter(|(key, _)| holders.get(key) == Some(&1))
                .map(|(key, _)| key.clone())
                .collect();
            answers.push((v, h));
        }
    }
    answers.sort_by_key(|(v, _)| std::cmp::Reverse(v["updated"].as_i64().unwrap_or(0)));
    let (mut answers, held): (Vec<Value>, Vec<Held>) = answers.into_iter().unzip();
    // Slice 2 of docs/deliverables.md: each request's copies, read once, for a fingerprint by which
    // the page reads its products again when one lands or goes and, in the list's answer, for its
    // card (slice 2b), both in the readers' hold there (#439), as a request's page builds them. A
    // copies' store that does not read leaves the list without them, not without its requests.
    let copies = (|| -> Result<Option<Vec<Vec<crate::deliverables::Row>>>> {
        let Some(db) = crate::deliverables::open_read(home)? else {
            return Ok(None);
        };
        Ok(Some(
            (held.iter().map(|h| copies_of(&db, &raw, h))).collect::<Result<_>>()?,
        ))
    })()
    .unwrap_or_else(|e| {
        eprintln!("oboete: the copies of My Work's requests not read: {e:#}");
        None
    });
    for (a, copies) in answers.iter_mut().zip(copies.iter().flatten()) {
        a["copies"] = json!(crate::deliverables::fingerprint(copies));
    }
    if let Some(tag) = cards {
        for (i, (a, h)) in answers.iter_mut().zip(&held).enumerate() {
            let copies = copies.as_ref().map_or(&[][..], |c| c[i].as_slice());
            // A request whose products do not read has no card; the others keep theirs.
            match shown_products(home, (&raw, &k, &rules), &h.sources, copies, tag) {
                Ok(list) => {
                    if let Some((deliverables, count)) = card(list) {
                        a["deliverables"] = json!(deliverables);
                        a["deliverable_count"] = json!(count);
                    }
                }
                Err(e) => eprintln!("oboete: a My Work card's deliverables not read: {e:#}"),
            }
        }
    }
    // F2, F7: where each request sits, and the areas.
    let focus = crate::my_work_focus::Focus::read(&raw, &k, &reading)?;
    for (a, h) in answers.iter_mut().zip(&held) {
        a["focus"] = json!(focus.of(&crate::my_work_focus::subject(h.id())));
    }
    // J4: the request a suggestion names, as this answer shows it; none when it shows no such.
    let shown: HashMap<&str, usize> = (held.iter().enumerate())
        .map(|(i, h)| (h.id(), i))
        .collect();
    for (i, h) in held.iter().enumerate() {
        let Some(&t) = h.suggested.as_deref().and_then(|t| shown.get(t)) else {
            continue;
        };
        let t = &answers[t];
        let suggestion = json!({"id": t["id"], "fingerprint": t["fingerprint"], "title": t["title"],
            "conversation": h.about});
        answers[i]["suggestion"] = suggestion;
    }
    let areas = crate::my_work_focus::areas(&focus, &goals, &rules);
    let other_asks = crate::my_work_focus::other_asks(&focus, &goals, &rules);
    // P6, V4: the build story beside the requests.
    let story = crate::my_work_story::page(&k, raw.device(), &hidden, &rules)?;
    // The page's read alone (`listed`), in its readers' hold.
    let subagents = match cards {
        Some(_) => subagents_of(home, (&raw, &k, &rules), &listed, now, &mut key)?,
        None => Vec::new(),
    };
    let answer = json!({"requests": answers, "unlinked": unlinked, "story": story,
        "areas": areas, "other_asks": other_asks, "page_words": crate::my_work_focus::page_words(),
        "subagents": subagents});
    Ok((answer, held))
}

/// Slice 2 of docs/subagents.md: the subagents of the `listed` conversations as the answer gives
/// them, each conversation by the id its request's `sessions` give it. Read in the list's readers'
/// hold, which the caller holds: a forget registers before this read or after it.
fn subagents_of(
    home: &Path,
    (raw, k, rules): (&Raw, &Connection, &Rules),
    listed: &BTreeSet<Key>,
    now: i64,
    key: &mut Option<Vec<u8>>,
) -> Result<Vec<Value>> {
    let found =
        crate::my_work_subagents::read(raw, k, &listed.iter().cloned().collect::<Vec<_>>(), rules)?;
    let mut conversations: HashMap<Key, (String, Option<String>)> = HashMap::new();
    for s in &found {
        if !conversations.contains_key(&s.conversation) {
            let id = shown_id(&Id::Conversation(s.conversation.clone()), rules, home, key)?;
            conversations.insert(s.conversation.clone(), id);
        }
    }
    crate::my_work_subagents::answer(
        &found,
        &conversations,
        &mut |s| {
            Ok(format!(
                "s:{}",
                keyed_print(&crate::my_work_subagents::held(s), home, key)?
            ))
        },
        rules,
        now,
    )
}

/// Slice 2: the listed subagent the page names `id`, read afresh from `raw` and `k`, with the folder
/// its conversation's newest record was recorded in (none when that label is gone or masked). The
/// caller holds the readers' hold, taken after it opened them.
pub(crate) fn subagent(
    home: &Path,
    (raw, k): (&Raw, &Connection),
    id: &str,
) -> Result<Option<(crate::my_work_subagents::Subagent, Option<String>)>> {
    let rules = Rules::load(home)?;
    let reading = crate::curate::Reading::now(raw, crate::curate::Reads::Live)?;
    let Built { requests, .. } = build(raw, k, &rules, &reading)?;
    let listed: Vec<Key> = (requests.iter().flat_map(|r| &r.owns))
        .map(|(key, _)| key.clone())
        .collect();
    let mut key = None;
    for s in crate::my_work_subagents::read(raw, k, &listed, &rules)? {
        let shown = format!(
            "s:{}",
            keyed_print(&crate::my_work_subagents::held(&s), home, &mut key)?
        );
        if shown == id {
            let folder = crate::my_work_subagents::recorded_folder(raw, &s)?;
            return Ok(Some((s, folder)));
        }
    }
    Ok(None)
}

/// docs/my-work-model.md P1, P3: what a progress review reads of a request, its texts before any
/// gate. Its records and ops are this device's only: a review lists what it was shown by this
/// device's seqs (S2), so a forget here reaches it (S5).
pub(crate) struct ReviewInput {
    pub id: String,
    /// Every conversation, another device's too, sorted: a new one makes the review due (P2).
    pub conversations: Vec<Key>,
    /// When it was asked, and its latest activity, unix ms.
    pub asked: i64,
    pub activity: i64,
    /// Its original ask, when it was read from this device or a work item.
    pub original: Option<(AskFrom, String)>,
    /// Its later prompts, the oldest first: record seq, time and text.
    pub asks: Vec<(i64, i64, String)>,
    /// F3: each of its other conversations' opening ask, this device's, by its conversation's
    /// order: record seq and text.
    pub openings: Vec<(i64, String)>,
    /// Its repositories as the gate shows them.
    pub repos: Vec<String>,
    /// Its session summaries, the newest first: turn op seq, time and fields.
    pub summaries: Vec<(i64, i64, BTreeMap<String, String>)>,
    /// Its cards, the newest first: window op seq, time and title.
    pub cards: Vec<(i64, i64, String)>,
    /// Its work items, the newest first: list, task, status and the time of the last write.
    pub work: Vec<(String, Option<String>, String, i64)>,
    pub sources: Sources,
    /// Its group's title with the op seqs of the grouping ops behind it (L2), when each of them is
    /// this device's.
    pub title: Option<(String, Vec<i64>)>,
}

/// Where a request's original ask was read from (S5): a prompt record, a summary's turn op, both by
/// seq, or a work item, which no forget takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AskFrom {
    Prompt(i64),
    Summary(i64),
    Work,
}

/// docs/my-work-model.md P1, S6: each request My Work shows from a group or a request id, with
/// activity since `active_since`, as a progress review reads it: an excluded repository's sessions
/// take no part (`gather`). A lone conversation is reviewed by no one, as in claude-mem.
pub(crate) fn review_inputs(
    raw: &Raw,
    k: &Connection,
    rules: &Rules,
    reading: &crate::curate::Reading,
    active_since: i64,
) -> Result<Vec<ReviewInput>> {
    let Built {
        convs,
        work,
        requests,
        ..
    } = build(raw, k, rules, reading)?;
    let mut out = Vec::new();
    for r in &requests {
        if matches!(r.id, Id::Conversation(_)) {
            continue;
        }
        let p = parts(r, &convs, &work, rules);
        let activity = (p.turns.iter().map(|t| t.s.ts))
            .chain(p.cards.iter().map(|c| c.c.ts))
            .chain(p.active.iter().copied())
            .chain(p.items.iter().map(|w| w.updated))
            .max()
            .unwrap_or(0);
        if activity < active_since {
            continue;
        }
        let mut conversations: Vec<Key> = r.owns.iter().map(|(key, _)| key.clone()).collect();
        conversations.sort();
        let device = raw.device();
        let original = &r.original;
        let from = match original.from {
            "prompt" => (p.prompts.iter())
                .find(|s| s.device == device && original.id.as_ref() == Some(&s.id))
                .map(|s| AskFrom::Prompt(s.seq)),
            "summary" => (p.turns.iter())
                .find(|t| t.s.device == device && original.id == Some(t.s.id(device)))
                .map(|t| AskFrom::Summary(t.s.op_seq)),
            _ => Some(AskFrom::Work),
        };
        // The prompts it owns: one that owns some replies of a conversation shares its prompts
        // on the page, but its review reads only its own.
        let (mut asks, mut openings) = (Vec::new(), Vec::new());
        for (key, own) in &r.owns {
            let Some(c) = convs.get(key) else { continue };
            // F3: the original's conversation is shown apart and opens none (CodeRabbit on #753).
            let mut opened = (c.prompts.iter()).any(|p| original.id.as_ref() == Some(&p.id));
            for s in (c.prompts.iter()).filter(|s| s.device == device) {
                let ours = match own {
                    Owns::All => true,
                    Owns::Prompts(spans) => {
                        spans.iter().any(|sp| holds(sp, device, (s.seq, s.seq)))
                    }
                };
                // As the page's follow-ups: from the original on, which is shown apart.
                let later = s.ts >= original.ts && original.id != Some(s.id.clone());
                if let Some(text) = (s.text.as_ref()).filter(|_| ours && later) {
                    asks.push((s.seq, s.ts, text.clone()));
                }
                // A conversation's first ask, as `Conversation::original` reads one.
                if let Some(text) = (s.text.as_ref()).filter(|t| ours && !opened && self::asks(t)) {
                    opened = true;
                    openings.push((s.seq, text.clone()));
                }
            }
        }
        asks.sort_by_key(|a| (a.1, a.0));
        let summaries = (p.turns.iter())
            .filter(|t| t.s.device == device)
            .map(|t| (t.s.op_seq, t.s.ts, t.s.fields.clone()))
            .collect();
        let cards = (p.cards.iter())
            .filter(|c| c.c.device == device)
            .map(|c| (c.c.op_seq, c.c.ts, c.c.title.clone()))
            .collect();
        let mut items: Vec<&&Work> = p.items.iter().collect();
        items.sort_by_key(|w| std::cmp::Reverse(w.updated));
        let work = (items.into_iter())
            .map(|w| (w.list.clone(), w.task.clone(), w.status(), w.updated))
            .collect();
        let sources = sources(r, &p.items, &p.cards);
        let ours = r.group_ops.iter().all(|(d, _)| d == device);
        let title = (r.title.clone())
            .filter(|_| ours)
            .map(|title| (title, r.group_ops.iter().map(|(_, seq)| *seq).collect()));
        out.push(ReviewInput {
            id: r.id.held(),
            conversations,
            asked: original.ts,
            activity,
            original: from.map(|from| (from, original.text.clone())),
            asks,
            openings,
            repos: p.repos.iter().cloned().collect(),
            summaries,
            cards,
            work,
            sources,
            title,
        });
    }
    Ok(out)
}

/// What My Work makes of the stores before it answers the page or reviews a request: the
/// conversations and work it gathered, the requests they make, and how many work records no
/// request takes.
struct Built {
    convs: BTreeMap<Key, Conversation>,
    work: Vec<Work>,
    requests: Vec<Request>,
    unlinked: usize,
}

fn build(
    raw: &Raw,
    k: &Connection,
    rules: &Rules,
    reading: &crate::curate::Reading,
) -> Result<Built> {
    let Gathered { convs, work, named } = gather(raw, k, rules)?;
    let grouped = crate::my_work_group::groups(raw, k, reading)?;
    // A conversation's request when it names no request id: its group's, else its own (C6).
    let own = |key: &Key| {
        (grouped.of.get(key))
            .map_or_else(|| Id::Conversation(key.clone()), |a| Id::Group(a.clone()))
    };
    let mut requests: Vec<Request> = Vec::new();
    let mut at: HashMap<Id, usize> = HashMap::new();
    let mut unlinked = 0;
    // A group member's work item with no opening ask and no request named waits, as its
    // conversation does, for the member that makes the group (#686).
    let mut deferred_work = Vec::new();
    for (i, w) in work.iter().enumerate() {
        let key = w.by.as_ref().map(|(key, ..)| key);
        let ids = key.and_then(|k| named.get(k));
        let own_id = w.request_id();
        if own_id.is_none() && ids.is_some_and(|s| s.len() > 1) {
            unlinked += 1;
            continue;
        }
        let explicit = own_id.or_else(|| ids.filter(|s| s.len() == 1)?.first().cloned());
        let explicit_text = w.request_text(rules);
        let original = key
            .and_then(|k| convs.get(k)?.original(raw.device()))
            .or_else(|| {
                let text = explicit.as_ref().and(explicit_text.clone())?;
                Some(Original {
                    id: None,
                    ts: w.updated,
                    text,
                    from: "work",
                })
            });
        let (Some(original), true) = (original, key.is_some() || explicit.is_some()) else {
            match key.map(own) {
                Some(id @ Id::Group(_)) if explicit.is_none() => deferred_work.push((i, id)),
                _ => unlinked += 1,
            }
            continue;
        };
        let id = explicit.map_or_else(|| own(key.unwrap()), Id::Explicit);
        let r = match at.get(&id) {
            Some(&j) => &mut requests[j],
            None => {
                at.insert(id.clone(), requests.len());
                requests.push(Request::new(id, original.clone()));
                requests.last_mut().unwrap()
            }
        };
        if original.ts < r.original.ts {
            r.original = original;
        }
        if r.explicit_text.is_none() {
            r.explicit_text = explicit_text;
        }
        r.work.push(i);
    }
    // A group's member with no opening ask waits for the member that makes the group, whatever
    // their order (#684).
    let mut deferred = Vec::new();
    for (key, c) in &convs {
        let ids = named
            .get(key)
            .map(|s| s.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if ids.len() > 1 {
            // Each request owns the prompts its writes were made in (the design's 1), as
            // claude-mem gives such a request the observations of those prompts: a prompt runs to
            // the next, so no summary is needed to tell them (Greptile on #574).
            let spans = c.spans();
            let mut owned = HashSet::new();
            for id in &ids {
                let Some(&j) = at.get(&Id::Explicit(id.clone())) else {
                    continue;
                };
                let calls: Vec<(String, i64)> = (requests[j].work.iter())
                    .filter_map(|&i| work[i].by.as_ref())
                    .filter(|(k, ..)| k == key)
                    .map(|(_, device, seq)| (device.clone(), *seq))
                    .collect();
                let mine: HashSet<Span> = (spans.iter())
                    .filter(|s| calls.iter().any(|(d, q)| holds(s, d, (*q, *q))))
                    .cloned()
                    .collect();
                owned.extend(mine.iter().cloned());
                requests[j].take(key, Owns::Prompts(mine));
            }
            unlinked += c.units(&owned);
            continue;
        }
        let id = (ids.first().cloned()).map_or_else(|| own(key), Id::Explicit);
        match at.get(&id) {
            Some(&j) => {
                requests[j].take(key, Owns::All);
                // A group's ask is the earliest of its conversations' (C6).
                if let (Id::Group(_), Some(o)) = (&id, c.original(raw.device()))
                    && o.ts < requests[j].original.ts
                {
                    requests[j].original = o;
                }
            }
            None => match c.original(raw.device()) {
                Some(o) if ids.is_empty() => {
                    at.insert(id.clone(), requests.len());
                    let mut r = Request::new(id, o);
                    r.take(key, Owns::All);
                    requests.push(r);
                }
                None if matches!(id, Id::Group(_)) => deferred.push((key, c, id)),
                _ => unlinked += c.units(&HashSet::new()),
            },
        }
    }
    for (key, c, id) in deferred {
        match at.get(&id) {
            Some(&j) => requests[j].take(key, Owns::All),
            None => unlinked += c.units(&HashSet::new()),
        }
    }
    for (i, id) in deferred_work {
        match at.get(&id) {
            // In its place in the work, newest first, from which `next` is read (#689).
            Some(&j) => {
                let work = &mut requests[j].work;
                work.insert(work.partition_point(|&x| x < i), i);
            }
            None => unlinked += 1,
        }
    }

    for r in &mut requests {
        if let Id::Group(anchor) = &r.id {
            let group = grouped.groups.get(anchor);
            r.title = group.and_then(|g| g.title.clone());
            r.group_ops = group
                .map(|g| g.ops.iter().cloned().collect())
                .unwrap_or_default();
            r.suggested = (grouped.suggested.get(anchor))
                .and_then(|picked| grouped.of.get(picked))
                .filter(|target| *target != anchor)
                .map(|target| Id::Group(target.clone()).held());
        }
    }
    Ok(Built {
        convs,
        work,
        requests,
        unlinked,
    })
}

/// A conversation as grouping reads it (docs/my-work-model.md C1, C2).
pub(crate) struct Seen {
    /// Its agent and session, as the stores hold them.
    pub key: (String, String),
    /// The repository of its first prompt that names one, as stored.
    pub repo: Option<String>,
    /// Its opening ask, when it is a prompt (claude-mem's `originalFor`): the record's device and
    /// seq, its time, and its text before any gate.
    pub ask: Option<(String, i64, i64, String)>,
    /// Its prompts that ask, the oldest first, as `ask` gives one.
    pub asks: Vec<(String, i64, i64, String)>,
    /// Its newest prompt, summary, card or work write, unix ms.
    pub last: i64,
    /// The request ids its work names.
    pub named: BTreeSet<String>,
    /// The work-state lists it wrote to, by repository and name, from every write a call of its
    /// sent (C3).
    pub lists: BTreeSet<(String, String)>,
    /// Its first cards of those My Work reads, the oldest first, each with a title: its device,
    /// its window op and its title (J2).
    pub cards: Vec<(String, i64, String)>,
}

/// Every conversation My Work reads (`gather`), as grouping reads it.
pub(crate) fn seen(raw: &Raw, k: &Connection, rules: &Rules) -> Result<Vec<Seen>> {
    let Gathered { convs, work, named } = gather(raw, k, rules)?;
    let mut lists = writers((raw, k, rules), &work)?;
    let mut out = Vec::new();
    for (key, c) in convs {
        let said = |p: &Said| {
            let text = p.text.clone()?;
            asks(&text).then(|| (p.device.clone(), p.seq, p.ts, text))
        };
        let asks: Vec<_> = c.prompts.iter().filter_map(said).collect();
        let ask = (c.original(raw.device()))
            .filter(|o| o.from == "prompt")
            .and_then(|o| {
                asks.iter()
                    .find(|a| o.id == Some(format!("{}:{}", a.0, a.1)))
            })
            .cloned();
        let wrote = (work.iter())
            .filter(|w| w.by.as_ref().is_some_and(|(by, ..)| *by == key))
            .map(|w| w.updated);
        let last = (c.prompts.iter().map(|p| p.ts))
            .chain(c.turns.iter().map(|t| t.s.ts))
            .chain(c.cards.iter().map(|card| card.c.ts))
            .chain(wrote)
            .max()
            .unwrap_or(0);
        let cards = (c.cards.iter().rev())
            .filter(|card| !card.c.title.trim().is_empty())
            .take(CONTEXT_CARDS)
            .map(|card| (card.c.device.clone(), card.c.op_seq, card.c.title.clone()))
            .collect();
        out.push(Seen {
            named: named.get(&key).cloned().unwrap_or_default(),
            lists: lists.remove(&key).unwrap_or_default(),
            cards,
            key,
            repo: c.repo,
            ask,
            asks,
            last,
        });
    }
    Ok(out)
}

/// The lists each conversation wrote to, by repository and name: every write of every item, by
/// the calls that sent it (`senders`), where `link` reads an item's newest write alone. None
/// before the worker has made `raw_docs_tools`.
fn writers(
    (raw, k, rules): (&Raw, &Connection, &Rules),
    work: &[Work],
) -> Result<HashMap<Key, BTreeSet<(String, String)>>> {
    let mut out: HashMap<Key, BTreeSet<(String, String)>> = HashMap::new();
    if !crate::consumer::manifest::exists(k, "index", "raw_docs_tools")? {
        return Ok(out);
    }
    let settings = crate::capture::Settings {
        rules: rules.clone(),
        ..crate::capture::Settings::default()
    };
    let mut near = k.prepare_cached(
        "SELECT device, seq FROM raw_docs INDEXED BY raw_docs_tools
         WHERE kind = 'tool' AND ts BETWEEN ?1 AND ?2",
    )?;
    let mut read = HashMap::new();
    for w in work {
        for write in w.writes.iter().chain(&w.list_writes) {
            for (key, ..) in senders(raw, &mut near, &mut read, (write, &settings))? {
                out.entry(key)
                    .or_default()
                    .insert((w.repo.clone(), w.list.clone()));
            }
        }
    }
    Ok(out)
}

/// Rule 6: a check of a request's version, in `state/my-work-reviews.json` under the keyed hash of
/// the request's stored id. `sessions` are its conversations' `forget::session` hashes, keyed, by
/// which a forget of one sweeps it. Nothing in the file reads without the home's key.
#[derive(serde::Serialize, serde::Deserialize)]
struct Review {
    version: String,
    at: i64,
    actor: String,
    sessions: Vec<String>,
}

/// What a request's check is filed under, beside its answer, and what its work product is read
/// from (slice 4).
pub(crate) struct Held {
    key: String,
    /// The request's id as the stores hold it (`Id::held`), where `key` is it keyed by the home's
    /// key (CodeRabbit on #746).
    id: String,
    sessions: Vec<String>,
    sources: Sources,
    /// The held id of the request the judge's open suggestion names (J4).
    suggested: Option<String>,
    /// The conversation that suggestion is about, as `sessions` shows it (its id and fingerprint),
    /// which a Join follows to where it went (#777).
    about: Option<Value>,
    /// The conversations no other request built holds, counted before a range leaves any out: a
    /// copy whose call raw no longer has joins only such a conversation's request
    /// (docs/deliverables.md, slice 2).
    sole: Vec<Key>,
}

impl Held {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

/// A request's conversations, each with the prompt spans it owns (none: all of it), its work
/// items' fields with the time of their last write, and its cards' modified files with the time
/// of their window.
#[derive(Default)]
pub(crate) struct Sources {
    convs: Vec<(Key, Option<Vec<Span>>)>,
    fields: Vec<(Map<String, Value>, i64)>,
    files: Vec<CardFiles>,
}

/// A card's modified files as the gate shows them and as the card stored them, its time, and its
/// window op by device and seq.
struct CardFiles {
    shown: Vec<String>,
    stored: Vec<String>,
    at: i64,
    op: (String, i64),
}

/// claude-mem's `mergeProducts` limit: the newest this many entries of a request's work product.
const PRODUCTS: usize = 60;
/// A request's product calls read at once, at most: a page of records' bodies in memory.
const CALLS_PAGE: usize = 100;

/// Slice 4 (parity row X16): what a request produced, as claude-mem 15.1's request drawer shows it:
/// the commits, branches and pull requests its conversations' recorded commands made, the links
/// its publishing tools returned, the files of kinds a person opens that its cards name as
/// modified, and what its work items attach. The calls are read live from raw.db, so a forgotten
/// or expired one is none; each entry is gated as the page's text is, and a link the gate changes
/// is shown as its title alone. No provider is called. With them, the request's copies
/// (docs/deliverables.md, slice 2), each with `file`, its link's tag (`tag`), which only this page
/// gets: `products_read`, which the model's progress review reads, has none. The flag says the
/// copies were not read (their store did not read): the page shows the rest and asks again.
pub(crate) fn products(home: &Path, held: &Held, tag: Tag) -> Result<(Vec<Value>, bool)> {
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok((Vec::new(), false));
    };
    // One fence for the whole answer (#439): a forget registered between two copies' checks would
    // leave the first in it.
    let _read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
    let rules = Rules::load(home)?;
    // A copies' store that does not read leaves the page without its copies, not without the rest.
    let copies = (|| match crate::deliverables::open_read(home)? {
        Some(db) => copies_of(&db, &raw, held),
        None => Ok(Vec::new()),
    })();
    let (copies, unread) = match copies {
        Ok(copies) => (copies, false),
        Err(e) => {
            eprintln!("oboete: a request's copies not read: {e:#}");
            (Vec::new(), true)
        }
    };
    let list = shown_products(home, (&raw, &k, &rules), &held.sources, &copies, tag)?;
    Ok((list.into_iter().map(|(p, _)| p).collect(), unread))
}

/// What a person opens on a request's card, at most (claude-mem's `CARD_DELIVERABLES`).
const CARD: usize = 3;

/// A request's card (docs/deliverables.md, slice 2b): of its products as its page shows them, the
/// first `CARD` that are not history, as claude-mem's `isCardDeliverable` takes them (commits and
/// branches stay on the page), and how many there are; none when there are none.
fn card(list: Vec<(Value, bool)>) -> Option<(Vec<Value>, usize)> {
    let mut items: Vec<Value> = (list.into_iter())
        .filter(|(_, history)| !history)
        .map(|(p, _)| p)
        .collect();
    let count = items.len();
    items.truncate(CARD);
    (count > 0).then_some((items, count))
}

/// A request's products as its page shows them, each with whether it is history (a commit or a
/// branch): what its `sources` recorded, gated, and its `copies` that are on disk (claude-mem's
/// mapper leaves out one that is not), each with its link's tag, newest first within
/// claude-mem's 60. A copy hides the recorded file of the path it covers (`covers`), as
/// claude-mem's `copyCoversSource` does: its own path compared with the path as recorded, before
/// the page's gate (#840, `Ungated::recorded`), never one that holds the mask, which a gate made
/// when the record was captured or summarized.
fn shown_products(
    home: &Path,
    (raw, k, rules): (&Raw, &Connection, &Rules),
    sources: &Sources,
    copies: &[crate::deliverables::Row],
    tag: Tag,
) -> Result<Vec<(Value, bool)>> {
    let mut list = products_of(raw, k, rules, sources)?;
    let copies: Vec<&crate::deliverables::Row> = (copies.iter())
        .filter(|c| crate::deliverables::on_disk(home, c))
        .collect();
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    for c in &copies {
        if !c.source_path.contains(crate::redact::MASK) && covers(c) {
            list.retain(|(p, _, u)| {
                p["url"].is_string() || u.recorded.as_deref() != Some(c.source_path.as_str())
            });
        }
    }
    let list = (list.into_iter()).map(|(p, _, u)| (p, u.history));
    let mut shown = (copies.iter())
        .map(|c| {
            let p = json!({
                "kind": c.kind,
                "title": gate(&c.title),
                "url": null,
                "path": gate(&c.source_path),
                "at": c.copied,
                "file": tag(c)?,
            });
            Ok((p, false))
        })
        .collect::<Result<Vec<(Value, bool)>>>()?;
    shown.extend(list);
    // Newest first within claude-mem's 60, a copy before an entry of the same time.
    shown.sort_by_key(|(p, _)| std::cmp::Reverse(p["at"].as_i64().unwrap_or(0)));
    shown.truncate(PRODUCTS);
    Ok(shown)
}

/// Whether a copy covers its source as claude-mem's `copyCoversSource` reads it: the source is
/// gone, or was not changed after the copy was made.
fn covers(c: &crate::deliverables::Row) -> bool {
    let modified = std::fs::metadata(&c.source_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    modified.is_none_or(|ms| ms <= c.copied)
}

/// A request's copies (docs/deliverables.md, slice 2), by id: those of the conversations it holds
/// whole, and of those it holds prompts of, the copies whose call a held prompt span holds while
/// raw still has that call (the record at its seq has its time). A copy whose call raw no longer
/// has (a restore gave its seq to another record, or it was removed) joins a request only as its
/// conversation's one holder among all requests built (`Held::sole`), as claude-mem attaches a
/// copy only to a conversation's single owner. A copy a forget denies (its call or its turn's end)
/// is none from the forget's registration on, before the sweep removes it.
fn copies_of(db: &Connection, raw: &Raw, held: &Held) -> Result<Vec<crate::deliverables::Row>> {
    let mut out = Vec::new();
    for (key, spans) in &held.sources.convs {
        for row in crate::deliverables::rows_of(db, &key.0, &key.1)? {
            let ours = match spans {
                None => true,
                Some(spans) => match call_kept(raw, &row)? {
                    true => {
                        (spans.iter()).any(|s| holds(s, &row.device, (row.call_seq, row.call_seq)))
                    }
                    false => held.sole.contains(key),
                },
            };
            if ours && !raw.denies(&row)? {
                out.push(row);
            }
        }
    }
    out.sort_by_key(|r| r.id);
    Ok(out)
}

/// Whether raw still has a copy's call: the record at its seq is an event, live or expired, of
/// its time.
fn call_kept(raw: &Raw, row: &crate::deliverables::Row) -> Result<bool> {
    let at = raw
        .after(&row.device, row.call_seq - 1, 1)?
        .into_iter()
        .next();
    Ok(match at.filter(|r| r.seq == row.call_seq).map(|r| r.item) {
        Some(crate::raw::Item::Event(e) | crate::raw::Item::Expired(e)) => e.ts == row.call_ts,
        _ => false,
    })
}

/// Where a product was read from (docs/my-work-model.md S5): a call's record, a card's window op,
/// each by device and seq, or a work item, which no forget takes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Origin {
    Record(String, i64),
    Card(String, i64),
    Work,
}

/// `products` of a request's `sources` from the stores open, each with where it was read from: a
/// progress review reads them (docs/my-work-model.md P3) and lists those among what it was shown.
/// Gated as the page shows them, with no copy.
pub(crate) fn products_read(
    raw: &Raw,
    k: &Connection,
    rules: &Rules,
    sources: &Sources,
) -> Result<Vec<(Value, Origin)>> {
    Ok((products_of(raw, k, rules, sources)?.into_iter())
        .map(|(p, origin, _)| (p, origin))
        .collect())
}

/// `products_read`, each with what was read of it before any gate (`Ungated`).
fn products_of(
    raw: &Raw,
    k: &Connection,
    rules: &Rules,
    sources: &Sources,
) -> Result<Vec<(Value, Origin, Ungated)>> {
    let mut lists = Vec::new();
    // The consumer makes its table; before its first step there is nothing to read.
    if crate::consumer::manifest::exists(k, "table", "product_calls")? {
        let mut calls = k.prepare(
            "SELECT device, seq FROM product_calls WHERE agent = ?1 AND session = ?2
             ORDER BY device, seq",
        )?;
        for ((agent, session), owned) in &sources.convs {
            let rows = (calls
                .query_map(params![agent, session], |r| Ok((r.get(0)?, r.get(1)?)))?)
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;
            let mut held: BTreeMap<String, Vec<i64>> = BTreeMap::new();
            for (device, seq) in rows {
                let ours = |spans: &Vec<Span>| spans.iter().any(|s| holds(s, &device, (seq, seq)));
                if owned.as_ref().is_none_or(ours) {
                    held.entry(device).or_default().push(seq);
                }
            }
            // Read a device's calls a page at a time (`Raw::at`), as the list reads every
            // request's: one call at a time was two queries each (slice 2b's measurement).
            for (device, seqs) in held {
                for page in seqs.chunks(CALLS_PAGE) {
                    for r in raw.at(&device, page)? {
                        let Item::Event(e) = r.item else { continue };
                        let Ok(body) = serde_json::from_str::<Value>(&e.body) else {
                            continue;
                        };
                        let source = crate::work_product::Source {
                            kind: "record".into(),
                            id: format!("{device}:{}", r.seq),
                            index: None,
                        };
                        let tool = body["tool"].as_str().unwrap_or("");
                        let mut list = crate::work_product::from_tool_use(
                            tool,
                            &body["input"],
                            &body["output"],
                            Some(e.ts),
                            Some(source),
                            body["failed"] == true,
                        );
                        for p in &mut list {
                            p.checkout.clone_from(&e.repo);
                        }
                        lists.push(list);
                    }
                }
            }
        }
    }
    for (fields, at) in &sources.fields {
        // What a work item attaches, as work state's own lines show it beside its key, since a
        // rule may match a value only there; a link the gate changes is its title alone
        // (CodeRabbit on #650).
        let mut changed = Vec::new();
        let shown = (fields.iter())
            .filter(|(k, _)| crate::work_product::is_work_product_field(k))
            .map(|(k, v)| {
                let text = crate::work_state::text(v);
                let gated = crate::work_state::beside(k, &text, rules);
                if gated == text {
                    (k.clone(), v.clone())
                } else {
                    changed.push(k.clone());
                    (k.clone(), Value::String(gated))
                }
            })
            .collect();
        let mut list = crate::work_product::from_fields(&shown, Some(*at), None);
        for p in &mut list {
            // A field the gate changed was read as gated: its path is not known as recorded.
            if p.field.as_ref().is_some_and(|f| changed.contains(f)) {
                p.url = None;
            } else {
                p.recorded.clone_from(&p.path);
            }
        }
        lists.push(list);
    }
    for f in &sources.files {
        let card = crate::work_product::Source {
            kind: "card".into(),
            id: format!("{}:{}", f.op.0, f.op.1),
            index: None,
        };
        let mut list = crate::work_product::from_files(&f.shown, Some(f.at), Some(card));
        // Each shown as the gate shows the card's file, recorded as the card stored it.
        for p in &mut list {
            let i = p.source.as_ref().and_then(|s| s.index);
            p.recorded = i.and_then(|i| f.stored.get(i)).cloned();
        }
        lists.push(list);
    }
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    Ok((crate::work_product::merge(lists, PRODUCTS).into_iter())
        .map(|p| {
            let recorded = p.recorded;
            let url = p.url.filter(|url| gate(url) == *url);
            let origin = (p.source.as_ref())
                .and_then(|s| {
                    let (device, seq) = s.id.rsplit_once(':')?;
                    let (device, seq) = (device.to_owned(), seq.parse().ok()?);
                    match s.kind.as_str() {
                        "record" => Some(Origin::Record(device, seq)),
                        "card" => Some(Origin::Card(device, seq)),
                        _ => None,
                    }
                })
                .unwrap_or(Origin::Work);
            let shown = json!({
                "kind": gate(&p.kind),
                "title": gate(&p.title),
                "url": url,
                "path": p.path.as_deref().map(gate),
                "at": p.at,
            });
            let ungated = Ungated {
                history: matches!(p.kind.as_str(), "commit" | "branch"),
                recorded,
            };
            (shown, origin, ungated)
        })
        .collect())
}

/// What `products_of` knows of a product before any gate, which the page's own list uses and the
/// model's review input never gets.
struct Ungated {
    /// A commit or a branch, which a card leaves to the page (`card`).
    history: bool,
    /// Its path as it was recorded, before the page's gate, which a copy's own path is compared
    /// with (#840): a card's file as the card stored it, a work item's where the gate left its
    /// field as it was; none for a link (a tool record makes only links), or a field the gate
    /// changed, which was read as the gate shows it.
    recorded: Option<String>,
}

const REVIEWS: &str = "my-work-reviews.json";

/// The checks; none when the file is not there or does not read: it is the page's, and no answer
/// waits on it, as claude-mem's sidecar is optional. A write replaces a file that does not read.
fn reviews(home: &Path) -> BTreeMap<String, Review> {
    (std::fs::read(home.join("state").join(REVIEWS)).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The hold on `state/my-work-reviews.lock`, from the read a check is tested against to its write,
/// so that two checks, or a check and a forget's sweep, do not come between each other: a check of
/// an older version does not replace a newer one, nor a stale one bring back what a forget took
/// (Codex on slice 3).
pub(crate) struct ReviewsLock(#[allow(dead_code)] std::fs::File);

/// The reviews' lock as a forget's sweep takes it: waited for as long as it is held.
pub(crate) fn reviews_lock(home: &Path) -> Result<ReviewsLock> {
    let lock = state_file(home, "my-work-reviews.lock")?;
    lock.lock()?;
    Ok(ReviewsLock(lock))
}

/// The reviews' lock as a check or its Undo takes it on a viewer's connection: waited for `wait`
/// at most, none past it, and by `WAITERS` at most at once, as Mark finished's.
pub(crate) fn reviews_lock_within(
    home: &Path,
    wait: std::time::Duration,
) -> Result<Option<ReviewsLock>> {
    static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    Ok(waited(home, "my-work-reviews.lock", wait, &WAITING)?.map(ReviewsLock))
}

/// Slice 2 of docs/subagents.md: Mark finished's own hold, taken before anything else, so two at
/// once append one stop. No other path takes it: it waits on nothing a forget's purge holds, as
/// the reviews' lock would (Codex on slice 2). Waited for `wait` at most, none past it, and by
/// `WAITERS` at most at once, so requests do not keep the viewer's connections waiting
/// (CodeRabbit on #850).
pub(crate) fn finished_lock(
    home: &Path,
    wait: std::time::Duration,
) -> Result<Option<std::fs::File>> {
    static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    waited(home, "my-work-finished.lock", wait, &WAITING)
}

/// `state/<name>` locked, waited for `wait` at most and by `WAITERS` at most at once (`waiting`
/// counts them): none past either.
fn waited(
    home: &Path,
    name: &str,
    wait: std::time::Duration,
    waiting: &'static std::sync::atomic::AtomicUsize,
) -> Result<Option<std::fs::File>> {
    use std::sync::atomic::Ordering;
    struct Waiting(&'static std::sync::atomic::AtomicUsize);
    impl Drop for Waiting {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    if waiting.fetch_add(1, Ordering::SeqCst) >= WAITERS {
        waiting.fetch_sub(1, Ordering::SeqCst);
        return Ok(None);
    }
    let _waiting = Waiting(waiting);
    let lock = state_file(home, name)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(Some(lock)),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                #[cfg(test)]
                if let Some(blocked) = FINISHED_BLOCKED.with(|hook| hook.borrow_mut().take()) {
                    blocked();
                }
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                std::thread::sleep(std::time::Duration::from_millis(20).min(left));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// How many requests may hold or wait for Mark finished's lock at once, and how many the
/// reviews', well under the viewer's 32 connections; one more is answered at once (CodeRabbit on
/// #850).
const WAITERS: usize = 4;

#[cfg(test)]
thread_local! {
    /// One-shot for this thread's next wait for Mark finished's lock or the reviews': a test's sign
    /// that it waits.
    pub(crate) static FINISHED_BLOCKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// `state/<name>`, opened to be locked.
fn state_file(home: &Path, name: &str) -> Result<std::fs::File> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    Ok(options.open(state.join(name))?)
}

/// The checks as `change` leaves them, written whole when it says they changed, under the hold.
fn change_reviews(
    home: &Path,
    _held: &ReviewsLock,
    change: impl FnOnce(&mut BTreeMap<String, Review>) -> bool,
) -> Result<()> {
    let mut all = reviews(home);
    if change(&mut all) {
        let state = home.join("state");
        crate::backup::write_synced(&state, REVIEWS, &serde_json::to_vec(&all)?)?;
    }
    Ok(())
}

/// Rule 6: a check of `version` filed for `held`, or its Undo, with the checks of the requests no
/// longer read (`current`) dropped, so that a request a forget took leaves nothing here. A check
/// of a session an exclusion hides now stays: the exclusion can be lifted, and the request is read
/// again.
pub(crate) fn set_review(
    (home, lock): (&Path, &ReviewsLock),
    (held, current): (&Held, &[Held]),
    version: &str,
    checked: bool,
    now: i64,
) -> Result<()> {
    let read: HashSet<&str> = current.iter().map(|h| h.key.as_str()).collect();
    let hidden = excluded_sessions(home)?;
    change_reviews(home, lock, |all| {
        all.retain(|key, r| {
            read.contains(key.as_str()) || r.sessions.iter().any(|s| hidden.contains(s))
        });
        if checked {
            let review = Review {
                version: version.to_owned(),
                at: now,
                actor: "local-user".to_owned(),
                sessions: held.sessions.clone(),
            };
            all.insert(held.key.clone(), review);
        } else if all.get(&held.key).is_some_and(|r| r.version == version) {
            all.remove(&held.key);
        }
        true
    })
}

/// The sessions of the repositories excluded now, as a check names its sessions (`Held::sessions`).
fn excluded_sessions(home: &Path) -> Result<HashSet<String>> {
    if !crate::raw::exists(home) {
        return Ok(HashSet::new());
    }
    let raw = crate::raw::open(home)?;
    let mut key = None;
    (raw.sessions_in(&raw.exclusions_now()?)?.iter())
        .filter_map(|s| s.split_once('\0'))
        .map(|(agent, session)| keyed(&crate::forget::session(agent, session), home, &mut key))
        .collect()
}

/// Rule 6: a forget's sweep at its step 5: the checks of the forgotten `sessions` (each target's
/// `forget::session` hash) go. A home with no file of checks has none to sweep.
pub(crate) fn forget_reviews(home: &Path, sessions: &[String]) -> Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }
    // Held before the file is looked for: a check whose hold began first writes it before this.
    let lock = reviews_lock(home)?;
    if !home.join("state").join(REVIEWS).exists() {
        return Ok(());
    }
    let mut key = None;
    let gone = (sessions.iter())
        .map(|s| keyed(s, home, &mut key))
        .collect::<Result<HashSet<String>>>()?;
    change_reviews(home, &lock, |all| {
        let before = all.len();
        all.retain(|_, r| !r.sessions.iter().any(|s| gone.contains(s)));
        all.len() != before
    })
}

/// The home's key of the fingerprints of masked ids and of the versions checks are of (`answer`):
/// 32 random bytes in `state/my-work.key`, made the first time a request is read and never in an
/// answer, so that neither tells anything of a guess at what it stands for (CodeRabbit on #574).
/// Whoever can read the file can read the stores, which hold the ids themselves. It is the owner's
/// alone to read (0600), and on disk before it is published, as the checks it keys are.
fn mask_key(home: &Path) -> Result<Vec<u8>> {
    use std::io::Write;
    let state = home.join("state");
    let path = state.join("my-work.key");
    // None for no file, or one that holds no key; a file that does not read is an error, not a
    // reason to make a new key over the one every check is filed under.
    let read = || -> Result<Option<Vec<u8>>> {
        match std::fs::read(&path) {
            Ok(key) => Ok(Some(key).filter(|key| key.len() == 32)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    };
    if let Some(key) = read()? {
        return Ok(key);
    }
    let (mut key, mut tag) = ([0u8; 32], [0u8; 8]);
    getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("random key: {e}"))?;
    getrandom::fill(&mut tag).map_err(|e| anyhow::anyhow!("random key: {e}"))?;
    std::fs::create_dir_all(&state)?;
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = state.join(format!("my-work.key.{tag}"));
    let published = (|| -> Result<Vec<u8>> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(&key)?;
        file.sync_all()?;
        // Published whole: a link fails on a name another read took first, whose key stays.
        let mine = if std::fs::hard_link(&tmp, &path).is_ok() {
            key.to_vec()
        } else if let Some(theirs) = read()? {
            theirs
        } else {
            // No links on this filesystem, or a file of the name that holds no key: this one's.
            // shortcut: two first reads on a filesystem without links can both rename, the later
            // key standing; publish with a rename that replaces nothing if such a home loses checks.
            std::fs::rename(&tmp, &path)?;
            key.to_vec()
        };
        #[cfg(unix)]
        std::fs::File::open(&state)?.sync_all()?;
        Ok(mine)
    })();
    // The temporary name goes, whether the key was published under the other or not.
    let _ = std::fs::remove_file(&tmp);
    published
}

/// A request's id as the answer shows it, and the fingerprint of the stored one when the gate
/// masks it: what a request of a conversation is known by, and its conversation in any request.
fn shown_id(
    id: &Id,
    rules: &Rules,
    home: &Path,
    key: &mut Option<Vec<u8>>,
) -> Result<(String, Option<String>)> {
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    let (shown, kind) = match id {
        // C6: `g:` and the keyed hash of its anchor, which tells nothing of the conversation; the
        // hash is its fingerprint too, so that no rule's mask merges two groups at the route
        // (Codex on #670).
        Id::Group(_) => {
            let print = keyed_print(&id.held(), home, key)?;
            return Ok((format!("g:{print}"), Some(print)));
        }
        Id::Conversation((agent, session)) => (format!("c:{}:{}", gate(agent), gate(session)), "c"),
        Id::Explicit(e) => (
            format!("r:{}", crate::work_state::beside("request_id", e, rules)),
            "r",
        ),
    };
    let held = id.held();
    // The stored id through the gate as assembled too, since a rule may match it and neither of
    // its parts, which the route's own gate (`view::answered`) would then mask alike in two ids
    // (Codex on #574), or a part's mask may hide what that rule would match: then nothing of it is
    // shown but its kind (Codex and CodeRabbit on #575).
    let shown = if gate(&held) == held {
        shown
    } else {
        format!("{kind}:{}", crate::redact::MASK)
    };
    // An id the gate masks takes a fingerprint of the stored one, so that two masked alike stay
    // two and each keeps its id from read to read (Codex on #574), keyed by the home's own key so
    // that it tells nothing of a guess at the id (CodeRabbit on #574). It is named in a field of
    // its own too, which the route takes out, so that no id's own text passes for one there
    // (Codex and Sourcery on #576).
    if shown == held {
        return Ok((shown, None));
    }
    let print = keyed_print(&held, home, key)?;
    Ok((format!("{shown}#{print}"), Some(print)))
}

/// A stored text hashed with the home's own key, made the first time one is needed: the same from
/// read to read, and nothing of a guess at the text.
fn keyed(held: &str, home: &Path, key: &mut Option<Vec<u8>>) -> Result<String> {
    let key = match key {
        Some(key) => key,
        None => key.insert(mask_key(home)?),
    };
    let keyed = [key.as_slice(), held.as_bytes()].concat();
    Ok(crate::forget::hash(&keyed))
}

/// `keyed`'s first 12 hex digits: the fingerprint of a stored text the gate masks.
fn keyed_print(held: &str, home: &Path, key: &mut Option<Vec<u8>>) -> Result<String> {
    Ok(keyed(held, home, key)?[..12].to_owned())
}

/// One request as the route gives it, every text through the gate; none when its newest update is
/// before `since`, which then gates nothing.
/// A request's records as My Work reads them: its work items; the summaries, cards and prompts of
/// the conversations it owns (of one it owns some prompts of, those prompts' parts); the prompts'
/// times that count as its activity; its repositories and agents as the gate shows them; each
/// conversation with its newest record; and its newest prompt. The page's answer and the progress
/// review read them.
struct Parts<'a> {
    items: Vec<&'a Work>,
    turns: Vec<&'a Turn>,
    cards: Vec<&'a Shown>,
    prompts: Vec<&'a Said>,
    active: Vec<i64>,
    repos: BTreeSet<String>,
    agents: BTreeSet<String>,
    sessions: Vec<(Option<i64>, &'a Key)>,
    latest_prompt: Option<&'a Said>,
}

fn parts<'a>(
    r: &'a Request,
    convs: &'a BTreeMap<Key, Conversation>,
    work: &'a [Work],
    rules: &Rules,
) -> Parts<'a> {
    let items: Vec<&Work> = r.work.iter().map(|&i| &work[i]).collect();
    let mut turns: Vec<&Turn> = Vec::new();
    let mut cards: Vec<&Shown> = Vec::new();
    let mut prompts: Vec<&Said> = Vec::new();
    // A request that owns some replies of a conversation shares its prompts but not their time.
    let mut active: Vec<i64> = Vec::new();
    let mut repos = BTreeSet::new();
    let mut agents = BTreeSet::new();
    // Each conversation it owns, with its newest record of the ones `updated` counts.
    let mut sessions: Vec<(Option<i64>, &Key)> = Vec::new();
    let mut latest_prompt: Option<&Said> = None;
    for (key, own) in &r.owns {
        let Some(c) = convs.get(key) else { continue };
        agents.insert(crate::redact::outbound_with(&key.0, rules));
        repos.extend(c.repos.iter().cloned());
        prompts.extend(c.prompts.iter());
        let (from_turn, from_card, from_active) = (turns.len(), cards.len(), active.len());
        match own {
            Owns::All => {
                turns.extend(c.turns.iter());
                cards.extend(c.cards.iter());
                active.extend(c.prompts.iter().map(|p| p.ts));
                if let Some(p) = c.prompts.last()
                    && latest_prompt.is_none_or(|l| p.ts > l.ts)
                {
                    latest_prompt = Some(p);
                }
            }
            Owns::Prompts(owned) => {
                let ours = |device: &str, span| owned.iter().any(|s| holds(s, device, span));
                // A reply is its prompt's, by the record it ends with: the records between a reply
                // and the next prompt are the next reply's too.
                turns.extend((c.turns.iter()).filter(|t| ours(&t.s.device, (t.span.1, t.span.1))));
                cards.extend((c.cards.iter()).filter(|card| ours(&card.c.device, card.span)));
                // Its own newest prompt tells whether its summary is behind (Codex on #574).
                if let Some(p) = (c.prompts.iter()).rfind(|p| ours(&p.device, (p.seq, p.seq)))
                    && latest_prompt.is_none_or(|l| p.ts > l.ts)
                {
                    latest_prompt = Some(p);
                }
            }
        }
        let wrote = (items.iter())
            .filter(|w| w.by.as_ref().is_some_and(|(by, ..)| by == key))
            .map(|w| w.updated);
        let latest = (turns[from_turn..].iter().map(|t| t.s.ts))
            .chain(cards[from_card..].iter().map(|c| c.c.ts))
            .chain(active[from_active..].iter().copied())
            .chain(wrote)
            .max();
        sessions.push((latest, key));
    }
    // The newest first, as the requests are; of one time, in the order the stores hold them
    // (`read` takes them in `convs`' order).
    sessions.sort_by_key(|s| std::cmp::Reverse(s.0));
    for w in &items {
        repos.insert(crate::redact::outbound_with(&w.repo, rules));
    }
    turns.sort_by_key(|t| std::cmp::Reverse(t.s.ts));
    cards.sort_by_key(|c| std::cmp::Reverse(c.c.ts));
    prompts.sort_by_key(|p| p.ts);
    Parts {
        items,
        turns,
        cards,
        prompts,
        active,
        repos,
        agents,
        sessions,
        latest_prompt,
    }
}

/// A request's sources of work product (`products`): its conversations, each with the prompt spans
/// it owns (none: all of it), its work items' fields with the time of their last write, and its
/// cards' modified files with the time of their window.
fn sources(r: &Request, items: &[&Work], cards: &[&Shown]) -> Sources {
    Sources {
        convs: (r.owns.iter())
            .map(|(key, own)| {
                let owned = match own {
                    Owns::All => None,
                    Owns::Prompts(spans) => Some(spans.iter().cloned().collect()),
                };
                (key.clone(), owned)
            })
            .collect(),
        // A list's own fields attach to it too, as claude-mem reads each entry's fields.
        fields: (items.iter())
            .flat_map(|w| {
                [
                    (w.fields.clone(), w.updated),
                    (w.list_fields.clone(), w.updated),
                ]
            })
            .filter(|(fields, _)| !fields.is_empty())
            .collect(),
        files: (cards.iter())
            .filter(|c| !c.c.files_modified.is_empty())
            .map(|c| CardFiles {
                shown: c.c.files_modified.clone(),
                stored: c.files.clone(),
                at: c.c.ts,
                op: (c.c.device.clone(), c.c.op_seq),
            })
            .collect(),
    }
}

fn answer(
    r: &Request,
    (convs, work, reviews): (
        &BTreeMap<Key, Conversation>,
        &[Work],
        &BTreeMap<String, Review>,
    ),
    (raw, k, rules, hidden): (&Raw, &Connection, &Rules, &HashSet<(String, i64)>),
    (home, key): (&Path, &mut Option<Vec<u8>>),
    (since, now): (i64, i64),
) -> Result<Option<(Value, Held)>> {
    let Parts {
        items,
        turns,
        cards,
        prompts,
        active,
        repos,
        agents,
        sessions,
        latest_prompt,
    } = parts(r, convs, work, rules);
    let original = &r.original;
    let follow_ups: Vec<Value> = {
        let later: Vec<&&Said> = (prompts.iter())
            .filter(|p| {
                p.text.is_some() && p.ts >= original.ts && original.id.as_ref() != Some(&p.id)
            })
            .collect();
        let from = later.len().saturating_sub(FOLLOW_UPS);
        (later[from..].iter())
            .map(|p| {
                let text = p.text.as_deref().map(|t| shown(t, FOLLOW_UP, rules));
                json!({"id": p.id, "ts": p.ts, "text": text})
            })
            .collect()
    };
    let activity = (turns.iter().map(|t| t.s.ts))
        .chain(cards.iter().map(|c| c.c.ts))
        .chain(active)
        .max();
    let updated = (items.iter().map(|w| w.updated))
        .chain(activity)
        .max()
        .unwrap_or(0);
    if updated < since {
        return Ok(None);
    }
    // P5: the readings, and the newest of them when no status was recorded after it
    // (claude-mem's `reviewIsNewerThanLifecycle`).
    let readings = crate::my_work_progress::readings(k, &r.id.held(), raw.device(), hidden)?;
    let last = lifecycle(&items);
    let at = last.as_ref().map(|l| (l.0).0);
    let counts = (readings.first()).filter(|x| at.is_none_or(|at| x.ts >= at));
    // claude-mem's `completedAtEpoch`: when the newest status recorded completes the request.
    let completed_at = (last.filter(|l| {
        ["done", "completed", "complete", "shipped", "delivered"].contains(&l.2.as_str())
    }))
    .map(|l| (l.0).0);
    // The review the card and the story show: the one that counts, else the newest below 100, a
    // status recorded after it notwithstanding (claude-mem's `progress`).
    let review = counts.or(readings.first().filter(|x| x.percent < 100));
    let status = status(&items, activity, now, counts);
    let recorded = (r.title.as_deref())
        .map(|t| crate::redact::outbound_with(t, rules))
        .or_else(|| (r.explicit_text.as_deref()).map(|t| title(&shown(t, ASK, rules))));
    let ask = shown(&original.text, ASK, rules);
    // Each work item's names as capture keeps them, beside their keys (Codex on #574), and its
    // detail, claude-mem's recorded change of it: its outcome, else its progress.
    let named: Vec<(String, Option<String>, Option<String>)> = (items.iter())
        .map(|w| {
            let list = crate::work_state::beside("list", &w.list, rules);
            let task = (w.task.as_deref()).map(|t| crate::work_state::beside("task", t, rules));
            let detail = (w.text(OUTCOME_KEYS, SUMMARY, rules))
                .or_else(|| w.text(PROGRESS_KEYS, SUMMARY, rules));
            (list, task, detail)
        })
        .collect();
    // claude-mem's `updates` (`presentWork`): each item's detail, else its name, and each card's
    // subtitle, else its title, the newest first, each text once.
    let mut updates: Vec<(i64, String)> = (items.iter().zip(&named))
        .filter_map(|(w, (list, task, detail))| {
            let name = || {
                meaningful(&json!(w.task.as_deref().unwrap_or(&w.list)))?;
                Some(cut(task.as_ref().unwrap_or(list), SUMMARY))
            };
            Some((w.updated, detail.clone().or_else(name)?))
        })
        .collect();
    updates.extend(cards.iter().filter_map(|c| {
        let text = [&c.c.subtitle, &c.c.title]
            .into_iter()
            .find(|t| !t.trim().is_empty())?;
        Some((c.c.ts, cut(text, SUMMARY)))
    }));
    updates.sort_by_key(|u| std::cmp::Reverse(u.0));
    let mut seen = HashSet::new();
    updates.retain(|u| seen.insert(u.1.clone()));
    updates.truncate(UPDATES);
    let (copy, title, summary, next, updating) = if turns.is_empty() {
        let summary = (updates.first().map(|u| u.1.clone())).unwrap_or_else(|| cut(&ask, SUMMARY));
        let next = items.iter().find_map(|w| w.text(NEXT_KEYS, NEXT, rules));
        let title = recorded.unwrap_or_else(|| title(&ask));
        ("recorded", title, Some(summary), next, false)
    } else {
        let field = |names: &[&str]| {
            turns.iter().find_map(|t| {
                (names.iter()).find_map(|n| t.s.fields.get(*n).filter(|v| !v.trim().is_empty()))
            })
        };
        let summary = field(&["completed", "learned", "investigated"]).map(|t| cut(t, SUMMARY));
        let next = field(&["next_steps"]).map(|t| cut(t, NEXT));
        let first = (turns.iter().rev())
            .find_map(|t| t.s.fields.get("request").filter(|v| !v.trim().is_empty()));
        let title = recorded
            .or_else(|| first.map(|t| title(t)))
            .unwrap_or_else(|| title(&ask));
        let updating = match latest_prompt {
            Some(p) => !covered(raw, k, p)?,
            None => false,
        };
        ("summary", title, summary, next, updating)
    };
    let names: Vec<String> = (repos.iter())
        .map(|repo| crate::consumer::manifest::repo_name(repo, rules))
        .collect();
    let ask = json!({"id": original.id, "ts": original.ts, "text": ask,
        "from": original.from});
    // Each name and value as the gate shows it beside its key, as work state's own lines do, a
    // closing status too: capture keeps it as sent, and the line that shows it gates it as any
    // (`capture::work_state`; Codex on #574). Its repository as `repos` names it, so that the page
    // keeps lists of one name in two repositories apart, as claude-mem's work streams are a
    // project's list (`listId`); where the gate masks either, a fingerprint of the stored pair
    // keeps two masked alike apart (`stream`; CodeRabbit on #597).
    let mut work = Vec::new();
    for (w, (list, task, detail)) in items.iter().zip(named) {
        let status = (w.fields.get("status"))
            .map(|s| crate::work_state::beside("status", &crate::work_state::text(s), rules));
        let repo = crate::redact::outbound_with(&w.repo, rules);
        let stream = if repo == w.repo && list == w.list {
            None
        } else {
            let pair = serde_json::to_string(&[&w.repo, &w.list])?;
            Some(keyed_print(&pair, home, key)?)
        };
        let mut item = json!({"list": list, "repo": repo, "task": task, "status": status,
            "detail": detail, "updated": w.updated});
        if let Some(stream) = stream {
            item["stream"] = json!(stream);
        }
        work.push(item);
    }
    let (id, fingerprint) = shown_id(&r.id, rules, home, key)?;
    // Each conversation by the id its own request has or would have, so that the page shows one
    // card a conversation, as claude-mem's agent view does (decision 55).
    let mut conversations = Vec::new();
    for (_, (agent, session)) in &sessions {
        let conversation = Id::Conversation((agent.clone(), session.clone()));
        let (id, fingerprint) = shown_id(&conversation, rules, home, key)?;
        let agent = crate::redact::outbound_with(agent, rules);
        conversations.push(json!({"agent": agent, "id": id, "fingerprint": fingerprint}));
    }
    let updates: Vec<Value> = (updates.iter())
        .map(|(ts, text)| json!({"ts": ts, "text": text}))
        .collect();
    // Rule 6: the version a check is of: what was recorded for the request, not what a curator
    // wrote of it (claude-mem's `contentFingerprint` leaves out its generated copy), keyed so that
    // it confirms no guess at a value the gate masks, as a masked id's fingerprint (`shown_id`).
    let said: Vec<&str> = (prompts.iter())
        .filter(|p| p.text.is_some() && p.ts >= original.ts && original.id.as_ref() != Some(&p.id))
        .map(|p| p.id.as_str())
        .collect();
    let held = r.id.held();
    let recorded = json!({
        "id": held,
        "original": [&original.id, original.from, &original.text],
        "said": said,
        "owns": r.owns.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        // With its writes' times, its list's too, as claude-mem's item carries its latest entry: a
        // status written again is a new version, though its value is not (Codex on slice 3).
        "work": (items.iter()).map(|w| {
            let at = |writes: &[WorkWrite]| writes.iter().map(|x| x.ts).collect::<Vec<_>>();
            json!([w.repo, w.list, w.task, w.fields, w.list_fields, at(&w.writes), at(&w.list_writes)])
        }).collect::<Vec<_>>(),
        "cards": cards.iter().map(|c| c.c.id(raw.device())).collect::<Vec<_>>(),
    });
    let version = keyed(&recorded.to_string(), home, key)?;
    let filed = keyed(&held, home, key)?;
    let checked = (reviews.get(&filed))
        .filter(|review| review.version == version)
        .map(|review| review.at);
    let swept_by = (r.owns.iter())
        .map(|((agent, session), _)| keyed(&crate::forget::session(agent, session), home, key))
        .collect::<Result<Vec<_>>>()?;
    let sources = sources(r, &items, &cards);
    let about = match (&r.id, &r.suggested) {
        (Id::Group(anchor), Some(_)) => {
            let (id, fingerprint) = shown_id(&Id::Conversation(anchor.clone()), rules, home, key)?;
            Some(json!({"id": id, "fingerprint": fingerprint}))
        }
        _ => None,
    };
    let held = Held {
        key: filed,
        id: held,
        sessions: swept_by,
        sources,
        suggested: r.suggested.clone(),
        about,
        sole: Vec::new(),
    };
    Ok(Some((
        json!({
            "id": id,
            "fingerprint": fingerprint,
            "version": version,
            "checked": checked,
            "title": title,
            "summary": summary,
            "next": next,
            "copy": copy,
            "updating": updating,
            "status": status,
            "updated": updated,
            "started": original.ts,
            "agents": agents,
            "sessions": conversations,
            "projects": names,
            "repos": repos,
            "ask": ask,
            "follow_ups": follow_ups,
            "updates": updates,
            "work": work,
            "progress": progress(&readings, review, (status, completed_at), rules),
            "story": story(review, rules),
            "summaries": turns.iter().map(|t| t.s.id(raw.device())).collect::<Vec<_>>(),
            "cards": cards.iter().take(ACTIVITY).map(|c| c.c.id(raw.device())).collect::<Vec<_>>(),
        }),
        held,
    )))
}

/// claude-mem's `statusFor` over a request's work and its records (an observation's row, here the
/// newest of its prompts, replies and cards), then its lifecycle (`buildModel`), as the page names
/// them: needs, blocked, ready, quiet, working, todo, unknown, closed.
fn status(
    items: &[&Work],
    activity: Option<i64>,
    now: i64,
    review: Option<&crate::my_work_progress::Reviewed>,
) -> &'static str {
    let states: Vec<String> = items.iter().map(|w| w.status()).collect();
    let all = |s: &[&str]| {
        activity.is_none() && !items.is_empty() && states.iter().all(|x| s.contains(&x.as_str()))
    };
    let updated = (items.iter().map(|w| w.updated))
        .chain(activity)
        .max()
        .unwrap_or(0);
    let from_work = if states.iter().any(|s| s == "needs-you") {
        "needs-you"
    } else if states.iter().any(|s| s == "needs-review") {
        "needs-review"
    } else if states.iter().any(|s| s == "blocked" || s == "failed") {
        "blocked"
    } else if all(&["done", "completed"]) {
        "done"
    } else if all(&["dropped"]) {
        "closed"
    } else if now - updated > QUIET_MS {
        "quiet"
    } else if (states.iter())
        .any(|s| ["doing", "in-progress", "active", "running", "open"].contains(&s.as_str()))
    {
        "in-progress"
    } else if (states.iter()).any(|s| ["todo", "planned", "queued"].contains(&s.as_str())) {
        "todo"
    } else {
        "unknown"
    };
    // An explicit shipped is its own state, apart from done (claude-mem 15.2).
    let lifecycle = lifecycle(items).map(|r| r.2).map(|s| {
        match s.as_str() {
            "shipped" | "delivered" => "shipped",
            "done" | "completed" | "complete" => "done",
            _ => return s,
        }
        .to_owned()
    });
    // Work all done does not complete its request; work all dropped closes it (claude-mem 15.1).
    let mut status = from_work;
    if status == "done" && !matches!(lifecycle.as_deref(), Some("done" | "shipped")) {
        status = "unknown";
    }
    if let Some(known) = lifecycle.as_deref().and_then(category) {
        status = known;
    }
    // P5: a review no status was recorded after makes shipped `shipped`, and 100% done, waiting
    // for a person's check (claude-mem's `normalizeRequestStatus`).
    if let Some(review) = review {
        if review.state == "shipped" {
            status = "shipped";
        } else if review.percent == 100
            && matches!(status, "in-progress" | "todo" | "unknown" | "quiet")
        {
            status = "done";
        }
    }
    match status {
        "needs-you" => "needs",
        "done" | "needs-review" => "ready",
        "in-progress" => "working",
        other => other,
    }
}

/// The newest record of the request's own state among its items (claude-mem's lifecycle): by the
/// write that set it, then `request_complete`'s at the same write.
fn lifecycle(items: &[&Work]) -> Option<(Stamp, bool, String)> {
    let mut recorded: Vec<(Stamp, bool, String)> =
        items.iter().flat_map(|w| w.lifecycle()).collect();
    recorded.sort_by_key(|r| std::cmp::Reverse((r.0, r.1)));
    recorded.into_iter().next()
}

/// P5, claude-mem's `progress`: the percent of `review`, the one the card shows; 100 for shipped.
/// When it was reviewed; for shipped, the UTC day the newest recorded status completed the request
/// and its time (`completed_at`), else the review's shipped date; and the estimate. None with no
/// review and no ship.
fn progress(
    readings: &[crate::my_work_progress::Reviewed],
    review: Option<&crate::my_work_progress::Reviewed>,
    (status, completed_at): (&str, Option<i64>),
    rules: &Rules,
) -> Value {
    let shipped = status == "shipped";
    if review.is_none() && !shipped {
        return Value::Null;
    }
    let percent = if shipped {
        Some(100)
    } else {
        review.map(|r| r.percent)
    };
    let gate = |text: &str| crate::redact::outbound_with(text, rules);
    let completed = shipped
        .then(|| {
            (completed_at.map(|at| crate::db::utc(at)[..10].to_owned()))
                .or_else(|| review.and_then(|r| r.shipped.as_deref().map(gate)))
        })
        .flatten();
    json!({
        "percent": percent,
        "reviewed": review.map(|r| r.ts),
        "completed": completed,
        // The time of that status, which the page shows as its own day, the UTC day `completed`
        // gives staying for "Asked again after ship" (#731).
        "completed_at": completed_at.filter(|_| shipped),
        "estimate": (!shipped).then(|| crate::my_work_progress::estimate(readings)).flatten(),
    })
}

/// P5: the build story on the request's page, the narrative and the milestones of the review the
/// progress shows, gated as they are read; none when it has neither.
fn story(review: Option<&crate::my_work_progress::Reviewed>, rules: &Rules) -> Value {
    let gate = |text: &str| crate::redact::outbound_with(text, rules);
    let Some(r) = review.filter(|r| r.narrative.is_some() || !r.milestones.is_empty()) else {
        return Value::Null;
    };
    let milestones: Vec<Value> = (r.milestones.iter())
        .map(|(date, text)| json!({"date": gate(date), "text": gate(text)}))
        .collect();
    json!({"narrative": r.narrative.as_deref().map(gate), "milestones": milestones})
}

/// The status a recorded request status means, its aliases read as `statusFor` reads an item's
/// (Codex on #574: claude-mem's lifecycle takes only its exact status names, so a recorded
/// `failed` left the work to decide); none for one that names no status. `open`, which
/// `request_complete: false` records too, names none, as in claude-mem's lifecycle: as in
/// progress it would hide needs you and blocked (CodeRabbit on #697).
fn category(status: &str) -> Option<&'static str> {
    Some(match status {
        "needs-you" => "needs-you",
        "needs-review" => "needs-review",
        "blocked" | "failed" => "blocked",
        "doing" | "in-progress" | "active" | "running" => "in-progress",
        "todo" | "planned" | "queued" => "todo",
        "closed" | "dropped" => "closed",
        "done" => "done",
        "shipped" => "shipped",
        "quiet" => "quiet",
        "unknown" => "unknown",
        _ => return None,
    })
}

/// Whether a turn op, kept or skipped, covers `said`: the reply it led to has had its summary
/// written, or been passed.
fn covered(raw: &Raw, k: &Connection, said: &Said) -> Result<bool> {
    let Some(Item::Event(e)) = (raw.after(&said.device, said.seq - 1, 1)?.pop())
        .filter(|r| r.seq == said.seq)
        .map(|r| r.item)
    else {
        return Ok(true);
    };
    Ok(k.query_row(
        "SELECT 1 FROM turns WHERE device = ?1 AND agent = ?2 AND session = ?3
               AND from_seq <= ?4 AND through >= ?4 LIMIT 1",
        params![said.device, e.agent, e.session, said.seq],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

/// claude-mem's `requestText`: the text of a prompt a person wrote, without the harness's blocks
/// and instruction lines, at most `max` characters; none when nothing of it is left.
fn request_text(value: &str, max: usize) -> Option<String> {
    static SECTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)^\s*#{1,2}\s+My request\s*:\s*(?:\r?\n)?((?s:.*))\z").unwrap()
    });
    static BLOCKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            "environment_context",
            "instructions",
            "system-reminder",
            "turn_aborted",
            "permissions",
            "collaboration_mode",
            "skills_instructions",
        ]
        .iter()
        .map(|tag| Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}>")).unwrap())
        .collect()
    });
    static AGENTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)^\s*#\s*AGENTS\.md instructions[^\n]*(?:\n|$)").unwrap()
    });
    static PASTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)</?pasted_content\b[^>]*>").unwrap());
    static FILES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^\s*#{1,2}\s+Files mentioned by the user\s*:").unwrap());
    static HARNESS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*<(?:environment_context|instructions|system-reminder)\b").unwrap()
    });
    let mut text = SECTION
        .captures(value)
        .and_then(|c| c.get(1))
        .map_or(value, |m| m.as_str())
        .to_owned();
    for block in BLOCKS.iter() {
        text = block.replace_all(&text, " ").into_owned();
    }
    let text = AGENTS.replace_all(&text, " ");
    let text = PASTED.replace_all(&text, "");
    let text = text.trim();
    if text.is_empty() || FILES.is_match(text) || HARNESS.is_match(text) {
        return None;
    }
    Some(clip(text, max))
}

/// claude-mem's cut of a prompt: its first `max - 1` characters and `…`.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
}

/// claude-mem's `isHumanActionRequest` (15.0, with no list of English action words since): two
/// letters in any language that are not an acknowledgement, a greeting or the harness's text, in
/// English or Japanese.
fn asks(text: &str) -> bool {
    static REJECTED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r"(?i)^(?:please\s+)?read\s+(?:the\s+)?[\w./ -]*(?:AGENTS|KICKOFF|CLAUDE)\.md(?:\s+and\s+(?:follow|obey|read|apply)(?:\s+it|\s+the\s+instructions)?\s*)?[.!]?$",
            r"(?i)^(?:hi|hello|hey|thanks|thank you|ok(?:ay)?|yes|no|sure|continue|go ahead|keep going|resume|done)[.!\s]*$",
            r"(?i)^(?:You are (?:Codex|an AI)|Available skills|Environment context)\b",
            r"(?i)^(?:(?:please|just)\s+)?make sure (?:this|that|it|everything) (?:is |will be |keeps? )?(?:working|works|ok|okay|correct)(?:[.!\s]*)$",
            r"^(?:(?:はい|いいえ|うん|ええ|了解(?:です|しました)?|りょうかい|わかった|わかりました|分かりました|ありがとう(?:ございます)?|お願いします|おねがいします|よろしく(?:お願いします)?|続けて(?:ください)?|続行(?:して)?|再開(?:して(?:ください)?)?|進めて(?:ください)?|どうぞ|いいよ|いいです|大丈夫(?:です)?|オッケー|OK|ok)[、,\s]*)+[。．.!！\s]*$",
        ]
        .iter()
        .map(|r| Regex::new(r).unwrap())
        .collect()
    });
    static LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}.*\p{L}").unwrap());
    static TESTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^you can (?:def(?:initely)? )?do (?:real )?tests?[,;.!\s]").unwrap()
    });
    static WORK_WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:fix|build|create|implement|change|update|rename|remove|add)\b")
            .unwrap()
    });
    let plain = text.replace(['*', '_', '`', '#'], "");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    LETTERS.is_match(&plain)
        && !REJECTED.iter().any(|r| r.is_match(&plain))
        && !(TESTS.is_match(&plain) && !WORK_WORD.is_match(&plain))
}

/// claude-mem's `humanRequestTitle`: a `GOAL:` line or the first line, without its marks, its
/// first sentence or clause, at most `TITLE` characters cut at a word.
pub(crate) fn title(text: &str) -> String {
    static GOAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)(?:^|\n)\s*GOAL\s*:\s*([^\n]+)").unwrap());
    static CALLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bcalled\s*$").unwrap());
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s*").unwrap());
    let Some(text) = request_text(text, ASK) else {
        return String::new();
    };
    let lines: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let goal = GOAL
        .captures(&text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str());
    let first = goal.or(lines.first().copied()).unwrap_or("");
    let quoted = match lines.get(1) {
        Some(next) if CALLED.is_match(first) && next.starts_with(['"', '“', '\'', '‘']) => {
            format!(" {next}")
        }
        _ => String::new(),
    };
    let line = format!("{first}{quoted}");
    let line = LINK.replace_all(&line, "$1");
    let line = BOLD.replace_all(&line, "$1");
    let line = CODE.replace_all(&line, "$1");
    let line = HEADING.replace(&line, "");
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    // A sentence's end and a space, or `; `, as claude-mem splits; and a Japanese full stop.
    let chars: Vec<char> = line.chars().collect();
    let mut end = chars.len();
    for i in 0..chars.len() {
        let next = chars.get(i + 1).copied();
        if matches!(chars[i], '.' | '!' | '?') && next.is_some_and(char::is_whitespace) {
            end = i + 1;
            break;
        }
        if chars[i] == ';' && next.is_some_and(char::is_whitespace) {
            end = i;
            break;
        }
        if chars[i] == '。' {
            end = i + 1;
            break;
        }
    }
    let clause: String = chars[..end].iter().collect();
    if clause.chars().count() <= TITLE {
        return clause;
    }
    let cut: Vec<char> = clause.chars().take(TITLE - 1).collect();
    let boundary = cut.iter().rposition(|c| *c == ' ').filter(|&b| b > 60);
    let kept: String = cut[..boundary.unwrap_or(TITLE - 1)].iter().collect();
    format!("{}…", kept.trim_end())
}

/// claude-mem's `cleanText` cut: at most `max` characters, at a space past two thirds of them
/// where there is one, and `…`.
fn cut(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let cut: Vec<char> = text.chars().take(max - 1).collect();
    let boundary = (cut.iter().rposition(|c| *c == ' '))
        .filter(|&b| b as f64 > max as f64 * 0.65)
        .unwrap_or(max - 1);
    let kept: String = cut[..boundary].iter().collect();
    format!("{}…", kept.trim_end_matches([',', ':', ';', '-', ' ']))
}

#[cfg(test)]
mod tests;
