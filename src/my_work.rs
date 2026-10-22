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
fn shown(text: &str, max: usize, rules: &Rules) -> String {
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
                writes: l.state.writes,
                list_writes: Vec::new(),
                by: None,
            });
        }
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.updated));
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

/// A request's identity (#570): an agent's session, or a `request_id`, as the stores hold them;
/// the answer gates it.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Id {
    Conversation(Key),
    Explicit(String),
}

struct Request {
    id: Id,
    original: Original,
    explicit_text: Option<String>,
    owns: Vec<(Key, Owns)>,
    work: Vec<usize>,
}

impl Request {
    fn new(id: Id, original: Original) -> Self {
        Self {
            id,
            original,
            explicit_text: None,
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
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok(json!({"requests": [], "unlinked": 0}));
    };
    let rules = Rules::load(home)?;
    let gate = |s: &str| crate::redact::outbound_with(s, &rules);
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
            let (page, more) = crate::turns::page(&k, &raw, None, before, SUMMARIES, &rules)?;
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
            let (page, more) = crate::cards::page(&k, &raw, None, before, CARDS, &rules)?;
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
    let work = link(&raw, &k, fold(writes), &excluded)?;
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
        c.prompts.push(p);
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
    let mut requests: Vec<Request> = Vec::new();
    let mut at: HashMap<Id, usize> = HashMap::new();
    let mut unlinked = 0;
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
            unlinked += 1;
            continue;
        };
        let id = explicit.map_or_else(|| Id::Conversation(key.unwrap().clone()), Id::Explicit);
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
        let id = (ids.first().cloned()).map_or_else(|| Id::Conversation(key.clone()), Id::Explicit);
        match at.get(&id) {
            Some(&j) => requests[j].take(key, Owns::All),
            None => match c.original(raw.device()) {
                Some(o) if ids.is_empty() => {
                    at.insert(id.clone(), requests.len());
                    let mut r = Request::new(id, o);
                    r.take(key, Owns::All);
                    requests.push(r);
                }
                _ => unlinked += c.units(&HashSet::new()),
            },
        }
    }

    let since = range.since(now);
    let mut answers = Vec::new();
    let mut key = None;
    for r in &requests {
        let stores = (&raw, &k, &rules);
        answers.extend(answer(
            r,
            &convs,
            &work,
            stores,
            (home, &mut key),
            (since, now),
        )?);
    }
    answers.sort_by_key(|v| std::cmp::Reverse(v["updated"].as_i64().unwrap_or(0)));
    Ok(json!({"requests": answers, "unlinked": unlinked}))
}

/// The home's key of the fingerprints of masked ids (`answer`): 32 random bytes in
/// `state/my-work.key`, made the first time an id is masked and never in an answer, so that a
/// fingerprint tells nothing of a guess at the id it stands for (CodeRabbit on #574). Whoever can
/// read the file can read the stores, which hold the ids themselves.
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

/// One request as the route gives it, every text through the gate; none when its newest update is
/// before `since`, which then gates nothing.
fn answer(
    r: &Request,
    convs: &BTreeMap<Key, Conversation>,
    work: &[Work],
    (raw, k, rules): (&Raw, &Connection, &Rules),
    (home, key): (&Path, &mut Option<Vec<u8>>),
    (since, now): (i64, i64),
) -> Result<Option<Value>> {
    let items: Vec<&Work> = r.work.iter().map(|&i| &work[i]).collect();
    let mut turns: Vec<&Turn> = Vec::new();
    let mut cards: Vec<&Shown> = Vec::new();
    let mut prompts: Vec<&Said> = Vec::new();
    // A request that owns some replies of a conversation shares its prompts but not their time.
    let mut active: Vec<i64> = Vec::new();
    let mut repos = BTreeSet::new();
    let mut agents = BTreeSet::new();
    let mut latest_prompt: Option<&Said> = None;
    for (key, own) in &r.owns {
        let Some(c) = convs.get(key) else { continue };
        agents.insert(crate::redact::outbound_with(&key.0, rules));
        repos.extend(c.repos.iter().cloned());
        prompts.extend(c.prompts.iter());
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
    }
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
    let recorded = (r.explicit_text.as_deref()).map(|t| title(&shown(t, ASK, rules)));
    let ask = shown(&original.text, ASK, rules);
    let (copy, title, summary, next, updating) = if turns.is_empty() {
        let mut updates: Vec<(i64, String)> = (items.iter())
            .filter_map(|w| {
                let text = w
                    .text(OUTCOME_KEYS, SUMMARY, rules)
                    .or_else(|| w.text(PROGRESS_KEYS, SUMMARY, rules));
                text.map(|t| (w.updated, t))
            })
            .collect();
        updates.extend(cards.iter().filter_map(|c| {
            let text = [&c.c.subtitle, &c.c.title]
                .into_iter()
                .find(|t| !t.trim().is_empty())?;
            Some((c.c.ts, cut(text, SUMMARY)))
        }));
        updates.sort_by_key(|u| std::cmp::Reverse(u.0));
        let summary =
            (updates.into_iter().next().map(|u| u.1)).unwrap_or_else(|| cut(&ask, SUMMARY));
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
    // (`capture::work_state`; Codex on #574).
    let work: Vec<Value> = (items.iter())
        .map(|w| {
            let status = (w.fields.get("status"))
                .map(|s| crate::work_state::beside("status", &crate::work_state::text(s), rules));
            let task = (w.task.as_deref()).map(|t| crate::work_state::beside("task", t, rules));
            // As capture keeps the list's name: beside its key (Codex on #574).
            let list = crate::work_state::beside("list", &w.list, rules);
            json!({"list": list, "task": task, "status": status, "updated": w.updated})
        })
        .collect();
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    let (id, held, kind) = match &r.id {
        Id::Conversation((agent, session)) => (
            format!("c:{}:{}", gate(agent), gate(session)),
            format!("c:{agent}:{session}"),
            "c",
        ),
        Id::Explicit(e) => (
            format!("r:{}", crate::work_state::beside("request_id", e, rules)),
            format!("r:{e}"),
            "r",
        ),
    };
    // The stored id through the gate as assembled too, since a rule may match it and neither of
    // its parts, which the route's own gate (`view::answered`) would then mask alike in two ids
    // (Codex on #574), or a part's mask may hide what that rule would match: then nothing of it is
    // shown but its kind (Codex and CodeRabbit on #575).
    let id = if gate(&held) == held {
        id
    } else {
        format!("{kind}:{}", crate::redact::MASK)
    };
    // An id the gate masks takes a fingerprint of the stored one, so that two masked alike stay
    // two and each keeps its id from read to read (Codex on #574), keyed by the home's own key so
    // that it tells nothing of a guess at the id (CodeRabbit on #574).
    let id = if id == held {
        id
    } else {
        let key = match key {
            Some(key) => key,
            None => key.insert(mask_key(home)?),
        };
        let keyed = [key.as_slice(), held.as_bytes()].concat();
        format!("{id}#{}", &crate::forget::hash(&keyed)[..12])
    };
    Ok(Some(json!({
        "id": id,
        "title": title,
        "summary": summary,
        "next": next,
        "copy": copy,
        "updating": updating,
        "status": status,
        "updated": updated,
        "started": original.ts,
        "agents": agents,
        "projects": names,
        "repos": repos,
        "ask": ask,
        "follow_ups": follow_ups,
        "work": work,
        "summaries": turns.iter().map(|t| t.s.id(raw.device())).collect::<Vec<_>>(),
        "cards": cards.iter().take(ACTIVITY).map(|c| c.c.id(raw.device())).collect::<Vec<_>>(),
    })))
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

/// claude-mem's `isHumanActionRequest`, without its list of English action words (the design's
/// 1): not an acknowledgement, a greeting or the harness's text, in English or Japanese.
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
    static TESTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^you can (?:def(?:initely)? )?do (?:real )?tests?[,;.!\s]").unwrap()
    });
    static WORK_WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:fix|build|create|implement|change|update|rename|remove|add)\b")
            .unwrap()
    });
    let plain = text.replace(['*', '_', '`', '#'], "");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    !plain.is_empty()
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

    /// A home whose times are near now: a work state write takes the time it is appended.
    struct Home {
        s: Store,
        t: i64,
    }

    impl Home {
        fn new() -> Self {
            Self {
                s: Store::new(),
                t: crate::db::now_ms(),
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
            let seq = self.s.raw.work_state(R, list, &fields, None).unwrap();
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
            self.s.event("tool", session, (R, "main"), ts, body)
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
        let Home { s, t } = h;
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
        h.s.run();
        let config = "[redaction]\nextra_rules = [{ id = \"t\", regex = 'task = \"canary-[0-9]{4}' }, \
                      { id = \"o\", regex = 'outcome = \"canary-[0-9]{4}' }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), config).unwrap();
        let v = super::read(h.s.home.path(), Range::All, h.t + 10_000).unwrap();
        let s1 = by_id(&v, "c:claude:s1");
        assert_eq!(s1["work"][0]["task"], crate::redact::MASK);
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
        }
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

    /// CodeRabbit on #574: the fingerprint of a masked id is keyed by the home's own key, made the
    /// first time an id is masked, so that it tells nothing of a guess at the id: a hash of the id
    /// alone is not it, and another home's fingerprint of the same id differs.
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
        assert!(!key(&homes[0]).exists(), "a key with no id masked");
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
}
