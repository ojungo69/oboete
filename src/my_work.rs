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

/// The first `max` characters of `text` as a request shows them: the gate reads `OVERLAP` more,
/// and a key block the cut leaves open goes, as `hook::clip` keeps a field. The gate reads no more
/// of a long prompt (a delegated review's runs to 20,000 characters).
pub(crate) fn shown(text: &str, max: usize, rules: &Rules) -> String {
    let window: String = text.chars().take(max + OVERLAP).collect();
    let mut gated = crate::redact::outbound_with(&window, rules);
    if window.len() < text.len()
        && !rules.is_off()
        && let Some(begin) = gated.rfind("-----BEGIN")
        && !gated[begin..].contains("-----END")
    {
        gated.truncate(begin);
    }
    clip(&gated, max)
}

/// A session summary, with the records it summarizes.
struct Turn {
    s: TurnSummary,
    span: (i64, i64),
}

/// A card, with the records its window holds.
struct Shown {
    c: Card,
    span: (i64, i64),
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
    /// only with its key), whole before the cut: a field is at most 2,000 characters (L3).
    fn text(&self, keys: &[&str], max: usize, rules: &Rules) -> Option<String> {
        keys.iter()
            .find_map(|k| Some((*k, self.fields.get(*k).and_then(meaningful)?)))
            .map(|(k, t)| cut(&crate::work_state::beside(k, &t, rules), max))
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
/// aliases of needs you.
fn normalized(status: &str) -> String {
    let s = status.trim().to_lowercase().replace(['_', ' '], "-");
    match s.as_str() {
        "needs-input" | "waiting-on-user" | "awaiting-approval" | "needs-approval" => {
            "needs-you".into()
        }
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
/// consumer's index `raw_docs_tools`, and each is read once (CodeRabbit on #574).
fn senders(
    raw: &Raw,
    near: &mut rusqlite::CachedStatement<'_>,
    read: &mut HashMap<(String, i64), Option<Call>>,
    write: &WorkWrite,
) -> Result<Vec<(Key, String, i64)>> {
    let span = params![write.ts - CALL_MS, write.ts + CALL_MS];
    let rows: Vec<(String, i64)> = near
        .query_map(span, |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut sent = Vec::new();
    for record in rows {
        if !read.contains_key(&record) {
            let c = call(raw, (&record.0, record.1))?;
            read.insert(record.clone(), c);
        }
        if let Some(c) = &read[&record]
            && c.repo.as_deref() == Some(write.repo.as_str())
            && c.list == write.list
            && c.fields == write.fields
        {
            sent.push((c.key.clone(), c.device.clone(), c.seq));
        }
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
fn link(raw: &Raw, k: &Connection, work: Vec<Work>, excluded: &HashSet<Key>) -> Result<Vec<Work>> {
    if !crate::consumer::manifest::exists(k, "index", "raw_docs_tools")? {
        let shown = if excluded.is_empty() { WORK } else { 0 };
        return Ok(work.into_iter().take(shown).collect());
    }
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
            if let [c] = &senders(raw, &mut near, &mut read, write)?[..] {
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
                let sent = senders(raw, &mut near, &mut read, write)?;
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
    /// A group's title, the newest grouping gave it (C6).
    title: Option<String>,
    owns: Vec<(Key, Owns)>,
    work: Vec<usize>,
}

impl Request {
    fn new(id: Id, original: Original) -> Self {
        Self {
            id,
            original,
            explicit_text: None,
            title: None,
            owns: Vec::new(),
            work: Vec::new(),
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
/// request takes (claude-mem's "N work records need a linked human request").
pub(crate) fn read(home: &Path, range: Range, now: i64) -> Result<Value> {
    Ok(read_held(home, range, now)?.0)
}

/// `read` of every range, with what each request's check is filed under, in the answer's order:
/// the review route's (rule 6).
pub(crate) fn reviewing(home: &Path, now: i64) -> Result<(Value, Vec<Held>)> {
    read_held(home, Range::All, now)
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
    // labels they return.
    let summaries = newest(
        SUMMARIES,
        |before| {
            let (page, more) = crate::turns::page(k, raw, (None, None), before, SUMMARIES, rules)?;
            let rows = (page.into_iter())
                .map(|s| {
                    let (span, key) = k.query_row(
                        "SELECT from_seq, through, agent, session FROM turns
                         WHERE device = ?1 AND op_seq = ?2",
                        params![s.device, s.op_seq],
                        |r| Ok(((r.get(0)?, r.get(1)?), (r.get(2)?, r.get(3)?))),
                    )?;
                    Ok((s, span, key))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((rows, more))
        },
        |(s, ..)| s.position(),
        |(_, _, key)| !excluded.contains(key),
    )?;
    for (s, span, key) in summaries {
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
                    let (span, agent, session): ((i64, i64), Option<String>, Option<String>) = k
                        .query_row(
                            "SELECT from_seq, to_seq, agent, session FROM cards
                             WHERE device = ?1 AND op_seq = ?2 AND n = ?3",
                            params![c.device, c.op_seq, c.n],
                            |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?, r.get(3)?)),
                        )?;
                    Ok((c, span, agent.zip(session)))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((rows, more))
        },
        |(c, ..)| c.position(),
        |(_, _, key)| key.as_ref().is_some_and(|key| !excluded.contains(key)),
    )?;
    for (card, span, key) in cards {
        let Some(key) = key else { continue };
        let c = convs.entry(key).or_default();
        c.repos.extend(card.repo.clone());
        c.cards.push(Shown { c: card, span });
    }
    let writes = (raw.native_work_writes()?.into_iter())
        .filter(|w| !excluded_repos.contains(&w.repo))
        .collect();
    let work = link(raw, k, fold(writes), &excluded)?;
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

fn read_held(home: &Path, range: Range, now: i64) -> Result<(Value, Vec<Held>)> {
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok((json!({"requests": [], "unlinked": 0}), Vec::new()));
    };
    let rules = Rules::load(home)?;
    let Gathered { convs, work, named } = gather(&raw, &k, &rules)?;
    let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live)?;
    let grouped = crate::my_work_group::groups(&raw, &k, &reading)?;
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
        let explicit_text = w.request_text(&rules);
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
            r.title = grouped.groups.get(anchor).and_then(|g| g.title.clone());
        }
    }
    let since = range.since(now);
    let mut answers = Vec::new();
    let mut key = None;
    let reviews = reviews(home);
    for r in &requests {
        let stores = (&raw, &k, &rules);
        answers.extend(answer(
            r,
            (&convs, &work, &reviews),
            stores,
            (home, &mut key),
            (since, now),
        )?);
    }
    answers.sort_by_key(|(v, _)| std::cmp::Reverse(v["updated"].as_i64().unwrap_or(0)));
    let (answers, held): (Vec<Value>, Vec<Held>) = answers.into_iter().unzip();
    Ok((json!({"requests": answers, "unlinked": unlinked}), held))
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
}

/// Every conversation My Work reads (`gather`), as grouping reads it.
pub(crate) fn seen(raw: &Raw, k: &Connection, rules: &Rules) -> Result<Vec<Seen>> {
    let Gathered { convs, work, named } = gather(raw, k, rules)?;
    let mut lists = writers(raw, k, &work)?;
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
        out.push(Seen {
            named: named.get(&key).cloned().unwrap_or_default(),
            lists: lists.remove(&key).unwrap_or_default(),
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
    raw: &Raw,
    k: &Connection,
    work: &[Work],
) -> Result<HashMap<Key, BTreeSet<(String, String)>>> {
    let mut out: HashMap<Key, BTreeSet<(String, String)>> = HashMap::new();
    if !crate::consumer::manifest::exists(k, "index", "raw_docs_tools")? {
        return Ok(out);
    }
    let mut near = k.prepare_cached(
        "SELECT device, seq FROM raw_docs INDEXED BY raw_docs_tools
         WHERE kind = 'tool' AND ts BETWEEN ?1 AND ?2",
    )?;
    let mut read = HashMap::new();
    for w in work {
        for write in w.writes.iter().chain(&w.list_writes) {
            for (key, ..) in senders(raw, &mut near, &mut read, write)? {
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
    sessions: Vec<String>,
    sources: Sources,
}

/// A request's conversations, each with the prompt spans it owns (none: all of it), its work
/// items' fields with the time of their last write, and its cards' modified files with the time
/// of their window.
#[derive(Default)]
pub(crate) struct Sources {
    convs: Vec<(Key, Option<Vec<Span>>)>,
    fields: Vec<(Map<String, Value>, i64)>,
    files: Vec<(Vec<String>, i64)>,
}

/// claude-mem's `mergeProducts` limit: the newest this many entries of a request's work product.
const PRODUCTS: usize = 60;

/// Slice 4 (parity row X16): what a request produced, as claude-mem 15.1's request drawer shows it:
/// the commits, branches and pull requests its conversations' recorded commands made, the links
/// its publishing tools returned, the files of kinds a person opens that its cards name as
/// modified, and what its work items attach. The calls are read live from raw.db, so a forgotten
/// or expired one is none; each entry is gated as the page's text is, and a link the gate changes
/// is shown as its title alone. No provider is called.
pub(crate) fn products(home: &Path, held: &Held) -> Result<Vec<Value>> {
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok(Vec::new());
    };
    let rules = Rules::load(home)?;
    let mut lists = Vec::new();
    // The consumer makes its table; before its first step there is nothing to read.
    if crate::consumer::manifest::exists(&k, "table", "product_calls")? {
        let mut calls = k.prepare(
            "SELECT device, seq FROM product_calls WHERE agent = ?1 AND session = ?2
             ORDER BY device, seq",
        )?;
        for ((agent, session), owned) in &held.sources.convs {
            let rows = (calls
                .query_map(params![agent, session], |r| Ok((r.get(0)?, r.get(1)?)))?)
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;
            for (device, seq) in rows {
                let ours = |spans: &Vec<Span>| spans.iter().any(|s| holds(s, &device, (seq, seq)));
                if owned.as_ref().is_some_and(|spans| !ours(spans)) {
                    continue;
                }
                let Some(Item::Event(e)) = (raw.after(&device, seq - 1, 1)?.pop())
                    .filter(|r| r.seq == seq)
                    .map(|r| r.item)
                else {
                    continue;
                };
                let Ok(body) = serde_json::from_str::<Value>(&e.body) else {
                    continue;
                };
                let source = crate::work_product::Source {
                    kind: "record".into(),
                    id: format!("{device}:{seq}"),
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
    for (fields, at) in &held.sources.fields {
        // What a work item attaches, as work state's own lines show it beside its key, since a
        // rule may match a value only there; a link the gate changes is its title alone
        // (CodeRabbit on #650).
        let mut changed = Vec::new();
        let shown = (fields.iter())
            .filter(|(k, _)| crate::work_product::is_work_product_field(k))
            .map(|(k, v)| {
                let text = crate::work_state::text(v);
                let gated = crate::work_state::beside(k, &text, &rules);
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
            if p.field.as_ref().is_some_and(|f| changed.contains(f)) {
                p.url = None;
            }
        }
        lists.push(list);
    }
    for (files, at) in &held.sources.files {
        lists.push(crate::work_product::from_files(files, Some(*at), None));
    }
    let gate = |s: &str| crate::redact::outbound_with(s, &rules);
    Ok((crate::work_product::merge(lists, PRODUCTS).into_iter())
        .map(|p| {
            let url = p.url.filter(|url| gate(url) == *url);
            json!({
                "kind": gate(&p.kind),
                "title": gate(&p.title),
                "url": url,
                "path": p.path.as_deref().map(gate),
                "at": p.at,
            })
        })
        .collect())
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

pub(crate) fn reviews_lock(home: &Path) -> Result<ReviewsLock> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let lock = options.open(state.join("my-work-reviews.lock"))?;
    lock.lock()?;
    Ok(ReviewsLock(lock))
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
/// longer read (`current`) dropped, so that a request a forget took leaves nothing here.
pub(crate) fn set_review(
    (home, lock): (&Path, &ReviewsLock),
    (held, current): (&Held, &[Held]),
    version: &str,
    checked: bool,
    now: i64,
) -> Result<()> {
    let read: HashSet<&str> = current.iter().map(|h| h.key.as_str()).collect();
    change_reviews(home, lock, |all| {
        all.retain(|key, _| read.contains(key.as_str()));
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
/// Whoever can read the file can read the stores, which hold the ids themselves.
fn mask_key(home: &Path) -> Result<Vec<u8>> {
    let path = home.join("state").join("my-work.key");
    let read = || std::fs::read(&path).ok().filter(|key| key.len() == 32);
    if let Some(key) = read() {
        return Ok(key);
    }
    let (mut key, mut tag) = ([0u8; 32], [0u8; 8]);
    getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("random key: {e}"))?;
    getrandom::fill(&mut tag).map_err(|e| anyhow::anyhow!("random key: {e}"))?;
    std::fs::create_dir_all(home.join("state"))?;
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = home.join("state").join(format!("my-work.key.{tag}"));
    std::fs::write(&tmp, key)?;
    // Published whole: a link fails on a name another read took first, whose key stays.
    if let Err(e) = std::fs::hard_link(&tmp, &path) {
        if let Some(theirs) = (e.kind() == std::io::ErrorKind::AlreadyExists)
            .then(read)
            .flatten()
        {
            let _ = std::fs::remove_file(&tmp);
            return Ok(theirs);
        }
        // No links on this filesystem, or a file of the name that holds no key: this one's.
        std::fs::rename(&tmp, &path)?;
        return Ok(key.to_vec());
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(key.to_vec())
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
fn answer(
    r: &Request,
    (convs, work, reviews): (
        &BTreeMap<Key, Conversation>,
        &[Work],
        &BTreeMap<String, Review>,
    ),
    (raw, k, rules): (&Raw, &Connection, &Rules),
    (home, key): (&Path, &mut Option<Vec<u8>>),
    (since, now): (i64, i64),
) -> Result<Option<(Value, Held)>> {
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
    let status = status(&items, activity, now);
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
                names
                    .iter()
                    .find_map(|n| t.s.fields.get(*n))
                    .filter(|v| !v.trim().is_empty())
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
    let sources = Sources {
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
            .map(|c| (c.c.files_modified.clone(), c.c.ts))
            .filter(|(files, _)| !files.is_empty())
            .collect(),
    };
    let held = Held {
        key: filed,
        sessions: swept_by,
        sources,
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
            "summaries": turns.iter().map(|t| t.s.id(raw.device())).collect::<Vec<_>>(),
            "cards": cards.iter().take(ACTIVITY).map(|c| c.c.id(raw.device())).collect::<Vec<_>>(),
        }),
        held,
    )))
}

/// claude-mem's `statusFor` over a request's work and its records (an observation's row, here the
/// newest of its prompts, replies and cards), then its lifecycle (`buildModel`), as the page names
/// them: needs, blocked, ready, quiet, working, todo, unknown, closed.
fn status(items: &[&Work], activity: Option<i64>, now: i64) -> &'static str {
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
    } else if states.iter().any(|s| s == "blocked" || s == "failed") {
        "blocked"
    } else if all(&["done", "completed"]) {
        "done"
    } else if all(&["dropped"]) {
        "closed"
    } else if now - updated > QUIET_MS {
        "quiet"
    } else if (states.iter())
        .any(|s| ["doing", "in-progress", "active", "running"].contains(&s.as_str()))
    {
        "in-progress"
    } else if (states.iter()).any(|s| ["todo", "planned", "queued"].contains(&s.as_str())) {
        "todo"
    } else {
        "unknown"
    };
    // The newest record of the request's own state among its items decides (claude-mem's
    // lifecycle): by the write that set it, then `request_complete`'s at the same write.
    let mut recorded: Vec<(Stamp, bool, String)> =
        items.iter().flat_map(|w| w.lifecycle()).collect();
    recorded.sort_by_key(|r| std::cmp::Reverse((r.0, r.1)));
    let lifecycle = recorded.into_iter().next().map(|r| r.2);
    let complete = lifecycle
        .as_deref()
        .is_some_and(|s| ["done", "completed", "complete", "shipped", "delivered"].contains(&s));
    let lifecycle = if complete {
        Some("done".to_owned())
    } else {
        lifecycle
    };
    let mut status = from_work;
    if matches!(status, "done" | "closed")
        && !matches!(lifecycle.as_deref(), Some("done" | "closed"))
    {
        status = "unknown";
    }
    if let Some(known) = lifecycle.as_deref().and_then(category) {
        status = known;
    }
    match status {
        "needs-you" => "needs",
        "done" => "ready",
        "in-progress" => "working",
        other => other,
    }
}

/// The status a recorded request status means, its aliases read as `statusFor` reads an item's
/// (Codex on #574: claude-mem's lifecycle takes only its exact status names, so a recorded
/// `failed` left the work to decide); none for one that names no status.
fn category(status: &str) -> Option<&'static str> {
    Some(match status {
        "needs-you" => "needs-you",
        "blocked" | "failed" => "blocked",
        "doing" | "in-progress" | "active" | "running" => "in-progress",
        "todo" | "planned" | "queued" => "todo",
        "closed" | "dropped" => "closed",
        "done" => "done",
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
fn title(text: &str) -> String {
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
mod tests {
    use super::*;
    use crate::raw::{OpKind, Target};
    use crate::search::b::fixture::Store;
    use serde_json::Map;

    const R: &str = "github.com/o/r";

    /// A home whose times are near now: a work state write takes the time it is appended, in
    /// `repo`, as the call that sent it.
    struct Home {
        s: Store,
        t: i64,
        repo: &'static str,
    }

    impl Home {
        fn new() -> Self {
            Self {
                s: Store::new(),
                t: crate::db::now_ms(),
                repo: R,
            }
        }

        /// A prompt of `session` at `at` ms from the start: its seq.
        fn said(&mut self, session: &str, at: i64, text: &str) -> i64 {
            self.s.said(session, R, self.t + at, text)
        }

        /// The agent's reply in `session` at `at`: its seq.
        fn reply(&mut self, session: &str, at: i64) -> i64 {
            let body = json!({"text": "Done."});
            self.s
                .event("reply", session, (R, "main"), self.t + at, body)
        }

        /// A work state write as oboete's server appends it: its time and fields.
        fn write(&mut self, list: &str, fields: Value) -> (i64, Map<String, Value>) {
            let fields = fields.as_object().unwrap().clone();
            let seq = self
                .s
                .raw
                .work_state(self.repo, list, &fields, None)
                .unwrap();
            let ts = rusqlite::Connection::open(self.s.home.path().join("raw.db"))
                .unwrap()
                .query_row(
                    "SELECT ts FROM ops WHERE device = ?1 AND op_seq = ?2",
                    rusqlite::params![self.s.raw.device(), seq],
                    |r| r.get(0),
                )
                .unwrap();
            (ts, fields)
        }

        /// The call of `work_state_write` in `session` at `ts` that sent `list` and `fields`.
        fn call(&mut self, session: &str, ts: i64, list: &str, fields: &Map<String, Value>) -> i64 {
            let input = json!({"list": list, "fields": fields}).to_string();
            let body = json!({"tool": "mcp__oboete__work_state_write", "input": input,
                "output": "Saved.", "failed": false});
            self.s.event("tool", session, (self.repo, "main"), ts, body)
        }

        /// A write and the call that made it in `session`, 20 ms after it: the call's seq.
        fn wrote(&mut self, session: &str, list: &str, fields: Value) -> i64 {
            let (ts, fields) = self.write(list, fields);
            self.call(session, ts + 20, list, &fields)
        }

        /// The session summary of `session`'s records `from` to `through`, at `at`: its ID.
        fn turn(
            &mut self,
            session: &str,
            (from, through): (i64, i64),
            at: i64,
            f: Value,
        ) -> String {
            self.s
                .turn(json!({"agent": "claude", "session": session, "repo": R,
                "ts": self.t + at, "from": from, "through": through, "read": [], "goals": [],
                "removed": [], "fields": f, "skipped": false}))
        }

        fn read(&self, range: Range, now: i64) -> Value {
            self.s.run();
            super::read(self.s.home.path(), range, now).unwrap()
        }

        /// The worker run, as `read` runs it, for a test that reads by another function.
        fn run_read(&self) {
            self.s.run();
        }
    }

    /// A `Bash` call in `session` at `at` that ran `command` and printed `out`: its seq.
    fn bash(h: &mut Home, session: &str, at: i64, command: &str, out: &str) -> i64 {
        let body = json!({"tool": "Bash", "input": json!({"command": command}).to_string(),
            "output": out, "failed": false});
        h.s.event("tool", session, (R, "main"), h.t + at, body)
    }

    fn by_id<'a>(v: &'a Value, id: &str) -> &'a Value {
        v["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no {id} in {v}"))
    }

    fn ids(v: &Value) -> Vec<String> {
        let requests = v["requests"].as_array().unwrap();
        requests
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn tasks(r: &Value) -> Vec<&str> {
        let work = r["work"].as_array().unwrap();
        work.iter().map(|w| w["task"].as_str().unwrap()).collect()
    }

    /// The design's 1 (claude-mem's `originalFor` and scope updates): a session's first prompt
    /// that asks is its request's original ask, not an acknowledgement or a greeting, in English
    /// or Japanese; its later prompts, the newest 6, are the follow-ups, each cut to 500
    /// characters.
    #[test]
    fn the_first_prompt_that_asks_is_the_original_ask_and_the_later_ones_follow_up() {
        let mut h = Home::new();
        h.said("s1", -60_000, "再開して");
        h.said("s1", -59_000, "Fix the parser's error messages.");
        for i in 1..=7 {
            h.said("s1", -58_000 + i * 1_000, &format!("Step {i}."));
        }
        h.said("s1", -40_000, &"x".repeat(600));
        h.said("s2", -30_000, "Continue.");
        h.said("s2", -29_000, "パーサのエラーを直して");
        let v = h.read(Range::All, h.t);
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["ask"]["text"], "Fix the parser's error messages.");
        let follow: Vec<&str> = (s1["follow_ups"].as_array().unwrap().iter())
            .map(|f| f["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            &follow[..5],
            ["Step 3.", "Step 4.", "Step 5.", "Step 6.", "Step 7."]
        );
        assert_eq!(follow.len(), 6);
        assert_eq!(follow[5].chars().count(), 500);
        assert!(follow[5].ends_with('…'));
        assert_eq!(
            by_id(&v, "c:claude:s2")["ask"]["text"],
            "パーサのエラーを直して"
        );
    }

    /// Slice 4 (parity row X16): a request's work product, as claude-mem 15.1 reads it: the commit
    /// its conversation made and pushed to GitHub, linked to it; a command that only names git
    /// makes nothing; what a work item attaches shows; a record that expired is no call, and its
    /// commit goes with it. Another conversation's commit is not this request's.
    #[test]
    fn a_requests_work_product_is_read_live_from_its_records() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship the parser fix.");
        let out = "[main abc1234] Fix the parser\n 1 file changed\nTo https://github.com/o/r.git\n   0000000..abc1234  main -> main";
        let commit = bash(
            &mut h,
            "s1",
            -59_000,
            "git commit -m 'Fix the parser' && git push origin main",
            out,
        );
        bash(&mut h, "s1", -58_000, "grep -rn 'git commit' docs", "");
        h.wrote(
            "s1",
            "release",
            json!({"task": "Ship", "status": "doing",
            "work_product": "https://example.com/notes"}),
        );
        h.said("s2", -50_000, "Rename the lexer.");
        bash(
            &mut h,
            "s2",
            -49_000,
            "git commit -m 'Rename the lexer'",
            "[main def5678] Rename the lexer",
        );
        h.run_read();
        let shown = |h: &Home| {
            let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
            let i = ids(&answer)
                .iter()
                .position(|id| id == "c:claude:s1")
                .unwrap();
            super::products(h.s.home.path(), &held[i]).unwrap()
        };
        let products = shown(&h);
        let titles: Vec<&str> = products
            .iter()
            .map(|p| p["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["example.com", "abc1234 Fix the parser", "main"],
            "{products:?}"
        );
        assert_eq!(products[1]["url"], "https://github.com/o/r/commit/abc1234");
        assert_eq!(products[0]["url"], "https://example.com/notes");
        h.s.raw.expire_for_test(&[commit]);
        let products = shown(&h);
        let titles: Vec<&str> = products
            .iter()
            .map(|p| p["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["example.com"], "{products:?}");
    }

    /// #652: a conversation's commit in one repository takes no link from its later push of the
    /// same branch in another; the push's own commit keeps its link.
    #[test]
    fn a_commit_takes_no_link_from_a_push_in_another_repository() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix both repositories.");
        let tool = |command: &str, out: &str| {
            json!({"tool": "Bash", "input": json!({"command": command}).to_string(),
                "output": out, "failed": false})
        };
        let t = h.t;
        let commit = tool("git commit -m 'Fix A'", "[main abc1234] Fix A");
        h.s.event("tool", "s1", ("github.com/o/a", "main"), t - 59_000, commit);
        let out = "[main def5678] Fix B\nTo https://github.com/o/b.git\n   0000000..def5678  main -> main";
        let pushed = tool("git commit -m 'Fix B' && git push origin main", out);
        h.s.event("tool", "s1", ("github.com/o/b", "main"), t - 58_000, pushed);
        h.run_read();
        let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
        let i = ids(&answer)
            .iter()
            .position(|id| id == "c:claude:s1")
            .unwrap();
        let products = super::products(h.s.home.path(), &held[i]).unwrap();
        let url = |title: &str| {
            (products.iter())
                .find(|p| p["title"] == title)
                .map(|p| p["url"].clone())
        };
        assert_eq!(url("abc1234 Fix A"), Some(Value::Null), "{products:?}");
        assert_eq!(
            url("def5678 Fix B"),
            Some(json!("https://github.com/o/b/commit/def5678")),
            "{products:?}"
        );
    }

    /// Slice 4: of a conversation two requests share, each has the work product of the prompts it
    /// owns.
    #[test]
    fn a_shared_conversations_work_product_follows_its_prompts() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A",
            "request": "Write the release notes", "status": "doing"}),
        );
        bash(
            &mut h,
            "s1",
            -55_000,
            "git commit -m 'Write the notes'",
            "[main aaa1111] Write the notes",
        );
        h.said("s1", -40_000, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B",
            "request": "Tag the release", "status": "doing"}),
        );
        bash(
            &mut h,
            "s1",
            -35_000,
            "git checkout -b release-1",
            "Switched to a new branch 'release-1'",
        );
        h.run_read();
        let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
        let titles = |id: &str| -> Vec<String> {
            let i = ids(&answer).iter().position(|x| x == id).unwrap();
            (super::products(h.s.home.path(), &held[i]).unwrap().iter())
                .map(|p| p["title"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(titles("r:A"), ["aaa1111 Write the notes"]);
        assert_eq!(titles("r:B"), ["release-1"]);
    }

    /// Slice 4: the rules as they are now gate what is shown, and a link they change shows as its
    /// title alone; a file a card names as modified shows by its name and path.
    #[test]
    fn work_product_is_gated_as_shown_and_names_files() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "Write the report.");
        h.wrote(
            "s1",
            "report",
            json!({"task": "report", "status": "doing",
            "docs_url": "https://intranet.example/report",
            "spec_url": "https://wiki.example/spec",
            "notes_url": "https://wiki.example/private/page"}),
        );
        let r = h.reply("s1", -50_000);
        h.s.cards(
            p,
            r,
            json!([{"type": "feature", "title": "Report", "narrative": "Wrote it.",
            "files_modified": ["/home/u/report.pdf", "/home/u/src/main.rs"]}]),
            false,
        );
        h.run_read();
        // Rules added after the worker's run: the read gates with them before a rescan does. Two
        // match a value only beside its key, as work state's lines show it (CodeRabbit on #650):
        // one hides it whole, one a part of it.
        std::fs::write(
            h.s.home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = 'intranet', regex = 'intranet' }, \
             { id = 'keyed', regex = 'spec_url = \"https' }, \
             { id = 'part', regex = 'notes_url = \"https://wiki\\.example/(private)' }]\n",
        )
        .unwrap();
        let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
        let i = ids(&answer)
            .iter()
            .position(|id| id == "c:claude:s1")
            .unwrap();
        let products = super::products(h.s.home.path(), &held[i]).unwrap();
        // A work item's link the gate hides any of, beside its key, is its title alone when an
        // address is left (notes_url), and nothing when none is (docs_url's host, spec_url whole).
        let shown = serde_json::to_string(&products).unwrap();
        assert!(
            !shown.contains("intranet") && !shown.contains("/spec") && !shown.contains("private"),
            "{products:?}"
        );
        assert!(
            !products.iter().any(|p| p["kind"] == "documentation"),
            "{products:?}"
        );
        let wiki: Vec<&Value> = products
            .iter()
            .filter(|p| p["title"] == "wiki.example")
            .collect();
        assert_eq!(wiki.len(), 1, "{products:?}");
        assert!(wiki[0]["url"].is_null(), "{products:?}");
        let files: Vec<&Value> = products.iter().filter(|p| p["path"].is_string()).collect();
        assert_eq!(files.len(), 1, "{products:?}");
        assert_eq!(
            (files[0]["title"].as_str(), files[0]["path"].as_str()),
            (Some("report.pdf"), Some("/home/u/report.pdf"))
        );
    }

    /// A session whose prompts never ask is no request; its card counts among the unlinked, and
    /// so, once, do the cards before its first prompt.
    #[test]
    fn a_session_that_never_asks_is_no_request() {
        let mut h = Home::new();
        let card =
            json!([{"type": "change", "title": "Went on", "subtitle": "s", "narrative": "n"}]);
        let p = h.said("s1", -60_000, "続けて");
        h.s.cards(p, p, card.clone(), false);
        let t = (h.s).event(
            "tool",
            "s2",
            (R, "main"),
            h.t - 59_000,
            json!({"tool": "Bash"}),
        );
        h.s.cards(t, t, card, false);
        h.said("s2", -58_000, "続けて");
        let v = h.read(Range::All, h.t);
        assert_eq!((ids(&v), &v["unlinked"]), (Vec::<String>::new(), &json!(2)));
    }

    /// A prompt asks once it holds two letters, in any language, as claude-mem 15.0's
    /// `isHumanActionRequest` reads one past its exclusions: a mark or a number alone does not.
    #[test]
    fn a_prompt_asks_with_two_letters_in_any_language() {
        for (text, asked) in [
            ("?", false),
            ("1", false),
            ("a?", false),
            ("42!", false),
            ("ok?", true),
            ("直して", true),
            ("Fix it", true),
            ("再開して", false),
        ] {
            assert_eq!(asks(text), asked, "{text}");
        }
    }

    /// Its work (claude-mem's `uniqueCall`): a work state write is the work of the session whose
    /// one call of oboete's `work_state_write` sent the same list and fields within 5 s; a write
    /// two calls match, or none, or one past 5 s, is no session's and is counted.
    #[test]
    fn a_write_is_the_work_of_the_one_session_whose_call_sent_it_within_five_seconds() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        h.said("s2", -60_000, "Fix the lexer.");
        let (ts, f) = h.write("plan", json!({"task": "parser", "status": "doing"}));
        h.call("s1", ts + 20, "plan", &f);
        // The same list and fields sent in another repository, or to another server, are not
        // its call.
        let input = json!({"list": "plan", "fields": f}).to_string();
        let sent =
            |tool: &str| json!({"tool": tool, "input": input, "output": "", "failed": false});
        let elsewhere = ("github.com/o/other", "main");
        h.s.event(
            "tool",
            "s2",
            elsewhere,
            ts + 25,
            sent("mcp__oboete__work_state_write"),
        );
        h.s.event(
            "tool",
            "s2",
            (R, "main"),
            ts + 30,
            sent("mcp__other__work_state_write"),
        );
        let (ts, f) = h.write("plan", json!({"task": "twice", "status": "todo"}));
        for (s, after) in [("s1", 20), ("s2", 30)] {
            h.call(s, ts + after, "plan", &f);
        }
        h.write("plan", json!({"task": "uncalled", "status": "todo"}));
        let (ts, f) = h.write("plan", json!({"task": "late", "status": "todo"}));
        h.call("s2", ts + 6_000, "plan", &f);
        let v = h.read(Range::All, h.t + 10_000);
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(tasks(s1), ["parser"]);
        assert_eq!(s1["status"], "working");
        assert_eq!(tasks(by_id(&v, "c:claude:s2")), Vec::<&str>::new());
        assert_eq!(v["unlinked"], 3);
    }

    /// The design's 2: work state imported from claude-mem (sourced ops, row N1) has no call in a
    /// session: My Work neither reads nor counts it.
    #[test]
    fn imported_work_state_is_neither_read_nor_counted() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let settings = crate::capture::Settings::load(h.s.home.path()).unwrap();
        let op = json!({"repo": "claude-mem:r", "list": "plan",
            "fields": {"task": "old", "status": "doing"}, "clock": 1, "at": h.t - 1_000,
            "source": "claude-mem", "source_id": "w1"});
        (h.s.raw)
            .append_imported_ops(&[(OpKind::WorkState, op)], &settings)
            .unwrap();
        let v = h.read(Range::All, h.t);
        assert_eq!(v["unlinked"], 0);
        assert_eq!(tasks(by_id(&v, "c:claude:s1")), Vec::<&str>::new());
    }

    /// The design's 1: two `request_id`s in a session make two requests with its original ask,
    /// each titled by its own recorded request, each with the replies its writes were made in and
    /// their summaries; a reply of neither, with a card, counts once among the unlinked. One id
    /// in a session takes all of it.
    #[test]
    fn two_request_ids_in_a_session_make_two_requests_each_with_its_own_replies() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A", "request": "Write the release notes",
                "status": "doing"}),
        );
        let r = h.reply("s1", -50_000);
        let notes = h.turn(
            "s1",
            (p, r),
            -50_000,
            json!({"request": "Release notes", "completed": "Notes written."}),
        );
        // A record between a reply and the next prompt: the next reply's turn starts with it.
        let gap = (h.s).event("compaction", "s1", (R, "main"), h.t - 45_000, json!({}));
        h.said("s1", -40_000, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B", "request": "Tag the release",
                "status": "doing"}),
        );
        let r = h.reply("s1", -35_000);
        let tag = h.turn(
            "s1",
            (gap, r),
            -35_000,
            json!({"request": "Tag", "completed": "Tagged."}),
        );
        let p = h.said("s1", -30_000, "Check the build.");
        let r = h.reply("s1", -25_000);
        h.turn(
            "s1",
            (p, r),
            -25_000,
            json!({"request": "Build", "completed": "Green."}),
        );
        h.s.cards(
            p,
            r,
            json!([{"type": "discovery", "title": "Green", "subtitle": "CI passed.",
                "narrative": "CI passed."}]),
            false,
        );
        let q = h.said("s2", -20_000, "Fix the docs.");
        h.wrote(
            "s2",
            "docs",
            json!({"task": "readme", "request_id": "C", "status": "todo"}),
        );
        let r = h.reply("s2", -15_000);
        let docs = h.turn(
            "s2",
            (q, r),
            -15_000,
            json!({"request": "Docs", "completed": "Fixed."}),
        );
        let q = h.said("s2", -12_000, "And the changelog.");
        let r = h.reply("s2", -11_000);
        let changelog = h.turn(
            "s2",
            (q, r),
            -11_000,
            json!({"request": "Changelog", "completed": "Added."}),
        );
        let v = h.read(Range::All, h.t + 10_000);
        let (a, b) = (by_id(&v, "r:A"), by_id(&v, "r:B"));
        for r in [a, b] {
            assert_eq!(r["ask"]["text"], "Ship the release.");
        }
        assert_eq!(a["title"], "Write the release notes");
        assert_eq!(b["title"], "Tag the release");
        assert_eq!(
            (&a["summaries"], &a["summary"]),
            (&json!([notes]), &json!("Notes written."))
        );
        assert_eq!(
            (&b["summaries"], &b["summary"]),
            (&json!([tag]), &json!("Tagged."))
        );
        assert!(!ids(&v).contains(&"c:claude:s1".to_owned()));
        assert_eq!(v["unlinked"], 1);
        assert_eq!(by_id(&v, "r:C")["summaries"], json!([changelog, docs]));
    }

    /// The design's 1: in a session of two `request_id`s, each request owns the prompts its work
    /// was written in, with their cards, before any reply is summarized, as claude-mem gives such
    /// a request the observations of its prompts; a prompt of neither, with a card, counts once
    /// among the unlinked (Greptile on #574).
    #[test]
    fn two_request_ids_own_their_prompts_cards_before_any_summary() {
        let mut h = Home::new();
        let card = |title: &str| json!([{"type": "change", "title": title, "subtitle": title, "narrative": "x"}]);
        let p = h.said("s1", -60_000, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A", "request": "Write the release notes",
                "status": "doing"}),
        );
        let r = h.reply("s1", -50_000);
        let notes = h.s.cards(p, r, card("Notes written"), false);
        let p = h.said("s1", -40_000, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B", "request": "Tag the release",
                "status": "doing"}),
        );
        let r = h.reply("s1", -35_000);
        let tag = h.s.cards(p, r, card("Tagged"), false);
        let p = h.said("s1", -30_000, "Check the build.");
        let r = h.reply("s1", -25_000);
        h.s.cards(p, r, card("CI passed"), false);
        let v = h.read(Range::All, h.t + 10_000);
        let (a, b) = (by_id(&v, "r:A"), by_id(&v, "r:B"));
        assert_eq!(
            (&a["cards"], &a["copy"]),
            (&json!(notes), &json!("recorded"))
        );
        assert_eq!((&b["cards"], &b["copy"]), (&json!(tag), &json!("recorded")));
        assert_eq!(v["unlinked"], 1);
    }

    /// The design's 1: a forgotten prompt still ends the prompt before it, by its row until the
    /// purge and by what the forget keeps after it (`denied_records`), with no text read, so the
    /// requests of a session do not share the cards after it (Greptile on #574).
    #[test]
    fn a_forgotten_prompt_still_ends_the_prompt_before_it() {
        let mut h = Home::new();
        let card = |title: &str| json!([{"type": "change", "title": title, "subtitle": title, "narrative": "x"}]);
        let p = h.said("s1", -60_000, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A", "status": "doing"}),
        );
        let r = h.reply("s1", -50_000);
        let notes = h.s.cards(p, r, card("Notes written"), false);
        let gone = h.said("s1", -40_000, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B", "status": "doing"}),
        );
        let t = (h.s).event(
            "tool",
            "s1",
            (R, "main"),
            h.t - 36_000,
            json!({"tool": "Bash"}),
        );
        let tag = h.s.cards(t, t, card("Tagged"), false);
        let path = h.s.home.path().to_owned();
        let target = crate::forget::Target::Record {
            device: h.s.raw.device().to_owned(),
            seq: gone,
        };
        crate::forget::start(&path, &crate::forget::preview(&path, target).unwrap()).unwrap();
        let shown = |v: &Value| {
            (
                by_id(v, "r:A")["cards"].clone(),
                by_id(v, "r:B")["cards"].clone(),
            )
        };
        assert_eq!(
            shown(&h.read(Range::All, h.t + 10_000)),
            (json!(notes), json!(tag))
        );
        // The purge has raw.db to itself, as `oboete forget` has it.
        let Home { s, t, .. } = h;
        let crate::search::b::fixture::Store { home, raw } = s;
        drop(raw);
        assert!(crate::forget::purge(home.path()).unwrap().is_some());
        crate::worker::run_once(home.path()).unwrap();
        let v = super::read(home.path(), Range::All, t + 10_000).unwrap();
        assert_eq!(shown(&v), (json!(notes), json!(tag)));
        assert!(!v.to_string().contains("Now tag it"), "{v}");
    }

    /// The design's 3: a request of several ids says "Summary updating" when the newest of its own
    /// prompts has no reply summarized yet, as a request of a whole session does (Codex on #574).
    #[test]
    fn a_request_of_several_ids_is_updating_by_its_own_newest_prompt() {
        let mut h = Home::new();
        let summary = |c: &str| json!({"request": c, "completed": c});
        let p = h.said("s1", -60_000, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A", "status": "doing"}),
        );
        let r = h.reply("s1", -55_000);
        h.turn("s1", (p, r), -55_000, summary("Notes written."));
        let p = h.said("s1", -50_000, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B", "status": "doing"}),
        );
        let r = h.reply("s1", -45_000);
        h.turn("s1", (p, r), -45_000, summary("Tagged."));
        h.said("s1", -40_000, "Add the changelog to the notes.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "changelog", "request_id": "A", "status": "doing"}),
        );
        let v = h.read(Range::All, h.t + 10_000);
        let updating = |id: &str| by_id(&v, id)["updating"].clone();
        assert_eq!(
            (updating("r:A"), updating("r:B")),
            (json!(true), json!(false))
        );
    }

    /// The design's 4, claude-mem's `statusFor` and its lifecycle: needs you (and its aliases),
    /// blocked, ready only on the request's own recorded completion, in progress, to do, and no
    /// recent update after 6 quiet hours; work that is all done is "Update recorded".
    #[test]
    fn each_status_comes_from_the_recorded_work_as_claude_mems_does() {
        let mut h = Home::new();
        let cases = [
            ("needs", json!({"task": "t", "status": "Needs_Input"})),
            ("blocked", json!({"task": "t", "status": "failed"})),
            ("unknown", json!({"task": "t", "status": "done"})),
            (
                "ready",
                json!({"task": "t", "status": "done", "request_complete": true}),
            ),
            (
                "ready",
                json!({"task": "t", "status": "doing", "request_status": "shipped"}),
            ),
            ("working", json!({"task": "t", "status": "in progress"})),
            ("todo", json!({"task": "t", "status": "planned"})),
            ("unknown", json!({"task": "t", "status": "someday"})),
            (
                "blocked",
                json!({"task": "t", "status": "doing", "request_status": "failed"}),
            ),
            (
                "working",
                json!({"task": "t", "status": "todo", "request_status": "Running"}),
            ),
        ];
        for (i, (_, fields)) in cases.iter().enumerate() {
            let session = format!("s{i}");
            h.said(&session, -60_000, "Do the thing.");
            h.wrote(&session, &format!("list {i}"), fields.clone());
        }
        let v = h.read(Range::All, h.t + 10_000);
        for (i, (status, _)) in cases.iter().enumerate() {
            assert_eq!(
                by_id(&v, &format!("c:claude:s{i}"))["status"],
                *status,
                "{i}"
            );
        }
        let later = h.read(Range::All, h.t + QUIET_MS + 60_000);
        assert_eq!(by_id(&later, "c:claude:s5")["status"], "quiet");
        assert_eq!(by_id(&later, "c:claude:s2")["status"], "quiet");
        assert_eq!(by_id(&later, "c:claude:s0")["status"], "needs");
    }

    /// Of two writes in one millisecond, the later one's item is the newer, as claude-mem reads its
    /// events newest first: the stable sort by time alone kept the first written first (the CI
    /// coverage run met it in `the_newest_record_of_the_request_decides_and_its_id_is_one_text`).
    #[test]
    fn of_two_writes_in_one_millisecond_the_later_is_the_newer() {
        let write = |task: &str| WorkWrite {
            repo: R.into(),
            list: "docs".into(),
            fields: json!({"task": task, "status": "doing"})
                .as_object()
                .unwrap()
                .clone(),
            ts: 1_000,
        };
        let work = fold(vec![write("a"), write("b")]);
        let tasks: Vec<&str> = work.iter().filter_map(|w| w.task.as_deref()).collect();
        assert_eq!(tasks, ["b", "a"]);
    }

    /// #570 (CodeRabbit on #573): the request's own status is the newest record of its items'
    /// `request_status` or `request_complete`, whatever an earlier one said; and a `request_id`
    /// written as a number and as text is one request, across sessions too.
    #[test]
    fn the_newest_record_of_the_request_decides_and_its_id_is_one_text() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "R", "request_status": "done"}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "R", "request_status": "blocked"}),
        );
        h.said("s2", -50_000, "Fix the docs.");
        h.said("s3", -40_000, "And their index.");
        h.wrote(
            "s2",
            "docs",
            json!({"task": "a", "request_id": 7, "status": "doing"}),
        );
        h.wrote(
            "s3",
            "docs",
            json!({"task": "b", "request_id": "7", "status": "doing"}),
        );
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(by_id(&v, "r:R")["status"], "blocked");
        let seven = by_id(&v, "r:7");
        assert_eq!(tasks(seven), ["b", "a"]);
        assert_eq!(seven["ask"]["text"], "Fix the docs.");
    }

    /// claude-mem's lifecycle records: each recorded completion or status is dated by the write
    /// that set it, its own item's or its list's, not by its item's newest write, so a later
    /// write of another field changes nothing, and a newer status of the same item outranks an
    /// older completion (Greptile on #574).
    #[test]
    fn a_request_status_is_dated_by_the_write_that_set_it() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "R", "request_status": "done"}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "R", "request_status": "blocked"}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "next_step": "Post them."}),
        );
        h.said("s2", -50_000, "Ship the docs.");
        h.wrote(
            "s2",
            "docs",
            json!({"task": "site", "request_id": "Q", "request_complete": true}),
        );
        h.wrote(
            "s2",
            "docs",
            json!({"task": "site", "request_status": "blocked"}),
        );
        // Both in one write: the completion comes first.
        h.said("s3", -40_000, "Ship the site.");
        h.wrote(
            "s3",
            "site",
            json!({"task": "deploy", "request_id": "P", "request_status": "blocked",
                "request_complete": true}),
        );
        // A task's status from its list's state is dated by the list's write.
        h.said("s4", -30_000, "Ship the app.");
        h.wrote("s4", "app", json!({"request_status": "blocked"}));
        h.wrote(
            "s4",
            "app",
            json!({"task": "build", "request_id": "L", "request_status": "done"}),
        );
        h.wrote("s4", "app", json!({"task": "sign", "request_id": "L"}));
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(by_id(&v, "r:R")["status"], "blocked");
        assert_eq!(by_id(&v, "r:Q")["status"], "blocked");
        assert_eq!(by_id(&v, "r:P")["status"], "ready");
        assert_eq!(by_id(&v, "r:L")["status"], "ready");
    }

    /// Of two statuses set at the same millisecond, the later write's decides, as claude-mem
    /// orders its records by where they were written (`compareSourceOrder`).
    #[test]
    fn of_two_statuses_set_at_once_the_later_write_decides() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "R", "request_status": "done"}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "R", "request_status": "blocked"}),
        );
        rusqlite::Connection::open(h.s.home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE ops SET ts = (SELECT MIN(ts) FROM ops WHERE type = 'work_state')
                 WHERE type = 'work_state'",
                [],
            )
            .unwrap();
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(by_id(&v, "r:R")["status"], "blocked");
    }

    /// The design's 3: the card's copy is the session summaries: the title the first reply's
    /// `request`, the summary the latest reply's `completed`, else `learned`, else
    /// `investigated`, the next step the latest `next_steps`, each cut at a word; a prompt after
    /// the newest summary marks it updating. Without a summary, the recorded wording.
    #[test]
    fn the_copy_is_the_session_summaries_else_the_recorded_wording() {
        let mut h = Home::new();
        let p = h.said(
            "s1",
            -60_000,
            "Make the parser report the line of each error.",
        );
        let r = h.reply("s1", -55_000);
        h.turn(
            "s1",
            (p, r),
            -55_000,
            json!({"request": "Parser errors with line numbers",
                "investigated": "Read the lexer.", "next_steps": "Add a test."}),
        );
        let p = h.said("s1", -50_000, "And the column.");
        let r = h.reply("s1", -45_000);
        let learned = "The lexer counts bytes. ".repeat(20);
        h.turn(
            "s1",
            (p, r),
            -45_000,
            json!({"request": "Columns too", "learned": learned,
                "investigated": "Read the column code."}),
        );
        let v = h.read(Range::All, h.t);
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["title"], "Parser errors with line numbers");
        let summary = s1["summary"].as_str().unwrap();
        assert!(summary.starts_with("The lexer counts bytes."), "{summary}");
        assert!(
            summary.ends_with('…') && summary.chars().count() <= SUMMARY,
            "{summary}"
        );
        assert_eq!(s1["next"], "Add a test.");
        assert_eq!(
            (&s1["copy"], &s1["updating"]),
            (&json!("summary"), &json!(false))
        );
        h.said("s1", -40_000, "One more thing.");
        let v = h.read(Range::All, h.t);
        assert_eq!(by_id(&v, "c:claude:s1")["updating"], true);
        let q = h.said(
            "s2",
            -30_000,
            "Speed up the index. It is slow on cold caches.",
        );
        h.s.cards(
            q,
            q,
            json!([{"type": "discovery", "title": "Cold reads",
                "subtitle": "The first read takes 3 s.", "narrative": "x"}]),
            false,
        );
        let v = h.read(Range::All, h.t);
        let s2 = by_id(&v, "c:claude:s2");
        assert_eq!(s2["title"], "Speed up the index.");
        assert_eq!(s2["summary"], "The first read takes 3 s.");
        assert_eq!(s2["copy"], "recorded");
        // A work item's recorded request titles it before the ask does.
        h.said("s3", -20_000, "Look at search.");
        h.wrote(
            "s3",
            "perf",
            json!({"task": "cold", "request": "Make cold reads fast", "status": "doing"}),
        );
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(by_id(&v, "c:claude:s3")["title"], "Make cold reads fast");
    }

    /// claude-mem's `updates` (`presentWork`): each work item's outcome, else its progress, else
    /// its name, and each card's subtitle, else its title, the newest first, each text once, at
    /// most 8, each cut to 240, whichever copy the request has; without a summary the newest is
    /// the card's summary.
    #[test]
    fn the_updates_are_the_newest_eight_of_the_work_and_the_cards_each_once() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "Ship the release.");
        let r = h.reply("s1", -55_000);
        let shipped = json!({"request": "Release", "completed": "Shipped."});
        h.turn("s1", (p, r), -55_000, shipped);
        for i in 1..=9 {
            let fields = json!({"task": format!("step {i}"), "status": "done",
                "outcome": format!("Step {i} done.")});
            h.wrote("s1", "release", fields);
        }
        let again = json!({"task": "again", "status": "done", "outcome": "Step 9 done."});
        h.wrote("s1", "release", again);
        let q = h.said("s2", -50_000, "Speed up search.");
        let cold = json!([{"type": "discovery", "title": "Cold reads",
            "subtitle": "The first read takes 3 s.", "narrative": "x"}]);
        h.s.cards(q, q, cold, false);
        let t = (h.s).event(
            "tool",
            "s2",
            (R, "main"),
            h.t - 45_000,
            json!({"tool": "Bash"}),
        );
        let rebuilt = json!([{"type": "change", "title": "Index rebuilt", "subtitle": " ", "narrative": "x"}]);
        h.s.cards(t, t, rebuilt, false);
        let long = "The index is rebuilt in the background. ".repeat(10);
        h.wrote(
            "s2",
            "search",
            json!({"task": "rebuild", "status": "doing", "progress": long}),
        );
        h.wrote(
            "s2",
            "search",
            json!({"task": "tag", "status": "doing", "outcome": "Tagged.", "progress": "Tagging."}),
        );
        h.wrote("s2", "search", json!({"task": "publish", "status": "todo"}));
        // A name that is a number alone says nothing, as claude-mem's `cleanText` reads one.
        h.wrote("s2", "search", json!({"task": "7", "status": "todo"}));
        h.wrote("s2", "search", json!({"status": "doing"}));
        let v = h.read(Range::All, h.t + 10_000);
        let updates = |id: &str| by_id(&v, id)["updates"].as_array().unwrap().clone();
        let texts = |id: &str| -> Vec<String> {
            (updates(id).iter())
                .map(|u| u["text"].as_str().unwrap().to_owned())
                .collect()
        };
        let steps: Vec<String> = (2..=9).rev().map(|i| format!("Step {i} done.")).collect();
        assert_eq!(texts("c:claude:s1"), steps);
        assert_eq!(by_id(&v, "c:claude:s1")["summary"], "Shipped.");
        let s2 = texts("c:claude:s2");
        assert_eq!(s2.len(), 6, "{s2:?}");
        assert_eq!(s2[..3], ["search", "publish", "Tagged."]);
        assert!(
            s2[3].starts_with("The index is rebuilt") && s2[3].ends_with('…'),
            "{s2:?}"
        );
        assert!(s2[3].chars().count() <= SUMMARY, "{s2:?}");
        assert_eq!(s2[4..], ["Index rebuilt", "The first read takes 3 s."]);
        let s2r = by_id(&v, "c:claude:s2");
        assert_eq!(
            (&s2r["copy"], &s2r["summary"]),
            (&json!("recorded"), &json!(s2[0]))
        );
        for id in ["c:claude:s1", "c:claude:s2"] {
            let ts: Vec<i64> = (updates(id).iter())
                .map(|u| u["ts"].as_i64().unwrap())
                .collect();
            assert!(ts.windows(2).all(|w| w[0] >= w[1]), "{ts:?}");
        }
    }

    /// A work item's detail, its recorded change on the request's page (claude-mem's
    /// `request-views.js`): its outcome, else its progress, cut to 240; none for an item with
    /// neither, one with a next step alone too.
    #[test]
    fn a_work_items_detail_is_its_outcome_else_its_progress() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Ship the release.");
        let long = "The changelog is long. ".repeat(20);
        h.wrote(
            "s1",
            "release",
            json!({"task": "changelog", "status": "doing", "progress": long}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "status": "doing", "outcome": "Tagged.", "progress": "Tagging."}),
        );
        h.wrote(
            "s1",
            "release",
            json!({"task": "publish", "status": "todo", "next_step": "Publish it."}),
        );
        let v = h.read(Range::All, h.t + 10_000);
        let work = by_id(&v, "c:claude:s1")["work"].as_array().unwrap().clone();
        let detail =
            |task: &str| work.iter().find(|w| w["task"] == task).unwrap()["detail"].clone();
        assert_eq!(
            (detail("tag"), detail("publish")),
            (json!("Tagged."), Value::Null)
        );
        let changelog = detail("changelog");
        let changelog = changelog.as_str().unwrap();
        assert!(
            changelog.starts_with("The changelog is long.") && changelog.ends_with('…'),
            "{changelog}"
        );
        assert!(changelog.chars().count() <= SUMMARY, "{changelog}");
    }

    /// The design's 1, beyond claude-mem's rule: a session none of whose prompts asks (resumed with
    /// an acknowledgement) takes its oldest reply's `request` as its ask, and all of its prompts
    /// follow it up.
    #[test]
    fn a_session_resumed_with_an_acknowledgement_asks_what_its_first_reply_says() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "再開して");
        let r = h.reply("s1", -55_000);
        let summary = h.turn(
            "s1",
            (p, r),
            -55_000,
            json!({"request": "Resume the parser fix", "completed": "Fixed."}),
        );
        let p = h.said("s1", -50_000, "続けて");
        let r = h.reply("s1", -45_000);
        h.turn(
            "s1",
            (p, r),
            -45_000,
            json!({"request": "Add the regression test", "completed": "Added."}),
        );
        let v = h.read(Range::All, h.t);
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(
            s1["ask"],
            json!({"id": summary, "ts": h.t - 60_000, "text": "Resume the parser fix",
                "from": "summary"})
        );
        assert_eq!(s1["follow_ups"][0]["text"], "再開して");
        assert_eq!(s1["title"], "Resume the parser fix");
        assert_eq!(v["unlinked"], 0);
    }

    /// Every text goes through the gate as its rules are now, before its cut: a value a rule added
    /// after the prompts were kept, which the worker's rescan has not masked yet, is masked in the
    /// ask and the follow-ups, also where the ask's cut at 1,200 characters splits it.
    #[test]
    fn a_value_a_rule_masks_is_masked_also_where_the_cut_splits_it() {
        let mut h = Home::new();
        let long = format!("{}Use the canary-4827 key.", "Fix the deploy. ".repeat(74));
        h.said("s1", -60_000, &long);
        h.said("s1", -50_000, "Use canary-4827 again.");
        h.s.run();
        let config =
            "[redaction]\nextra_rules = [{ id = \"canary\", regex = 'canary-[0-9]{4}' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t).unwrap();
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["ask"]["text"].as_str().unwrap().chars().count(), ASK);
        assert_eq!(s1["follow_ups"].as_array().unwrap().len(), 1);
        assert!(!v.to_string().contains("canary-"), "{v}");
    }

    /// Work state's names and values go out as its own lines show them, each beside its key: a
    /// rule added after the write that matches a value only with its key masks it (the commit
    /// security review of this slice).
    #[test]
    fn a_work_value_a_rule_masks_beside_its_key_is_masked() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Rotate the keys.");
        h.wrote(
            "s1",
            "keys",
            json!({"task": "canary-4827", "status": "doing", "outcome": "canary-1234 rotated"}),
        );
        // A task's name is its update when it records no outcome or progress, and a progress is.
        h.wrote(
            "s1",
            "keys",
            json!({"task": "canary-5555", "status": "todo"}),
        );
        h.wrote(
            "s1",
            "keys",
            json!({"task": "revoke", "status": "doing", "progress": "canary-9999 next"}),
        );
        h.s.run();
        let config = "[redaction]\nextra_rules = [{ id = \"t\", regex = 'task = \"canary-[0-9]{4}' }, \
                      { id = \"o\", regex = 'outcome = \"canary-[0-9]{4}' }, \
                      { id = \"p\", regex = 'progress = \"canary-[0-9]{4}' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let s1 = by_id(&v, "c:claude:s1");
        let mask = json!(crate::redact::MASK);
        let work = s1["work"].as_array().unwrap();
        let tasks: Vec<&Value> = work.iter().map(|w| &w["task"]).collect();
        assert_eq!(tasks, [&json!("revoke"), &mask, &mask]);
        let details: Vec<&Value> = work.iter().map(|w| &w["detail"]).collect();
        assert_eq!(details, [&mask, &Value::Null, &mask]);
        // Masked alike, the three updates are one text, shown once.
        let updates = s1["updates"].as_array().unwrap();
        assert_eq!((updates.len(), &updates[0]["text"]), (1, &mask));
        assert_eq!(s1["summary"], crate::redact::MASK);
        assert!(!v.to_string().contains("canary-"), "{v}");
    }

    /// A list's name goes out beside its key too, as capture keeps it (`capture::work_state`): a
    /// rule added after the write that matches it only with its key masks it (Codex on #574).
    #[test]
    fn a_list_name_a_rule_masks_beside_its_key_is_masked() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Rotate the keys.");
        h.wrote(
            "s1",
            "canary-4827",
            json!({"task": "rotate", "status": "doing"}),
        );
        h.s.run();
        let config =
            "[redaction]\nextra_rules = [{ id = \"l\", regex = 'list = \"canary-[0-9]{4}' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["work"][0]["list"], crate::redact::MASK);
        assert!(!v.to_string().contains("canary-"), "{v}");
    }

    /// A closing status is kept as sent (`capture::work_state`), and goes out beside its key as
    /// work state's own lines show it: a rule added after the write that matches it only with its
    /// key masks it, and the item is still closed (Codex on #574).
    #[test]
    fn a_closing_status_a_rule_masks_beside_its_key_is_masked() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Rotate the keys.");
        h.wrote("s1", "keys", json!({"task": "rotate", "status": "done"}));
        h.s.run();
        let config = "[redaction]\nextra_rules = [{ id = \"s\", regex = 'status = \"done' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["work"][0]["status"], crate::redact::MASK);
        assert_eq!(s1["status"], "unknown");
    }

    /// The design's 3 (#570's measurement): a read makes no provider call.
    #[test]
    fn a_read_makes_no_provider_call() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let calls = |h: &Home| -> i64 {
            crate::providers_db::open(h.s.home.path())
                .unwrap()
                .query_row("SELECT count(*) FROM provider_calls", [], |r| r.get(0))
                .unwrap()
        };
        let before = calls(&h);
        assert_eq!(ids(&h.read(Range::All, h.t)), ["c:claude:s1"]);
        assert_eq!(calls(&h), before);
    }

    /// The range is the newest update's: a day by default, a week, or all; a session in an
    /// excluded repository and a prompt a forget took are not shown.
    #[test]
    fn the_range_the_exclusions_and_a_forget_decide_what_is_shown() {
        let mut h = Home::new();
        h.said("old", -3 * 86_400_000, "Fix the old thing.");
        let gone = h.said("s1", -60_000, "Delete the canary-4827 note.");
        h.said("s1", -50_000, "Fix the parser.");
        h.s.said(
            "x",
            "github.com/o/private",
            h.t - 60_000,
            "Fix the private thing.",
        );
        h.s.exclude("github.com/o/private");
        let device = h.s.raw.device().to_owned();
        (h.s.raw)
            .append_tombstone(Target::Record { device, seq: gone })
            .unwrap();
        assert_eq!(ids(&h.read(Range::Day, h.t)), ["c:claude:s1"]);
        assert_eq!(
            ids(&h.read(Range::Week, h.t)),
            ["c:claude:s1", "c:claude:old"]
        );
        let v = h.read(Range::Day, h.t);
        assert_eq!(by_id(&v, "c:claude:s1")["ask"]["text"], "Fix the parser.");
        assert!(!v.to_string().contains("canary-4827"), "{v}");
    }

    /// The design's 1 and 5: an excluded repository's work, and the work an excluded session
    /// wrote elsewhere, take no part in the `WORK` items read either, so they hide no older work
    /// (Codex on #574).
    #[test]
    fn excluded_work_takes_no_part_in_the_bound() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        h.wrote("s1", "plan", json!({"task": "parser", "status": "doing"}));
        // So that every later write is newer than this one.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let private = "github.com/o/private";
        h.s.said("x", private, h.t - 60_000, "Fix the private thing.");
        for i in 0..WORK {
            let f = json!({"task": format!("p{i}"), "status": "todo"});
            let f = f.as_object().unwrap();
            h.s.raw.work_state(private, "plan", f, None).unwrap();
            h.wrote(
                "x",
                "plan",
                json!({"task": format!("x{i}"), "status": "todo"}),
            );
        }
        h.s.exclude(private);
        let v = h.read(Range::All, h.t + 60_000);
        assert_eq!(ids(&v), ["c:claude:s1"]);
        assert_eq!(tasks(by_id(&v, "c:claude:s1")), ["parser"]);
        assert_eq!(v["unlinked"], 0);
    }

    /// The design's 1 and 5: an excluded session's prompts, summaries and cards take no part in
    /// the `PROMPTS`, `SUMMARIES` and `CARDS` read either, so newer ones push out no session found
    /// by its prompts, nor a request's ask, summary or activity (Codex on #574).
    #[test]
    fn excluded_sessions_take_no_part_in_the_readers_bounds() {
        let mut h = Home::new();
        h.said("s0", -61_000, "Fix the lexer.");
        let p = h.said("s1", -60_000, "Fix the parser.");
        let r = h.reply("s1", -59_000);
        let summary = h.turn(
            "s1",
            (p, r),
            -59_000,
            json!({"request": "Parser work", "completed": "Fixed."}),
        );
        let card = h.s.cards(
            p,
            p,
            json!([{"type": "change", "title": "Parser fixed", "narrative": "x"}]),
            false,
        );
        let (private, t) = ("github.com/o/private", h.t);
        let x: Vec<i64> = (0..PROMPTS as i64)
            .map(|i| {
                h.s.said("x", private, t - 50_000 + i, "Fix the private thing.")
            })
            .collect();
        for i in 0..SUMMARIES as i64 {
            let f = json!({"request": "Private work", "completed": "Done."});
            h.turn("x", (x[0], x[0]), -50_000 + i, f);
        }
        let many = vec![json!({"type": "change", "title": "Private", "narrative": "x"}); CARDS];
        h.s.cards(x[0], x[0], Value::Array(many), false);
        h.s.exclude(private);
        let v = h.read(Range::Day, h.t);
        assert_eq!(ids(&v), ["c:claude:s1", "c:claude:s0"]);
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(
            (&s1["ask"]["from"], &s1["ask"]["text"]),
            (&json!("prompt"), &json!("Fix the parser."))
        );
        assert_eq!(s1["summaries"], json!([summary]));
        assert_eq!(s1["cards"], json!(card));
    }

    /// The design's 5: a prompt a forget took, before the purge rewrites its row, or one whose text
    /// expired takes no part in the `PROMPTS` that find a session of prompts alone either, so
    /// newer ones push out no older session (Codex on #574).
    #[test]
    fn hidden_prompts_take_no_part_in_the_prompts_bound() {
        let mut h = Home::new();
        h.said("s0", -61_000, "Fix the lexer.");
        for i in 0..PROMPTS as i64 {
            h.said("x", -50_000 + i, "Fix the old thing.");
        }
        let gone: Vec<i64> = (0..PROMPTS as i64)
            .map(|i| h.said("y", -40_000 + i, "Fix the gone thing."))
            .collect();
        let device = h.s.raw.device().to_owned();
        for seq in gone {
            let device = device.clone();
            (h.s.raw)
                .append_tombstone(Target::Record { device, seq })
                .unwrap();
        }
        // As retention leaves an expired record (docs/retention.md).
        rusqlite::Connection::open(h.s.home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET enc = 'plain', body = X'', original_bytes = NULL, expired = 1
                 WHERE type = 'event' AND session = 'x'",
                [],
            )
            .unwrap();
        assert_eq!(ids(&h.read(Range::Day, h.t)), ["c:claude:s0"]);
    }

    /// The design's 5: a reader that pages on past an excluded session's records reads its bound
    /// and no more, each record once: here the `SUMMARIES` newest of a session's own.
    #[test]
    fn a_reader_that_pages_on_reads_its_bound_once() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "Fix the parser.");
        let r = h.reply("s1", -59_000);
        let f = json!({"request": "Parser work", "completed": "Fixed."});
        let mut s1: Vec<String> = (0..SUMMARIES as i64)
            .map(|i| h.turn("s1", (p, r), -50_000 + i, f.clone()))
            .collect();
        let private = "github.com/o/private";
        let x =
            h.s.said("x", private, h.t - 40_000, "Fix the private thing.");
        for i in 0..SUMMARIES as i64 - 1 {
            h.turn("x", (x, x), -40_000 + i, f.clone());
        }
        s1.push(h.turn("s1", (p, r), -30_000, f));
        h.s.exclude(private);
        let v = h.read(Range::Day, h.t);
        s1.reverse();
        s1.truncate(SUMMARIES);
        assert_eq!(by_id(&v, "c:claude:s1")["summaries"], json!(s1));
    }

    /// The design's 5: a card of no session, whose window spans two sessions' records, is no
    /// request's, so it takes no part in the `CARDS` read either (Codex on #574).
    #[test]
    fn a_card_of_no_session_takes_no_part_in_the_bound() {
        let mut h = Home::new();
        let p = h.said("s1", -60_000, "Fix the parser.");
        let card = h.s.cards(
            p,
            p,
            json!([{"type": "change", "title": "Parser fixed", "narrative": "x"}]),
            false,
        );
        let a = h.said("s2", -50_000, "Look at the lexer.");
        let b = h.said("s3", -49_000, "Look at the index.");
        let many = vec![json!({"type": "change", "title": "Both", "narrative": "x"}); CARDS];
        h.s.cards(a, b, Value::Array(many), false);
        let v = h.read(Range::Day, h.t);
        assert_eq!(by_id(&v, "c:claude:s1")["cards"], json!(card));
    }

    /// claude-mem's `readAgentRecords`: every prompt of a session read for its work, a summary or
    /// a card is read, however many newer prompts other sessions have, so it keeps its request,
    /// its own first ask and its follow-ups; `PROMPTS` bounds only the sessions found by their
    /// prompts alone (Codex on #574).
    #[test]
    fn a_session_read_has_every_prompt_read_however_many_are_newer() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        h.wrote("s1", "plan", json!({"task": "parser", "status": "doing"}));
        let p = h.said("s3", -59_000, "Fix the docs.");
        h.s.cards(
            p,
            p,
            json!([{"type": "change", "title": "Docs fixed", "narrative": "x"}]),
            false,
        );
        h.said("s4", -58_000, "Fix the index.");
        h.said("s5", -57_000, "Fix the cache.");
        for i in 0..PROMPTS as i64 {
            h.said("s2", -50_000 + i, &format!("Look at the lexer, part {i}."));
        }
        h.said("s4", -10_000, "And its tests.");
        let v = h.read(Range::All, h.t + 10_000);
        let s4 = by_id(&v, "c:claude:s4");
        let follow: Vec<&str> = (s4["follow_ups"].as_array().unwrap().iter())
            .map(|f| f["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            (&s4["ask"]["text"], follow),
            (&json!("Fix the index."), vec!["And its tests."])
        );
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(
            (&s1["ask"]["text"], tasks(s1)),
            (&json!("Fix the parser."), vec!["parser"])
        );
        assert_eq!(by_id(&v, "c:claude:s3")["ask"]["text"], "Fix the docs.");
        assert!(
            !ids(&v).contains(&"c:claude:s5".to_owned()),
            "{:?}",
            ids(&v)
        );
        assert_eq!(v["unlinked"], 0);
    }

    /// claude-mem's `buildHumanRequests`: a work item whose write no single call links, with a
    /// `request_id` and a recorded request, is a request of its own on that wording
    /// (`explicit-work-state-request`); one without an id is unlinked and counted (Codex on #574).
    #[test]
    fn an_unlinked_item_with_a_request_id_and_a_recorded_request_is_its_own_request() {
        let mut h = Home::new();
        h.write(
            "plan",
            json!({"task": "a", "request_id": "R", "request": "Fix the release notes",
                "status": "doing"}),
        );
        h.write("plan", json!({"task": "b", "status": "doing"}));
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(ids(&v), ["r:R"]);
        let r = by_id(&v, "r:R");
        assert_eq!(
            (&r["ask"]["from"], &r["ask"]["text"]),
            (&json!("work"), &json!("Fix the release notes"))
        );
        assert_eq!(tasks(r), ["a"]);
        assert_eq!(v["unlinked"], 1);
    }

    /// The design's 1: an item no call ties to a session may be an excluded session's, so while a
    /// session is excluded it is not read; before the fts consumer has made `raw_docs_tools` no
    /// item is tied to one, so then no work is read (Codex on #574).
    #[test]
    fn work_of_no_known_session_is_not_read_while_a_session_is_excluded() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let private = "github.com/o/private";
        h.s.said("x", private, h.t - 60_000, "Fix the private thing.");
        h.wrote(
            "x",
            "plan",
            json!({"task": "p", "request_id": "P", "request": "Fix the private thing",
                "status": "doing"}),
        );
        h.write(
            "loose",
            json!({"task": "q", "request_id": "Q", "request": "Fix the loose thing",
                "status": "doing"}),
        );
        let unindexed = |h: &Home| {
            h.s.run();
            (crate::knowledge::open(h.s.home.path()).unwrap())
                .execute_batch("DROP INDEX raw_docs_tools")
                .unwrap();
            super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap()
        };
        let v = unindexed(&h);
        assert!(ids(&v).contains(&"r:P".into()) && ids(&v).contains(&"r:Q".into()));
        h.s.exclude(private);
        for v in [h.read(Range::All, h.t + 10_000), unindexed(&h)] {
            assert_eq!(ids(&v), ["c:claude:s1"]);
            assert_eq!(v["unlinked"], 0);
        }
    }

    /// The design's 1: an item shows what each of its writes left, and a task the state of its
    /// list too, so while a session is excluded it is read only when a call of a session not
    /// excluded, and none of an excluded one's, sent each of those writes: an excluded session's
    /// write, of which capture keeps no call, may be newer than another session's linked one or
    /// between two of them (Codex on #574).
    #[test]
    fn an_item_is_read_only_when_no_write_it_shows_may_be_an_excluded_sessions() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let private = "github.com/o/private";
        h.s.said("x", private, h.t - 60_000, "Fix the private thing.");
        // A newer write no call sent.
        h.wrote("s1", "plan", json!({"task": "parser", "status": "doing"}));
        h.write("plan", json!({"task": "parser", "status": "canary-1"}));
        // The excluded session's write between two of s1's.
        h.wrote("s1", "plan", json!({"task": "lexer", "status": "doing"}));
        h.wrote(
            "x",
            "plan",
            json!({"task": "lexer", "next_step": "canary-2"}),
        );
        h.wrote("s1", "plan", json!({"task": "lexer", "status": "blocked"}));
        // The state of a task's list, which its request fields fall back to.
        h.wrote("s1", "fixes", json!({"task": "index", "status": "doing"}));
        h.write("fixes", json!({"request": "Fix the canary-3 thing"}));
        // A write both sessions' calls sent.
        h.wrote("s1", "plan", json!({"task": "both", "status": "doing"}));
        let (ts, f) = h.write("plan", json!({"task": "both", "status": "canary-4"}));
        h.call("s1", ts + 20, "plan", &f);
        h.call("x", ts + 30, "plan", &f);
        h.wrote("s1", "own", json!({"task": "kept", "status": "doing"}));
        let sorted = |v: &Value| {
            let mut t = tasks(by_id(v, "c:claude:s1"));
            t.sort_unstable();
            t.into_iter().map(str::to_owned).collect::<Vec<_>>()
        };
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(sorted(&v), ["both", "index", "kept", "lexer", "parser"]);
        for canary in ["canary-1", "canary-2", "canary-3", "canary-4"] {
            assert!(v.to_string().contains(canary), "{canary} in {v}");
        }
        h.s.exclude(private);
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(ids(&v), ["c:claude:s1"]);
        assert_eq!(sorted(&v), ["kept"]);
        assert_eq!(v["unlinked"], 0);
        assert!(!v.to_string().contains("canary"), "{v}");
    }

    /// A session is known by its labels as the stores hold them, and the gate only shows them:
    /// two sessions a rule added later masks alike stay two requests, with their own summaries,
    /// cards and work, and neither takes an excluded session's exclusion (Codex on #574).
    #[test]
    fn sessions_the_gate_labels_alike_stay_apart() {
        let mut h = Home::new();
        let p = h.said("sess-1", -60_000, "Fix the parser.");
        let r = h.reply("sess-1", -59_000);
        let summary = h.turn(
            "sess-1",
            (p, r),
            -59_000,
            json!({"request": "Parser work", "completed": "Fixed."}),
        );
        h.wrote(
            "sess-1",
            "plan",
            json!({"task": "parser", "status": "doing"}),
        );
        let q = h.said("sess-2", -50_000, "Fix the lexer.");
        let card = h.s.cards(
            q,
            q,
            json!([{"type": "change", "title": "Lexer fixed", "narrative": "x"}]),
            false,
        );
        let private = "github.com/o/private";
        h.s.said("sess-3", private, h.t - 40_000, "Fix the private thing.");
        h.s.exclude(private);
        h.s.run();
        let rule =
            "[redaction]\nextra_rules = [{ id = \"id\", regex = '^(claude|sess-[0-9]+)$' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let shown = v["requests"].as_array().unwrap();
        assert_eq!(shown.len(), 2, "{v}");
        assert_eq!(shown[0]["ask"]["text"], "Fix the parser.");
        assert_eq!(
            (tasks(&shown[0]), &shown[0]["summaries"]),
            (vec!["parser"], &json!([summary]))
        );
        assert_eq!(
            (&shown[1]["ask"]["text"], &shown[1]["cards"]),
            (&json!("Fix the lexer."), &json!(card))
        );
        let first = ids(&v);
        assert_ne!(first[0], first[1]);
        for (id, r) in first.iter().zip(shown) {
            assert!(id.starts_with("c:[REDACTED]:[REDACTED]#") && !id.contains("sess-"));
            assert_eq!(r["agents"], json!(["[REDACTED]"]));
            // Its conversation is named by the id its own request has, the fingerprint too.
            let session = json!({"agent": "[REDACTED]", "id": id, "fingerprint": r["fingerprint"]});
            assert_eq!(r["sessions"], json!([session]));
        }
    }

    /// Decision 55 (claude-mem's agent view, one card a conversation): a request names the
    /// conversations it owns, the newest activity first, each by its own records of the ones its
    /// `updated` counts (a summary, a card, a prompt, a write), and by the id its own request has
    /// or would have: a request of several conversations each, and two requests of one
    /// conversation that one.
    #[test]
    fn a_request_names_its_conversations_by_their_own_ids_the_newest_first() {
        let mut h = Home::new();
        // Of `rel-1`'s conversations, in the order the stores hold them, the newest record of
        // `s1` is a card, of `s2` a summary, of `s3` and `s4` a prompt, `s4`'s the later, and of
        // `s5` and `s6` a write, `s6`'s the later: a record of one counts for no other.
        h.said("s1", -90_000, "Check the links.");
        let t = (h.s).event(
            "tool",
            "s1",
            (R, "main"),
            h.t + 50_000,
            json!({"tool": "Bash"}),
        );
        let checked = json!([{"type": "change", "title": "Links checked", "narrative": "x"}]);
        h.s.cards(t, t, checked, false);
        let p = h.said("s2", -90_000, "Draft the release notes.");
        let r = h.reply("s2", -89_000);
        let drafted = json!({"request": "Notes", "completed": "Drafted."});
        h.turn("s2", (p, r), 40_000, drafted);
        h.said("s3", 10_000, "Check the dates.");
        h.said("s4", 30_000, "Check the names.");
        h.said("s5", -60_000, "Write the release notes.");
        h.said("s6", -95_000, "Check the release notes.");
        for s in ["s1", "s2", "s3", "s4", "s5", "s6"] {
            // Each write a millisecond of its own, which its item is dated by.
            std::thread::sleep(std::time::Duration::from_millis(2));
            let fields = json!({"task": s, "status": "doing", "request_id": "rel-1"});
            h.wrote(s, "notes", fields);
        }
        h.said("t1", -50_000, "Fix the parser.");
        h.said("t2", -40_000, "Fix the lexer and the docs.");
        h.wrote(
            "t2",
            "lexer",
            json!({"task": "lexer", "status": "doing", "request_id": "a",
                "request": "Fix the lexer"}),
        );
        h.wrote(
            "t2",
            "docs",
            json!({"task": "docs", "status": "doing", "request_id": "b",
                "request": "Fix the docs"}),
        );
        let v = h.read(Range::All, h.t + 120_000);
        let of = |s: &str| {
            let id = format!("c:claude:{s}");
            json!({"agent": "claude", "id": id, "fingerprint": null})
        };
        let sessions = |id: &str| by_id(&v, id)["sessions"].clone();
        let newest_first: Vec<Value> = ["s1", "s2", "s4", "s3", "s6", "s5"].map(of).to_vec();
        assert_eq!(sessions("r:rel-1"), json!(newest_first));
        assert_eq!(sessions("c:claude:t1"), json!([of("t1")]));
        assert_eq!(
            (sessions("r:a"), sessions("r:b")),
            (json!([of("t2")]), json!([of("t2")]))
        );
    }

    /// Rule 6: a request's version is the home-keyed hash of what was recorded for it: a summary a
    /// curator writes leaves it, a follow-up and a field of its work move it.
    #[test]
    fn a_requests_version_follows_what_was_recorded_not_what_a_curator_wrote() {
        let mut h = Home::new();
        let asked = h.said("s1", -60_000, "Fix the parser.");
        let replied = h.reply("s1", -55_000);
        let done = json!({"task": "parser", "status": "done", "request_status": "done"});
        h.wrote("s1", "fix", done);
        let version = |h: &Home| -> Value {
            let v = h.read(Range::All, h.t + 10_000);
            by_id(&v, "c:claude:s1")["version"].clone()
        };
        let first = version(&h);
        assert!(first.as_str().is_some_and(|v| v.len() == 64), "{first}");
        let unkeyed = crate::forget::hash(b"c:claude:s1");
        assert_ne!(first, json!(unkeyed));
        h.turn(
            "s1",
            (asked, replied),
            -50_000,
            json!({"completed": "Parser fixed."}),
        );
        assert_eq!(version(&h), first, "a summary leaves it");
        h.said("s1", -40_000, "And the docs.");
        let second = version(&h);
        assert_ne!(second, first, "a follow-up moves it");
        h.wrote("s1", "fix", json!({"task": "parser", "outcome": "Merged."}));
        let third = version(&h);
        assert_ne!(third, second, "a field of its work moves it");
        // Keyed: a new key, the same records, another version.
        std::fs::remove_file(h.s.home.path().join("state").join("my-work.key")).unwrap();
        assert_ne!(version(&h), third, "keyed by the home's key");
    }

    /// Rule 6: a check is of one version: shown while it is the request's, unread when a newer one
    /// is recorded; an Undo of another version leaves it; a write drops the checks of requests no
    /// longer read; a forget of its session takes it, of another session leaves it. The file holds
    /// no id, session or text of the store's.
    #[test]
    fn a_check_is_of_one_version_and_goes_with_its_session() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let done = json!({"task": "parser", "status": "done", "request_status": "done"});
        h.wrote("s1", "fix", done);
        h.said("s2", -50_000, "Fix the docs.");
        h.run_read();
        let home = h.s.home.path().to_owned();
        let now = h.t + 10_000;
        let reviewing = || super::reviewing(&home, now).unwrap();
        let at = |v: &Value, id: &str| -> usize {
            (v["requests"].as_array().unwrap().iter())
                .position(|r| r["id"] == id)
                .unwrap()
        };
        let (v, held) = reviewing();
        let (one, two) = (at(&v, "c:claude:s1"), at(&v, "c:claude:s2"));
        let version = v["requests"][one]["version"].as_str().unwrap().to_owned();
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[one], &held),
            &version,
            true,
            now,
        )
        .unwrap();
        let (v, held) = reviewing();
        assert_eq!(v["requests"][at(&v, "c:claude:s1")]["checked"], json!(now));
        assert_eq!(v["requests"][at(&v, "c:claude:s2")]["checked"], Value::Null);
        let file = std::fs::read_to_string(home.join("state").join(super::REVIEWS)).unwrap();
        for held_text in ["s1", "claude", "parser", "github"] {
            assert!(!file.contains(held_text), "{file}");
        }
        // An Undo of another version leaves the check.
        let other = "0".repeat(64);
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[one], &held),
            &other,
            false,
            now,
        )
        .unwrap();
        assert_eq!(reviewing().0["requests"][one]["checked"], json!(now));
        // A forget of another session leaves it, of its own takes it.
        let session = |s: &str| crate::forget::session("claude", s);
        super::forget_reviews(&home, &[session("s2")]).unwrap();
        assert_eq!(reviewing().0["requests"][one]["checked"], json!(now));
        super::forget_reviews(&home, &[session("s1")]).unwrap();
        assert_eq!(reviewing().0["requests"][one]["checked"], Value::Null);
        // A write drops the checks of requests no longer read.
        let (v, held) = reviewing();
        let two_version = v["requests"][two]["version"].as_str().unwrap().to_owned();
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[one], &held),
            &version,
            true,
            now,
        )
        .unwrap();
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[two], &held[two..=two]),
            &two_version,
            true,
            now,
        )
        .unwrap();
        let (v, _) = reviewing();
        assert_eq!(
            v["requests"][one]["checked"],
            Value::Null,
            "dropped as no longer read"
        );
        assert_eq!(v["requests"][two]["checked"], json!(now));
        // A newer version reads unchecked, and its Undo of the old version leaves nothing behind.
        h.said("s2", -40_000, "And the index.");
        h.run_read();
        let (v, held) = reviewing();
        assert_eq!(v["requests"][at(&v, "c:claude:s2")]["checked"], Value::Null);
        let two = at(&v, "c:claude:s2");
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[two], &held),
            &two_version,
            false,
            now,
        )
        .unwrap();
        let file = std::fs::read_to_string(home.join("state").join(super::REVIEWS)).unwrap();
        assert_eq!(file, "{}");
    }

    /// Codex on slice 3: a request status written again is a new version, though its value is not:
    /// a check of a result reported complete does not hide the blocked state a later write records.
    #[test]
    fn a_status_written_again_is_a_new_version() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let both = json!({"task": "parser", "status": "doing", "request_complete": true,
            "request_status": "blocked"});
        h.wrote("s1", "fix", both);
        h.run_read();
        let home = h.s.home.path().to_owned();
        let now = h.t + 10_000;
        let (v, held) = super::reviewing(&home, now).unwrap();
        assert_eq!(v["requests"][0]["status"], "ready", "{v}");
        let version = v["requests"][0]["version"].as_str().unwrap().to_owned();
        let lock = super::reviews_lock(&home).unwrap();
        super::set_review((&home, &lock), (&held[0], &held), &version, true, now).unwrap();
        drop(lock);
        // A write of its own millisecond.
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.wrote(
            "s1",
            "fix",
            json!({"task": "parser", "request_status": "blocked"}),
        );
        h.run_read();
        let (v, _) = super::reviewing(&home, now).unwrap();
        let r = &v["requests"][0];
        assert_eq!(
            (r["status"].as_str(), &r["checked"]),
            (Some("blocked"), &Value::Null),
            "{v}"
        );
    }

    /// Codex on slice 3: a forget's sweep waits for a check's hold, which runs from the read the
    /// check is tested against to its write, so that a stale check cannot bring back what the sweep
    /// takes.
    #[test]
    fn a_forgets_sweep_waits_for_a_checks_hold() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let done = json!({"task": "parser", "status": "done", "request_status": "done"});
        h.wrote("s1", "fix", done);
        h.run_read();
        let home = h.s.home.path().to_owned();
        let now = h.t + 10_000;
        let lock = super::reviews_lock(&home).unwrap();
        let (v, held) = super::reviewing(&home, now).unwrap();
        let version = v["requests"][0]["version"].as_str().unwrap().to_owned();
        let (at, session) = (home.clone(), crate::forget::session("claude", "s1"));
        let sweep = std::thread::spawn(move || super::forget_reviews(&at, &[session]).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!sweep.is_finished(), "the sweep waits for the hold");
        super::set_review((&home, &lock), (&held[0], &held), &version, true, now).unwrap();
        drop(lock);
        sweep.join().unwrap();
        let file = std::fs::read_to_string(home.join("state").join(super::REVIEWS)).unwrap();
        assert_eq!(file, "{}", "taken once the hold was let go");
    }

    /// Rule 6: a file of checks that does not read is no check, and the next write replaces it.
    #[test]
    fn a_file_of_checks_that_does_not_read_is_no_check() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        let done = json!({"task": "parser", "status": "done", "request_status": "done"});
        h.wrote("s1", "fix", done);
        h.run_read();
        let home = h.s.home.path().to_owned();
        std::fs::create_dir_all(home.join("state")).unwrap();
        std::fs::write(home.join("state").join(super::REVIEWS), b"{not json").unwrap();
        let now = h.t + 10_000;
        let (v, held) = super::reviewing(&home, now).unwrap();
        assert_eq!(v["requests"][0]["checked"], Value::Null);
        let version = v["requests"][0]["version"].as_str().unwrap().to_owned();
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[0], &held),
            &version,
            true,
            now,
        )
        .unwrap();
        assert_eq!(
            super::reviewing(&home, now).unwrap().0["requests"][0]["checked"],
            json!(now)
        );
    }

    /// Rule 6: `oboete forget` of a session takes My Work's check of it when its purge commits.
    #[test]
    fn a_forget_of_a_session_takes_its_check() {
        let mut h = Home::new();
        let first = h.said("s1", -60_000, "Fix the parser.");
        let done = json!({"task": "parser", "status": "done", "request_status": "done"});
        h.wrote("s1", "fix", done);
        h.run_read();
        let home = h.s.home.path().to_owned();
        let now = h.t + 10_000;
        let (v, held) = super::reviewing(&home, now).unwrap();
        let version = v["requests"][0]["version"].as_str().unwrap().to_owned();
        super::set_review(
            (&home, &super::reviews_lock(&home).unwrap()),
            (&held[0], &held),
            &version,
            true,
            now,
        )
        .unwrap();
        let device = h.s.raw.device().to_owned();
        let Home { s, .. } = h;
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let target = crate::forget::session_target(&home, &format!("{device}:{first}")).unwrap();
        crate::forget::start(&home, &crate::forget::preview(&home, target).unwrap()).unwrap();
        crate::forget::purge(&home).unwrap().unwrap();
        let file = std::fs::read_to_string(home.join("state").join(super::REVIEWS)).unwrap();
        assert_eq!(file, "{}");
    }

    /// Each work item names its repository as the request's `repos` do, so that the page keeps
    /// two lists of one name in two repositories apart, as claude-mem's work streams are a
    /// project's list (Codex on slice 2's page).
    #[test]
    fn each_work_item_names_its_repository() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser in both repositories.");
        h.wrote("s1", "fix", json!({"task": "here", "status": "doing"}));
        h.repo = "github.com/o/other";
        h.wrote("s1", "fix", json!({"task": "there", "status": "doing"}));
        let v = h.read(Range::All, h.t + 10_000);
        let r = by_id(&v, "c:claude:s1");
        let mut shown: Vec<(&str, &str, &str)> = (r["work"].as_array().unwrap().iter())
            .map(|w| {
                let field = |k: &str| w[k].as_str().unwrap();
                (field("list"), field("repo"), field("task"))
            })
            .collect();
        shown.sort();
        let other = ("fix", "github.com/o/other", "there");
        assert_eq!(shown, [other, ("fix", R, "here")], "{r}");
        assert_eq!(r["repos"], json!(["github.com/o/other", R]));
    }

    /// Two repositories a rule masks alike keep two work streams: an item whose repository or
    /// list the gate masks carries a fingerprint of the stored pair, keyed by the home's own key,
    /// the same from read to read and holding nothing of either (CodeRabbit on #597). An item the
    /// gate shows as stored carries none.
    #[test]
    fn work_the_gate_masks_alike_keeps_its_streams_apart() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser in both repositories.");
        h.wrote("s1", "fix", json!({"task": "here", "status": "doing"}));
        h.repo = "github.com/o/other";
        h.wrote("s1", "fix", json!({"task": "there", "status": "doing"}));
        let streams = |v: &Value| -> Vec<Value> {
            let work = by_id(v, "c:claude:s1")["work"].as_array().unwrap().clone();
            work.iter().map(|w| w["stream"].clone()).collect()
        };
        let v = h.read(Range::All, h.t + 10_000);
        assert_eq!(streams(&v), [Value::Null, Value::Null], "{v}");
        let rule =
            "[redaction]\nextra_rules = [{ id = \"repo\", regex = 'github\\.com/o/[a-z]+' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let read = || super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let v = read();
        let work = by_id(&v, "c:claude:s1")["work"].as_array().unwrap().clone();
        assert!(work.iter().all(|w| w["repo"] == crate::redact::MASK), "{v}");
        let prints = streams(&v);
        let twelve = |p: &Value| p.as_str().is_some_and(|p| p.len() == 12);
        assert!(prints.iter().all(twelve), "{v}");
        assert_ne!(prints[0], prints[1]);
        assert_eq!(streams(&read()), prints, "the same from read to read");
        assert!(!v.to_string().contains("github.com/o/"), "{v}");
    }

    /// #596: a conversation known only by its work (no prompt, summary or card of its own read)
    /// is still the one its writes came from, as claude-mem credits a request to the session of
    /// the call that wrote it: its request names it in `sessions`, dated by its write, and its
    /// agent in `agents`; two that a rule masks alike stay two; its item with no `request_id`
    /// stays unlinked and counted.
    #[test]
    fn a_conversation_known_only_by_its_work_is_named_by_its_request() {
        let mut h = Home::new();
        for s in ["sess-1", "sess-2"] {
            // Each write a millisecond of its own, which its conversation is dated by.
            std::thread::sleep(std::time::Duration::from_millis(2));
            let fields = json!({"task": s, "status": "doing", "request_id": "rel",
                "request": "Fix the release notes"});
            h.wrote(s, "notes", fields);
        }
        h.wrote("sess-3", "plan", json!({"task": "loose", "status": "todo"}));
        let v = h.read(Range::All, h.t + 10_000);
        let r = by_id(&v, "r:rel");
        let of = |s: &str| {
            let id = format!("c:claude:{s}");
            json!({"agent": "claude", "id": id, "fingerprint": null})
        };
        assert_eq!(r["sessions"], json!([of("sess-2"), of("sess-1")]));
        assert_eq!(r["agents"], json!(["claude"]));
        assert_eq!(
            (ids(&v), &v["unlinked"]),
            (vec!["r:rel".to_owned()], &json!(1))
        );
        let rule = "[redaction]\nextra_rules = [{ id = \"id\", regex = '^sess-[0-9]+$' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let shown: Vec<&str> = (by_id(&v, "r:rel")["sessions"].as_array().unwrap().iter())
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(shown.len(), 2, "{v}");
        assert_ne!(shown[0], shown[1]);
        assert!(
            (shown.iter())
                .all(|id| id.starts_with("c:claude:[REDACTED]#") && !id.contains("sess-")),
            "{shown:?}"
        );
    }

    /// Codex on #574: an id the gate masks takes a fingerprint of the stored one, so that two
    /// masked alike stay two, each the same from read to read, and the stored one is not shown.
    #[test]
    fn ids_masked_alike_stay_two_and_keep_from_read_to_read() {
        let mut h = Home::new();
        for id in ["sess-7", "sess-8"] {
            h.write(
                "plan",
                json!({"task": id, "request_id": id, "request": "Fix the release notes",
                    "status": "doing"}),
            );
        }
        h.s.run();
        let rule = "[redaction]\nextra_rules = [{ id = \"id\", regex = '^sess-[0-9]+$' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let read = || ids(&super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap());
        let first = read();
        assert_eq!(first.len(), 2);
        assert_ne!(first[0], first[1]);
        assert!(
            (first.iter()).all(|id| id.starts_with("r:[REDACTED]#") && !id.contains("sess-")),
            "{first:?}"
        );
        assert_eq!(read(), first);
    }

    /// Codex on #574 (#570): a rule may match an id as assembled and neither of its parts. The
    /// answer's ids are the gate's already, so the route's own gate (`view::answered`, the same
    /// rules) masks nothing more in them, and two such ids stay two.
    #[test]
    fn an_id_a_rule_masks_as_assembled_is_masked_before_the_route_gates_it() {
        let mut h = Home::new();
        for id in ["ticket-1", "ticket-2"] {
            h.write(
                "plan",
                json!({"task": id, "request_id": id, "request": "Fix the release notes",
                    "status": "doing"}),
            );
        }
        h.s.run();
        let rule = "[redaction]\nextra_rules = [{ id = \"id\", regex = 'r:ticket-[0-9]' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let rules = Rules::load(h.s.home.path()).unwrap();
        let read = ids(&super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap());
        let routed: Vec<String> = (read.iter())
            .map(|id| crate::redact::outbound_with(id, &rules))
            .collect();
        assert_eq!((routed.len(), &routed), (2, &read));
        assert_ne!(routed[0], routed[1]);
        assert!(!routed.concat().contains("ticket-"), "{routed:?}");
    }

    /// Codex and CodeRabbit on #575: a part's mask may hide what a rule of the assembled id would
    /// match, so the stored id is put through the gate as assembled too; when a rule matches it,
    /// nothing of it is shown but its kind and the fingerprint.
    #[test]
    fn an_id_a_rule_matches_as_stored_shows_nothing_of_it() {
        let mut h = Home::new();
        h.said("s1", -60_000, "Fix the parser.");
        for id in ["ticket-1", "ticket-2"] {
            h.write(
                "plan",
                json!({"task": id, "request_id": id, "request": "Fix the release notes",
                    "status": "doing"}),
            );
        }
        h.s.run();
        let rule = |id: &str, regex: &str| format!("{{ id = \"{id}\", regex = '{regex}' }}");
        let rules = [
            rule("tail", "-[0-9]$"),
            rule("ticket", "r:ticket-[0-9]"),
            rule("agent", "^claude$"),
            rule("conversation", "^c:claude:s1$"),
        ];
        let config = format!("[redaction]\nextra_rules = [{}]\n", rules.join(", "));
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let rules = Rules::load(h.s.home.path()).unwrap();
        let read = ids(&super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap());
        let routed: Vec<String> = (read.iter())
            .map(|id| crate::redact::outbound_with(id, &rules))
            .collect();
        assert_eq!((routed.len(), &routed), (3, &read));
        assert_eq!(routed.iter().collect::<HashSet<_>>().len(), 3, "{routed:?}");
        for id in &routed {
            let shown = id.split('#').next().unwrap();
            assert!(
                ["c:[REDACTED]", "r:[REDACTED]"].contains(&shown),
                "{routed:?}"
            );
        }
    }

    /// CodeRabbit on #574: the fingerprint of a masked id is keyed by the home's own key, made at
    /// the first read (rule 6's versions are keyed by it too), so that it tells nothing of a guess
    /// at the id: a hash of the id alone is not it, and another home's fingerprint of the same id
    /// differs.
    #[test]
    fn a_masked_ids_fingerprint_is_keyed_by_the_homes_own_key() {
        let homes: Vec<Home> = (0..2)
            .map(|_| {
                let mut h = Home::new();
                h.write(
                    "plan",
                    json!({"task": "t", "request_id": "sess-7", "request": "Fix the release notes",
                        "status": "doing"}),
                );
                h.s.run();
                h
            })
            .collect();
        let key = |h: &Home| h.s.home.path().join("state").join("my-work.key");
        let read = |h: &Home| ids(&super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap());
        assert_eq!(read(&homes[0]), ["r:sess-7"]);
        // Made at the first read, though no id is masked: each request's version is keyed by it
        // (rule 6).
        assert!(key(&homes[0]).exists(), "a key for the versions");
        let rule = "[redaction]\nextra_rules = [{ id = \"id\", regex = '^sess-[0-9]+$' }]\n";
        for h in &homes {
            std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        }
        let (a, b) = (read(&homes[0]), read(&homes[1]));
        assert!(a[0].starts_with("r:[REDACTED]#") && b[0].starts_with("r:[REDACTED]#"));
        assert_ne!(a, b);
        let unkeyed = &crate::forget::hash(b"r:sess-7")[..12];
        assert!(!a[0].ends_with(unkeyed), "{a:?}");
        assert!(key(&homes[0]).exists());
        assert_eq!(read(&homes[0]), a);
    }

    /// A grouping op that decided `groups` (each its conversations, the anchor first, and its
    /// title) over the prompts `shown`, as the grouping job keeps one.
    fn grouped(h: &mut Home, groups: &[(&[&str], &str)], shown: &[i64]) {
        let groups: Vec<Value> = (groups.iter())
            .map(|(members, title)| {
                let keys: Vec<Value> = members.iter().map(|s| json!(["claude", s])).collect();
                json!({"conversations": keys, "title": title, "links": []})
            })
            .collect();
        let op = crate::my_work_ops::MyWorkOp {
            job: "group".into(),
            about: R.into(),
            entry: "fake".into(),
            ts: crate::db::now_ms(),
            read: crate::my_work_ops::Read {
                spans: shown.iter().map(|&s| (s, s)).collect(),
                ops: vec![],
            },
            removed: vec![],
            fields: BTreeMap::from([
                ("groups".to_owned(), json!(groups)),
                ("ignored".to_owned(), json!([])),
            ]),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }

    /// docs/my-work-model.md C6: a group is one request, by `g:` and a keyed hash of its anchor:
    /// the earliest of its asks, the newest six follow-ups of all its conversations, each
    /// conversation's own card in the agents view (C7), and the title grouping gave it.
    #[test]
    fn a_group_is_one_request_with_the_earliest_ask_and_the_newest_six_follow_ups() {
        let mut h = Home::new();
        // The earliest ask is the conversation read second, so the group takes it from there.
        let first = h.said("s2", 0, "Build the parser.");
        let second = h.said("s1", 1_000, "Add tests for the parser.");
        for i in 0..4 {
            h.said("s2", 2_000 + i, &format!("Handle case {i} in the parser."));
            h.said("s1", 3_000 + i, &format!("Cover case {i} in the tests."));
        }
        grouped(
            &mut h,
            &[(&["s1", "s2"], "The parser and its tests")],
            &[first, second],
        );
        let v = h.read(Range::All, crate::db::now_ms());
        let requests = v["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{v}");
        let r = &requests[0];
        let id = r["id"].as_str().unwrap();
        assert!(id.starts_with("g:") && id.len() == 14, "{id}");
        assert!(!id.contains("s1"), "{id}");
        assert_eq!(r["title"], "The parser and its tests");
        assert_eq!(r["ask"]["text"], "Build the parser.");
        let follow_ups: Vec<&str> = (r["follow_ups"].as_array().unwrap().iter())
            .map(|f| f["text"].as_str().unwrap())
            .collect();
        assert_eq!(follow_ups.len(), 6);
        assert_eq!(follow_ups.last(), Some(&"Cover case 3 in the tests."));
        let sessions: Vec<&str> = (r["sessions"].as_array().unwrap().iter())
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(sessions.len(), 2);
        assert!(
            sessions.iter().all(|s| s.starts_with("c:claude:")),
            "{sessions:?}"
        );
    }

    /// C7: a conversation joining a group is a new version of its request, as a new follow-up is;
    /// a new title is not.
    #[test]
    fn a_joined_conversation_changes_the_version_and_a_new_title_does_not() {
        let mut h = Home::new();
        let first = h.said("s1", 0, "Build the parser.");
        let second = h.said("s2", 1_000, "Add tests for the parser.");
        let third = h.said("s3", 2_000, "Fix the parser's errors.");
        grouped(&mut h, &[(&["s1", "s2"], "The parser")], &[first, second]);
        let version = |h: &Home| {
            let v = h.read(Range::All, crate::db::now_ms());
            let r = (v["requests"].as_array().unwrap().iter())
                .find(|r| r["id"].as_str().unwrap().starts_with("g:"))
                .unwrap()
                .clone();
            (
                r["version"].as_str().unwrap().to_owned(),
                r["title"].clone(),
            )
        };
        let (before, _) = version(&h);
        grouped(&mut h, &[(&["s1"], "The parser, retitled")], &[first]);
        let (retitled, title) = version(&h);
        assert_eq!(
            (retitled.as_str(), title),
            (before.as_str(), json!("The parser, retitled"))
        );
        grouped(
            &mut h,
            &[(&["s1", "s3"], "The parser, retitled")],
            &[first, third],
        );
        let (joined, _) = version(&h);
        assert_ne!(joined, before);
    }

    /// #684: a member grouping kept whose conversation has cards and no readable opening ask is in
    /// its group's request, though it is read before the member that makes the group.
    #[test]
    fn a_member_with_no_readable_ask_read_first_is_in_its_groups_request() {
        let card = json!([{"type": "change", "title": "Tables cached", "subtitle": "Tables cached",
            "narrative": "x"}]);
        let mut h = Home::new();
        // s0 is read before s1 and has no prompt: a reply and its card.
        let quiet = h.reply("s0", 0);
        let card_ids = h.s.cards(quiet, quiet, card.clone(), false);
        let first = h.said("s1", 1_000, "Build the parser.");
        grouped(&mut h, &[(&["s1", "s0"], "The parser")], &[first]);
        let v = h.read(Range::All, crate::db::now_ms());
        let requests = v["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{v}");
        assert_eq!(requests[0]["sessions"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(requests[0]["cards"], json!(card_ids), "{v}");
        assert_eq!(v["unlinked"], 0, "{v}");
        // No group: the same conversation is counted among the unlinked, as before.
        let mut h = Home::new();
        let quiet = h.reply("s0", 0);
        h.s.cards(quiet, quiet, card.clone(), false);
        let v = h.read(Range::All, crate::db::now_ms());
        assert_eq!(v["unlinked"], 1, "{v}");
        // A group none of whose members has an opening ask: its conversations are among the
        // unlinked, as before.
        let mut h = Home::new();
        for session in ["s0", "s2"] {
            let quiet = h.reply(session, 0);
            h.s.cards(quiet, quiet, card.clone(), false);
        }
        grouped(&mut h, &[(&["s0", "s2"], "The parser")], &[]);
        let v = h.read(Range::All, crate::db::now_ms());
        assert_eq!(v["requests"].as_array().unwrap().len(), 0, "{v}");
        assert_eq!(v["unlinked"], 2, "{v}");
    }

    /// #686: a work item written in a group member's conversation that has no readable opening
    /// ask, and names no request, is in the group's request with the work product its fields
    /// record, though it is read before the member that makes the group; with no group it is
    /// counted among the unlinked, once.
    #[test]
    fn a_quiet_members_work_items_are_in_its_groups_request() {
        let mut h = Home::new();
        // s0 is read first: a reply and a work item its agent wrote, no prompt.
        h.reply("s0", 0);
        h.wrote(
            "s0",
            "parser",
            json!({"task": "Wire the parser", "status": "doing",
            "work_product": "https://example.com/parser"}),
        );
        let first = h.said("s1", 1_000, "Build the parser.");
        h.wrote(
            "s1",
            "parser-tests",
            json!({"task": "Test the parser", "status": "todo"}),
        );
        grouped(&mut h, &[(&["s1", "s0"], "The parser")], &[first]);
        let v = h.read(Range::All, crate::db::now_ms());
        let requests = v["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{v}");
        assert_eq!(
            tasks(&requests[0]),
            ["Test the parser", "Wire the parser"],
            "{v}"
        );
        assert_eq!(v["unlinked"], 0, "{v}");
        let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
        assert_eq!(ids(&answer).len(), 1, "{answer}");
        let products = super::products(h.s.home.path(), &held[0]).unwrap();
        assert!(
            products
                .iter()
                .any(|p| p["url"] == "https://example.com/parser"),
            "{products:?}"
        );
        // No group: the item is among the unlinked, once, and the reply alone counts nothing.
        let mut h = Home::new();
        h.reply("s0", 0);
        h.wrote(
            "s0",
            "parser",
            json!({"task": "Wire the parser", "status": "doing"}),
        );
        let v = h.read(Range::All, crate::db::now_ms());
        assert_eq!(v["requests"].as_array().unwrap().len(), 0, "{v}");
        assert_eq!(v["unlinked"], 1, "{v}");
        // A group none of whose members makes its request: the item is among the unlinked, once.
        h.reply("s2", 10);
        grouped(&mut h, &[(&["s0", "s2"], "The parser")], &[]);
        let v = h.read(Range::All, crate::db::now_ms());
        assert_eq!(v["requests"].as_array().unwrap().len(), 0, "{v}");
        assert_eq!(v["unlinked"], 1, "{v}");
    }

    /// #689: a work item that waited for its group's request takes its place in the request's
    /// work, newest first, so the request's next step is the newest one recorded.
    #[test]
    fn a_deferred_work_item_keeps_its_place_in_its_requests_work() {
        let mut h = Home::new();
        h.reply("s0", 0);
        let first = h.said("s1", 1_000, "Build the parser.");
        h.wrote(
            "s1",
            "parser-tests",
            json!({"task": "Test the parser", "status": "todo",
            "next_step": "Write the first test."}),
        );
        h.wrote(
            "s0",
            "parser",
            json!({"task": "Wire the parser", "status": "doing",
            "next_step": "Wire the lexer in."}),
        );
        grouped(&mut h, &[(&["s1", "s0"], "The parser")], &[first]);
        let v = h.read(Range::All, crate::db::now_ms());
        let requests = v["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{v}");
        assert_eq!(requests[0]["next"], "Wire the lexer in.", "{v}");
    }

    /// S6: while the repository a group read is excluded, its conversations are not shown as
    /// a group; taken back, the group is shown again.
    #[test]
    fn a_group_read_from_an_excluded_repository_is_not_shown() {
        let mut h = Home::new();
        let first = h.said("s1", 0, "Build the parser.");
        let second = h.said("s2", 1_000, "Add tests for the parser.");
        grouped(&mut h, &[(&["s1", "s2"], "The parser")], &[first, second]);
        h.s.raw.exclude(R, false).unwrap();
        let v = h.read(Range::All, crate::db::now_ms());
        assert!(v["requests"].as_array().unwrap().is_empty(), "{v}");
        h.s.raw.exclude(R, true).unwrap();
        let v = h.read(Range::All, crate::db::now_ms());
        assert_eq!(v["requests"].as_array().unwrap().len(), 1, "{v}");
    }
}
