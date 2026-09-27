//! Milestone 3 Task 5 (docs/milestone-3-plan.md D2, D8, D12; issue #54): a device's next window
//! of records, cut where a curator can read it whole, and its text as it may leave the machine.
//! Every record in a window's range is covered by it: curated with its text, elided with a marker
//! (a tool output larger than a window), or carrying no text (a session's start or end, a
//! tombstone).
//! `run_phase` curates the next window: it calls the curator on its text and appends the window
//! op and its claims in one transaction, or keeps a pending row that says what it waits for.

use crate::config::Summary;
use crate::provider::{ChainFailed, ChainResult, Fallback, Skip};
use crate::providers_db::{self, Pending};
use crate::raw::{Event, Item, OpKind, Raw};
use crate::redact::Rules;
use anyhow::{Result, anyhow};
use rusqlite::Connection;
use serde_json::{Value, json};

/// D8's interim window size, in estimated tokens (`budget::estimate`): Groq free's 8,000-token
/// ceiling holds a window, the prompt's fixed part and a 1,250-token answer
/// (docs/spike/curator-sizes.md). Measurement Window replaces it at the end of milestone 3.
pub const WINDOW_TOKENS: u32 = 5_000;
/// A tool's input shown in a window, at most: the output is what a window reads or elides.
const TOOL_INPUT_CHARS: usize = 2_000;
/// A session heading's length, at most.
const HEADING_CHARS: usize = 200;
/// Records read at a time while a window is cut.
const PAGE: usize = 200;
/// Records one window covers at most, text or not (issue #54: a window is bounded in records as
/// well as in tokens, so a long run of tombstones cannot make one unbounded).
const MAX_RECORDS: usize = 2_000;

/// One window of a device's records.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub device: String,
    /// Where it starts: the first record after the curation checkpoint, or inside the event a
    /// window split (`from_offset`, a byte offset into its text).
    pub from_seq: i64,
    pub from_offset: Option<i64>,
    /// The last record it covers, and where it stopped inside it when it split it.
    pub to_seq: i64,
    pub to_offset: Option<i64>,
    /// What the curator reads, through the egress gate, grouped by session. Empty when none of its
    /// records has text (a session's start, tombstones): the caller covers it without a call.
    pub text: String,
    /// Tool outputs larger than a window: seen, and elided with a marker in `text`.
    pub elided: Vec<i64>,
    /// Cut by its size: more records follow. Otherwise it ends at the device's last record.
    pub full: bool,
}

/// One record's share of a window.
struct Piece {
    seq: i64,
    /// Where its text starts inside the event, and where it stops when the window split it.
    from: i64,
    to: Option<i64>,
    /// Its session as stored (what groups it) and as shown (its heading, through the gate).
    key: String,
    heading: String,
    /// Its text: empty for a record with nothing to read.
    text: String,
    tokens: u32,
    /// A typed prompt that starts here: a turn boundary (D12).
    turn: bool,
    tool: bool,
}

/// Bytes of records read at a time while a window is cut, at least one record (spec 3.1: pages
/// bounded by events and bytes).
const PAGE_BYTES: usize = 4 << 20;

/// The device's next window after its curation checkpoint, or `None` when it has no record
/// there. `rules` are the redaction rules as they are now: a rule added after capture still
/// hides its matches (spec 6.4).
pub fn next_window(raw: &Raw, device: &str, budget: u32, rules: &Rules) -> Result<Option<Window>> {
    let (seq, offset) = raw.curation_checkpoint(device)?;
    let mut after = if offset.is_some() { seq - 1 } else { seq };
    let (mut pieces, mut used, mut elided, mut full) = (Vec::<Piece>::new(), 0, Vec::new(), false);
    let mut sessions = std::collections::HashSet::new();
    'read: loop {
        let records = raw.after_within(device, after, PAGE, PAGE_BYTES)?;
        if records.is_empty() {
            break;
        }
        for r in records {
            after = r.seq;
            if pieces.len() >= MAX_RECORDS {
                full = true;
                break 'read;
            }
            let Item::Event(e) = r.item else {
                pieces.push(empty(r.seq));
                continue;
            };
            let from = if r.seq == seq { offset.unwrap_or(0) } else { 0 };
            let prepared = Prepared::new(&e, rules);
            let mut piece = prepared.piece(r.seq, from, None);
            // A session's first record also brings its heading.
            let heading = if sessions.contains(&piece.key) {
                0
            } else {
                crate::budget::estimate(&format!("## {}\n", piece.heading))
            };
            if !piece.text.is_empty() {
                piece.tokens += heading;
            }
            if used + piece.tokens <= budget {
                used += piece.tokens;
                if !piece.text.is_empty() {
                    sessions.insert(piece.key.clone());
                }
                pieces.push(piece);
                continue;
            }
            full = true;
            if pieces.iter().all(|p| p.text.is_empty()) {
                // The first event to read does not fit: a tool output is elided, anything else is
                // split, and the next window starts where this part stops.
                if piece.tool {
                    piece = prepared.elided(r.seq);
                    elided.push(r.seq);
                } else {
                    piece = prepared.split(r.seq, from, budget.saturating_sub(used + heading));
                }
                pieces.push(piece);
            } else {
                cut_back(&mut pieces);
            }
            break 'read;
        }
    }
    let (Some(first), Some(last)) = (pieces.first(), pieces.last()) else {
        return Ok(None);
    };
    Ok(Some(Window {
        device: device.to_owned(),
        from_seq: first.seq,
        from_offset: (first.from > 0).then_some(first.from),
        to_seq: last.seq,
        to_offset: last.to,
        text: grouped(&pieces),
        elided,
        full,
    }))
}

/// D12: a full window ends before its last turn boundary, else after its last tool call, else
/// where the next event did not fit. The first record always stays, so a window moves on.
fn cut_back(pieces: &mut Vec<Piece>) {
    if let Some(i) = pieces.iter().rposition(|p| p.turn).filter(|&i| i > 0) {
        pieces.truncate(i);
    } else if let Some(i) = pieces.iter().rposition(|p| p.tool) {
        pieces.truncate(i + 1);
    }
}

fn empty(seq: i64) -> Piece {
    Piece {
        seq,
        from: 0,
        to: None,
        key: String::new(),
        heading: String::new(),
        text: String::new(),
        tokens: 0,
        turn: false,
        tool: false,
    }
}

/// Capture's `{kind, mime, bytes, sha256}` markers for binary content, whole or inside a text:
/// neither the content nor its marker goes to a curator (spec 2.3). Known by their four keys in
/// order, whatever their values: a capture rule may have masked any of them.
fn without_markers(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        let value = r#"(?:"(?:[^"\\]|\\.)*"|[^,{}"]*)"#;
        regex::Regex::new(&format!(
            r#"\{{"kind":{value},"mime":{value},"bytes":{value},"sha256":{value}\}}"#
        ))
        .expect("marker pattern")
    });
    RE.replace_all(s, "").into_owned()
}

/// What the gate hides in a text, in its offsets (`redact::hidden`).
type Hidden = Option<Vec<(usize, usize)>>;

/// One event as a window shows it: its line before its long text through the gate, its long text
/// (the part a window may split) with what the gate hides in it found once on the whole of it, and
/// its session.
struct Prepared<'r> {
    rules: &'r Rules,
    head: String,
    long: Option<(String, Hidden)>,
    tool: bool,
    turn: bool,
    key: String,
    heading: String,
}

impl<'r> Prepared<'r> {
    fn new(e: &Event, rules: &'r Rules) -> Self {
        let body: Value = serde_json::from_str(&e.body).unwrap_or(Value::String(e.body.clone()));
        let text = |v: &Value| match v {
            Value::Null => None,
            Value::String(t) => Some(without_markers(t)),
            v => Some(without_markers(&v.to_string())),
        };
        // Fields this renderer does not know (a key a redaction rule masked at capture) are shown
        // after the long text rather than dropped unseen.
        let rest = |known: &[&str]| -> Option<String> {
            let other: serde_json::Map<String, Value> = body
                .as_object()?
                .iter()
                .filter(|(k, _)| !known.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (!other.is_empty()).then(|| without_markers(&Value::Object(other).to_string()))
        };
        let joined = |long: Option<String>, known: &[&str]| match (long, rest(known)) {
            (Some(l), Some(r)) => Some(format!("{l}\n{r}")),
            (l, r) => l.or(r),
        };
        let gate = |s: &str| crate::redact::outbound_with(s, rules);
        let (head, long, tool) = match e.kind.as_str() {
            "prompt" if body["omitted"] == true => ("[user] (not stored)".into(), None, false),
            "prompt" | "envelope" => {
                let who = if e.kind == "prompt" {
                    "[user]"
                } else {
                    "[harness]"
                };
                let long = joined(text(&body["prompt"]), &["prompt", "omitted"]);
                (who.to_owned(), long, false)
            }
            "reply" => (
                "[assistant]".into(),
                joined(text(&body["assistant"]), &["assistant"]),
                false,
            ),
            "compaction" => match joined(text(&body["summary"]), &["summary", "trigger"]) {
                Some(s) => ("[compaction summary]".into(), Some(s), false),
                None => ("[compaction]".into(), None, false),
            },
            "tool" => {
                let input = text(&body["input"]).unwrap_or_default();
                // Cut like a split event: a secret across the cut is found in the whole input.
                let cut = input
                    .char_indices()
                    .nth(TOOL_INPUT_CHARS)
                    .map_or(input.len(), |(i, _)| i);
                let input = crate::redact::outbound_part(&input, 0..cut, rules);
                let failed = if body["failed"] == true {
                    " failed"
                } else {
                    ""
                };
                let name = gate(body["tool"].as_str().unwrap_or("?"));
                let head = format!("[tool {name}{failed}] input: {input}\n  output:");
                let known = [
                    "tool",
                    "input",
                    "output",
                    "failed",
                    "agent_id",
                    "interrupted",
                ];
                let long = joined(text(&body["output"]), &known).unwrap_or_default();
                (head, Some(long), true)
            }
            _ => (String::new(), None, false), // a session's start or end: nothing to read
        };
        // Each label through the gate on its own, before it is shortened or joined.
        let place = match (&e.repo, &e.branch) {
            (Some(r), b) => {
                let repo = gate(r);
                let name = repo.rsplit('/').next().unwrap_or(&repo).to_owned();
                match b {
                    Some(b) => format!(" in {name}@{}", gate(b)),
                    None => format!(" in {name}"),
                }
            }
            _ => String::new(),
        };
        // A label is shown whole up to this, after the gate: a heading never takes a window.
        let heading: String = format!("{} session {}{place}", gate(&e.agent), gate(&e.session))
            .chars()
            .take(HEADING_CHARS)
            .collect();
        Self {
            rules,
            long: long.map(|l| {
                let hidden = crate::redact::hidden(&l, rules);
                (l, hidden)
            }),
            head,
            tool,
            turn: e.kind == "prompt",
            key: format!("{}\u{0}{}", e.agent, e.session),
            heading,
        }
    }

    /// Its text from byte `from` of its long text to `to` (its end when none), through the gate.
    fn piece(&self, seq: i64, from: i64, to: Option<i64>) -> Piece {
        let mut text = self.head.clone();
        if let Some((long, hidden)) = &self.long {
            let start = boundary(long, from);
            let end = to.map_or(long.len(), |t| boundary(long, t)).max(start);
            text.push(' ');
            text.push_str(&crate::redact::outbound_range(
                long,
                start..end,
                hidden.as_deref(),
                self.rules,
            ));
        }
        self.with(seq, from, to, text)
    }

    fn with(&self, seq: i64, from: i64, to: Option<i64>, text: String) -> Piece {
        Piece {
            seq,
            from,
            to,
            key: self.key.clone(),
            heading: self.heading.clone(),
            tokens: line_tokens(&text),
            turn: self.turn && from == 0,
            tool: self.tool,
            text,
        }
    }

    /// A tool call whose output is larger than a window: its line, and the output's size only.
    fn elided(&self, seq: i64) -> Piece {
        let bytes = self.long.as_ref().map_or(0, |(l, _)| l.len());
        let text = format!("{} ({bytes} bytes of output, seen, elided)", self.head);
        Piece {
            turn: false,
            ..self.with(seq, 0, None, text)
        }
    }

    /// The part from `from` that fits in `room` tokens: as far as fits, back to a line's end when
    /// one is in its second half, and at least one character so that curation moves on.
    fn split(&self, seq: i64, from: i64, room: u32) -> Piece {
        let long = self.long.as_ref().map_or("", |(l, _)| l.as_str());
        let start = boundary(long, from);
        let fits = |end: usize| self.piece(seq, from, Some(end as i64)).tokens <= room;
        let (mut lo, mut hi) = (next_char(long, start), long.len());
        if !fits(lo) {
            hi = lo;
        }
        // The largest end that fits, on a character boundary.
        while lo < hi {
            let mid = boundary(long, ((lo + hi).div_ceil(2)) as i64);
            let mid = if mid <= lo { next_char(long, lo) } else { mid };
            if mid <= hi && fits(mid) {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1).max(lo);
                hi = boundary(long, hi as i64).max(lo);
            }
        }
        let half = boundary(long, (start + (lo - start) / 2) as i64);
        let end = long[half..lo].rfind('\n').map_or(lo, |i| half + i + 1);
        let to = (end < long.len()).then_some(end as i64);
        self.piece(seq, from, to)
    }
}

/// `e`'s text from byte `from` of its long text to `to` (its end when none), through the gate.
#[cfg(test)]
fn render(e: &Event, seq: i64, from: i64, to: Option<i64>, rules: &Rules) -> Piece {
    Prepared::new(e, rules).piece(seq, from, to)
}

/// What a piece's line takes in a window, its newline included: the sum over a window's lines and
/// headings is never less than the estimate of the text they make (each is rounded up).
fn line_tokens(text: &str) -> u32 {
    if text.is_empty() {
        0
    } else {
        crate::budget::estimate(text) + 1
    }
}

/// `at` moved back to a character boundary of `s`, within it.
fn boundary(s: &str, at: i64) -> usize {
    let mut i = usize::try_from(at).unwrap_or(0).min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The boundary after the character at `at`, or the end.
fn next_char(s: &str, at: usize) -> usize {
    s[at..]
        .chars()
        .next()
        .map_or(s.len(), |c| at + c.len_utf8())
}

/// D12: the window's text with each session's records together, sessions in the order they first
/// appear, records in seq order within each. Sessions are told apart as stored, not by their
/// headings, which the gate may make alike.
fn grouped(pieces: &[Piece]) -> String {
    let mut sessions: Vec<(&str, &str)> = Vec::new();
    for p in pieces.iter().filter(|p| !p.text.is_empty()) {
        if !sessions.iter().any(|(k, _)| *k == p.key) {
            sessions.push((&p.key, &p.heading));
        }
    }
    let mut out = String::new();
    for (key, heading) in sessions {
        out.push_str(&format!("## {heading}\n"));
        for p in pieces.iter().filter(|p| p.key == key && !p.text.is_empty()) {
            out.push_str(&p.text);
            out.push('\n');
        }
    }
    out
}

/// What one run of the phase did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// A window was covered: curated, covered with no text to read, or skipped. More may follow.
    Covered,
    /// The next window is not tried before `until` (unix ms). `up`: it waits only on time, and
    /// within `STAY_UP_MS`, so the worker stays up for it (D10).
    Waiting { until: i64, up: bool },
    /// Nothing to curate.
    Idle,
}

/// The curator: `(span, prompt, working)` to an answer. `working` says until when the owner is
/// still working (D9), asked right before each subscription call.
pub type Curator<'a> = dyn FnMut(&str, &str, &dyn Fn() -> Option<i64>) -> Result<ChainResult> + 'a;

/// D10: a wait longer than this does not keep the worker up.
const STAY_UP_MS: i64 = 30 * 60 * 1000;
/// D11: attempts with no answer before a window is skipped, and the time between two of them.
const ATTEMPTS: i64 = 3;
const RETRY_MS: i64 = 10 * 60 * 1000;
/// A window that waits on the owner (a provider stopped until `oboete resume`, a curator CLI the
/// isolation gate refused) is tried again this often, and never keeps the worker up.
const OWNER_RETRY_MS: i64 = 60 * 60 * 1000;
/// v1's bounds on one answer.
const MAX_OBSERVATIONS: usize = 12;
const MAX_SUMMARY_CHARS: usize = 2_000;
const KINDS: [&str; 6] = [
    "decision",
    "bugfix",
    "feature",
    "discovery",
    "change",
    "preference",
];

#[cfg(test)]
thread_local! {
    /// A test seam: the phase stops between the answer and its append, as a crash would.
    static STOP_BEFORE_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The curation phase (D3): this device's next window, curated or waited on. A window that
/// reaches the device's last record waits until the owner has stopped (the idle gate's time),
/// so a window is not sent for every few records while the owner works.
pub fn run_phase(
    raw: &mut Raw,
    db: &Connection,
    rules: &Rules,
    summary: &Summary,
    curator: &mut Curator,
) -> Result<Phase> {
    let device = raw.device().to_owned();
    let Some(w) = next_window(raw, &device, summary.window_tokens, rules)? else {
        providers_db::clear_pending(db, &device)?;
        return Ok(Phase::Idle);
    };
    if w.text.is_empty() {
        return cover(raw, db, &w, json!({"outcome": "covered"}), Vec::new());
    }
    // At most D10's stay-up: a longer wait would not keep the worker up, and once the owner
    // stopped no hook would start one to curate what waited.
    let idle = (i64::from(summary.idle_minutes) * 60_000).min(STAY_UP_MS);
    let working = |raw: &Raw| match raw.last_hook_ts() {
        Ok(ts) => ts.map(|ts| ts + idle).filter(|&t| t > crate::db::now_ms()),
        // Unreadable: taken for the owner at work, so no subscription is spent on a guess.
        Err(_) => Some(crate::db::now_ms() + idle),
    };
    let now = crate::db::now_ms();
    if !w.full
        && let Some(until) = working(raw)
    {
        return Ok(Phase::Waiting {
            until,
            up: until - now <= STAY_UP_MS,
        });
    }
    let prompt = prompt(&summary.language, &w.text);
    let sent = sha256_hex(&prompt);
    // A row for another request is stale, and its attempts were not on this one: a restore or a
    // skipped window moved the checkpoint, records added since made the window longer, or new
    // rules or another language changed what would be sent.
    let range = |p: &Pending| (p.from_seq, p.from_offset, p.to_seq, p.to_offset);
    let pending = providers_db::pending_of(db, &device)?.filter(|p| {
        range(p) == (w.from_seq, w.from_offset, w.to_seq, w.to_offset) && p.prompt == sent
    });
    if let Some(p) = &pending
        && p.next_attempt_at > now
    {
        return Ok(waiting(p, now));
    }
    let span = format!("{}-{}", w.from_seq, w.to_seq);
    let answer = {
        let raw: &Raw = raw;
        curator(&span, &prompt, &|| working(raw))
    };
    let failed = match answer {
        Ok(r) => match parse(&r.output) {
            Ok((summary, claims)) => {
                let op = json!({"outcome": "curated", "provider": r.provider, "summary": summary});
                return cover(raw, db, &w, op, claims);
            }
            // Counted like a provider that failed: no answer this window can use.
            Err(e) => vec![Fallback {
                provider: r.provider,
                reason: format!("{e:#}"),
                skip: Skip::Failed,
            }],
        },
        Err(e) => match e.downcast::<ChainFailed>() {
            Ok(ChainFailed(fallbacks)) => fallbacks,
            Err(e) => return Err(e),
        },
    };
    let reason = ChainFailed(failed.clone()).to_string();
    let (hold, next, counted) = hold(&failed, now);
    let attempts = pending.as_ref().map_or(0, |p| p.attempts) + i64::from(counted);
    if attempts >= ATTEMPTS {
        let op = json!({"outcome": "skipped", "reason": reason});
        return cover(raw, db, &w, op, Vec::new());
    }
    let p = Pending {
        device,
        from_seq: w.from_seq,
        from_offset: w.from_offset,
        to_seq: w.to_seq,
        to_offset: w.to_offset,
        reason,
        hold: hold.into(),
        attempts,
        next_attempt_at: next,
        since: pending.map_or(now, |p| p.since),
        prompt: sent,
    };
    providers_db::set_pending(db, &p)?;
    Ok(waiting(&p, now))
}

/// What a window every provider went past waits for, when it is tried again, and whether the
/// attempt counts toward D11's three: only when no provider waits on time or a budget, and at
/// least one was tried (or can never take it). One that waits only on the owner does not count,
/// nor does a chain with no entry at all (an owner hold too: the owner configures one).
fn hold(failed: &[Fallback], now: i64) -> (&'static str, i64, bool) {
    let timed = |budget: bool| {
        failed
            .iter()
            .filter_map(|f| match f.skip {
                Skip::Wait(at) if !budget => Some(at),
                Skip::Budget(at) if budget => Some(at),
                _ => None,
            })
            .min()
    };
    match (timed(false), timed(true)) {
        // Whichever comes first says what the window waits for then.
        (Some(wait), Some(budget)) if budget < wait => ("budget", budget, false),
        (Some(wait), _) => ("time", wait, false),
        (None, Some(budget)) => ("budget", budget, false),
        (None, None)
            if failed
                .iter()
                .any(|f| matches!(f.skip, Skip::Failed | Skip::TooBig)) =>
        {
            ("time", now + RETRY_MS, true)
        }
        (None, None) => ("owner", now + OWNER_RETRY_MS, false),
    }
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn waiting(p: &Pending, now: i64) -> Phase {
    Phase::Waiting {
        until: p.next_attempt_at,
        up: p.hold == "time" && p.next_attempt_at - now <= STAY_UP_MS,
    }
}

/// The window op, with the claims it yields, in one transaction: the curation checkpoint moves
/// with the window's knowledge (D2, spec 3.1).
fn cover(
    raw: &mut Raw,
    db: &Connection,
    w: &Window,
    mut op: Value,
    claims: Vec<Value>,
) -> Result<Phase> {
    op["from_seq"] = w.from_seq.into();
    op["from_offset"] = w.from_offset.into();
    op["to_seq"] = w.to_seq.into();
    op["to_offset"] = w.to_offset.into();
    op["elided"] = w.elided.clone().into();
    let mut ops = vec![(OpKind::Window, op)];
    ops.extend(claims.into_iter().map(|c| (OpKind::Claim, c)));
    #[cfg(test)]
    if STOP_BEFORE_APPEND.with(std::cell::Cell::get) {
        anyhow::bail!("stopped before the append (test seam)");
    }
    raw.append_ops(&ops)?;
    providers_db::clear_pending(db, &w.device)?;
    Ok(Phase::Covered)
}

/// v1's observation prompt (Task 7 replaces it with claims), for a window that spans sessions.
fn prompt(language: &str, text: &str) -> String {
    format!(
        "You are the long-term memory of a software developer. Below is a stretch of their work with \
         coding agents, grouped by session under `## <agent> ...` headings.\n\
         Extract only what is worth remembering in future sessions of these repositories, then write a short summary.\n\
         Observations are facts, decisions, bug fixes, discoveries, changes or the developer's stated preferences: \
         concrete, with file paths, names and numbers. Skip routine tool noise, restated instructions and anything \
         the code itself already shows. If nothing is worth remembering, return an empty observations array. \
         At most {MAX_OBSERVATIONS} observations, each with a kind, a specific title (max 80 chars) and a body of 1-3 sentences.\n\
         The summary is 2-4 sentences about this stretch: what was worked on, what was decided, what is still open.\n\
         Write every title, body and the summary in {language}.\n\n\
         --- WORK ---\n{text}\n--- END ---"
    )
}

/// The answer's shape, as the chain checks it.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "observations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": KINDS},
                        "title": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["kind", "title", "body"],
                    "additionalProperties": false
                }
            },
            "summary": {"type": "string"}
        },
        "required": ["observations", "summary"],
        "additionalProperties": false
    })
}

/// The summary and one claim op body per observation, bounded as v1 bounds them.
fn parse(v: &Value) -> Result<(String, Vec<Value>)> {
    let observations = v["observations"]
        .as_array()
        .ok_or_else(|| anyhow!("invalid output: observations is not an array"))?;
    let summary: String = v["summary"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect();
    let mut claims = Vec::new();
    for o in observations.iter().take(MAX_OBSERVATIONS) {
        let title = o["title"].as_str().unwrap_or("").trim();
        let body = o["body"].as_str().unwrap_or("").trim();
        if title.is_empty() || body.is_empty() {
            continue;
        }
        // CLI providers do not enforce the schema's enum.
        let kind = o["kind"]
            .as_str()
            .filter(|k| KINDS.contains(k))
            .unwrap_or("discovery");
        claims.push(json!({
            "kind": kind,
            "title": title.chars().take(120).collect::<String>(),
            "body": body.chars().take(1_000).collect::<String>(),
        }));
    }
    Ok((summary, claims))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{OpKind, test_event};

    fn event(kind: &str, body: Value) -> Event {
        Event {
            kind: kind.into(),
            ..test_event(&body.to_string())
        }
    }

    fn prompt(text: &str) -> Event {
        event("prompt", serde_json::json!({"prompt": text}))
    }

    fn tool(output: &str) -> Event {
        event(
            "tool",
            serde_json::json!({"tool": "Bash", "input": "cargo test", "output": output, "failed": false}),
        )
    }

    /// The window's range as the curation phase records it (D2).
    fn close(raw: &mut Raw, w: &Window) {
        let op = serde_json::json!({"from_seq": w.from_seq, "from_offset": w.from_offset,
            "to_seq": w.to_seq, "to_offset": w.to_offset, "outcome": "curated"});
        raw.append_ops(&[(OpKind::Window, op)]).unwrap();
    }

    fn store() -> (tempfile::TempDir, Raw, String) {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        (home, raw, dev)
    }

    #[test]
    fn a_window_takes_what_fits_and_ends_before_its_last_turn() {
        let (_h, mut raw, dev) = store();
        let rules = Rules::default();
        assert_eq!(next_window(&raw, &dev, 200, &rules).unwrap(), None);
        raw.append(&prompt("fix the date parser")).unwrap(); // 1
        raw.append(&tool("0 failed; 21 passed")).unwrap(); // 2
        raw.append(&prompt("now the importer")).unwrap(); // 3
        raw.append(&tool(&"x".repeat(600))).unwrap(); // 4: fits alone, not with 1-3
        let w = next_window(&raw, &dev, 200, &rules).unwrap().unwrap();
        // Records 1-3 fit and 4 does not: the window ends before the turn at 3.
        assert_eq!(
            (w.from_seq, w.to_seq, w.to_offset, w.full),
            (1, 2, None, true)
        );
        assert!(w.text.contains("[user] fix the date parser") && !w.text.contains("importer"));
        assert!(w.text.starts_with("## claude session s\n"));
        close(&mut raw, &w);
        let w = next_window(&raw, &dev, 1_000, &rules).unwrap().unwrap();
        assert_eq!((w.from_seq, w.to_seq, w.full), (3, 4, false));
    }

    #[test]
    fn an_event_larger_than_the_window_is_split_and_no_part_is_marked_done_unseen() {
        let (_h, mut raw, dev) = store();
        let rules = Rules::default();
        let lines: Vec<String> = (0..200)
            .map(|i| format!("line {i:03} of the answer"))
            .collect();
        let reply = lines.join("\n");
        raw.append(&event("reply", serde_json::json!({"assistant": reply})))
            .unwrap();
        raw.append(&prompt("thanks")).unwrap();
        let mut seen = String::new();
        let mut windows = 0;
        while let Some(w) = next_window(&raw, &dev, 300, &rules).unwrap() {
            windows += 1;
            assert!(windows < 50, "no progress");
            if w.to_seq == 1 {
                // A part ends at a line's end, and the next starts there.
                assert!(
                    w.to_offset
                        .is_none_or(|o| reply.as_bytes()[o as usize - 1] == b'\n')
                );
            }
            seen.push_str(&w.text);
            close(&mut raw, &w);
        }
        assert!(windows > 2);
        // Every line was in some part, once, and the prompt after it too.
        for l in &lines {
            assert_eq!(seen.matches(l.as_str()).count(), 1, "{l}");
        }
        assert!(seen.contains("[user] thanks"));
    }

    #[test]
    fn a_tool_output_larger_than_the_window_is_elided_with_a_marker() {
        let (_h, raw, dev) = store();
        let mut raw = raw;
        raw.append(&tool(&"y".repeat(5_000))).unwrap();
        raw.append(&prompt("next")).unwrap();
        let w = next_window(&raw, &dev, 300, &Rules::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.from_seq, w.to_seq, w.to_offset, w.elided.clone()),
            (1, 1, None, vec![1])
        );
        assert!(w.text.contains("(5000 bytes of output, seen, elided)") && !w.text.contains("yyy"));
    }

    #[test]
    fn a_split_events_next_part_starts_where_the_last_window_ended() {
        let (h, mut raw, dev) = store();
        let rules = Rules::default();
        let reply = "one line of the answer\n".repeat(100);
        raw.append(&event("reply", serde_json::json!({"assistant": reply})))
            .unwrap();
        let first = next_window(&raw, &dev, 200, &rules).unwrap().unwrap();
        let cut = first.to_offset.expect("split");
        close(&mut raw, &first);
        // The worker stops between the two windows.
        drop(raw);
        let raw = crate::raw::open(h.path()).unwrap();
        let next = next_window(&raw, &dev, 200, &rules).unwrap().unwrap();
        assert_eq!((next.from_seq, next.from_offset), (1, Some(cut)));
        let part = |w: &Window| w.text.split_once("[assistant] ").unwrap().1.to_owned();
        let (a, b) = (part(&first), part(&next));
        assert!(reply.starts_with(&(a.trim_end_matches('\n').to_owned() + "\n" + &b[..10])));
    }

    #[test]
    fn a_secret_added_to_the_rules_after_capture_does_not_leave_in_a_window() {
        let (h, mut raw, dev) = store();
        // Captured under the bundled rules, which do not know the owner's own token shape. One
        // long line, so each size cuts it somewhere else.
        let secret = format!("acme-{}", "7Qx9Lm2Vb4Nr");
        let filler = "words of the answer ".repeat(30);
        let reply = format!(
            "{filler}the key is {secret} and <private>the {secret} plan</private> ends {filler}"
        );
        let reply_event = event("reply", serde_json::json!({"assistant": reply}));
        raw.append(&reply_event).unwrap();
        std::fs::write(
            h.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[A-Za-z0-9]{12}' }]\n",
        )
        .unwrap();
        let rules = Rules::load(h.path()).unwrap();
        let leaks = |text: &str| {
            [&secret[..8], &secret[5..], "plan"]
                .into_iter()
                .find(|p| text.contains(p))
        };
        for budget in 30..400 {
            let w = next_window(&raw, &dev, budget, &rules).unwrap().unwrap();
            let mut text = w.text.clone();
            // The window after it, from where this one stopped.
            if let Some(o) = w.to_offset {
                text.push_str(&render(&reply_event, 1, o, None, &rules).text);
            }
            assert_eq!(leaks(&text), None, "{budget}: {text:?}");
        }
        // A cut anywhere inside the secret or the block: the part before it and the part after it.
        let at = reply.find(&secret).unwrap();
        let block = reply.find("<private>").unwrap()..reply.find("</private>").unwrap() + 10;
        for o in (at..at + secret.len()).chain(block) {
            let before = render(&reply_event, 1, 0, Some(o as i64), &rules).text;
            let after = render(&reply_event, 1, o as i64, None, &rules).text;
            assert_eq!(leaks(&before), None, "{o}: {before:?}");
            assert_eq!(leaks(&after), None, "{o}: {after:?}");
        }
    }

    #[test]
    fn a_window_is_bounded_in_records_and_one_with_nothing_to_read_has_no_text() {
        let (_h, mut raw, dev) = store();
        let start = event("start", serde_json::json!({"source": "startup"}));
        for _ in 0..MAX_RECORDS + 5 {
            raw.append(&start).unwrap();
        }
        let w = next_window(&raw, &dev, WINDOW_TOKENS, &Rules::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.from_seq, w.to_seq, w.full),
            (1, MAX_RECORDS as i64, true)
        );
        assert_eq!(w.text, "");
    }

    #[test]
    fn a_secret_across_the_tool_input_cut_does_not_leave() {
        let (h, _raw, _) = store();
        std::fs::write(
            h.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[A-Za-z0-9]{12}' }]\n",
        )
        .unwrap();
        let rules = Rules::load(h.path()).unwrap();
        let secret = format!("acme-{}", "7Qx9Lm2Vb4Nr");
        // The input is cut at 2,000 characters, in the middle of the secret.
        let input = "a".repeat(TOOL_INPUT_CHARS - 6) + " " + &secret + " rest";
        let e = event(
            "tool",
            serde_json::json!({"tool": "Bash", "input": input, "output": "ok", "failed": false}),
        );
        let text = render(&e, 1, 0, None, &rules).text;
        assert!(!text.contains("acme-") && !text.contains("rest"), "{text}");
    }

    #[test]
    fn a_window_that_splits_its_first_event_stays_within_its_budget_with_the_heading() {
        let (_h, mut raw, dev) = store();
        let reply = "one line of the answer\n".repeat(100);
        let long_session = Event {
            session: "a-long-session-label-".repeat(10),
            ..event("reply", serde_json::json!({"assistant": reply}))
        };
        raw.append(&long_session).unwrap();
        for budget in [120, 200, 300] {
            let w = next_window(&raw, &dev, budget, &Rules::default())
                .unwrap()
                .unwrap();
            assert!(w.to_offset.is_some());
            let took = crate::budget::estimate(&w.text);
            assert!(took <= budget, "{budget}: {took}");
        }
    }

    #[test]
    fn labels_binary_markers_and_masked_keys_are_shown_as_the_gate_allows() {
        let (h, mut raw, dev) = store();
        std::fs::write(
            h.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^acme-[A-Za-z0-9]{12}$' }]\n",
        )
        .unwrap();
        let rules = Rules::load(h.path()).unwrap();
        // The session label is gated whole, before it is joined into a heading.
        let secret = format!("acme-{}", "7Qx9Lm2Vb4Nr");
        let labelled = Event {
            session: secret.clone(),
            ..prompt("hello")
        };
        raw.append(&labelled).unwrap();
        // A binary marker, inside a text: neither the content nor the marker goes.
        let sha = "a".repeat(64);
        let marker = format!(r#"{{"kind":"image","mime":"image/png","bytes":3,"sha256":"{sha}"}}"#);
        raw.append(&prompt(&format!("see {marker} here"))).unwrap();
        // A marker a capture rule masked a value of is still a marker.
        raw.append(&prompt(
            r#"then {"kind":"image","mime":"image/png","bytes":3,"sha256":"[REDACTED]"} there"#,
        ))
        .unwrap();
        // A reply whose key a rule masked at capture keeps its text.
        raw.append(&event(
            "reply",
            serde_json::json!({"[REDACTED]": "an important decision"}),
        ))
        .unwrap();
        let w = next_window(&raw, &dev, WINDOW_TOKENS, &rules)
            .unwrap()
            .unwrap();
        assert!(!w.text.contains("7Qx9Lm2Vb4Nr"), "{}", w.text);
        assert!(
            !w.text.contains(&sha) && !w.text.contains("image/png"),
            "{}",
            w.text
        );
        assert!(
            w.text.contains("see  here") && w.text.contains("then  there"),
            "{}",
            w.text
        );
        assert!(w.text.contains("an important decision"), "{}", w.text);
    }

    #[test]
    fn sessions_the_gate_labels_alike_stay_apart() {
        let (h, mut raw, dev) = store();
        std::fs::write(
            h.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"id\", regex = '^sess-[0-9]+$' }]\n",
        )
        .unwrap();
        let rules = Rules::load(h.path()).unwrap();
        for (session, text) in [("sess-1", "one"), ("sess-2", "two"), ("sess-1", "three")] {
            raw.append(&Event {
                session: session.into(),
                ..prompt(text)
            })
            .unwrap();
        }
        let w = next_window(&raw, &dev, WINDOW_TOKENS, &rules)
            .unwrap()
            .unwrap();
        assert_eq!(w.text.matches("## ").count(), 2, "{}", w.text);
        let (first, second) = w.text.split_at(w.text.rfind("## ").unwrap());
        assert!(first.contains("one") && first.contains("three") && second.contains("two"));
    }

    #[test]
    fn a_split_in_japanese_text_cuts_on_character_boundaries() {
        let (_h, mut raw, dev) = store();
        let reply = "日".repeat(10_000);
        raw.append(&event("reply", serde_json::json!({"assistant": reply})))
            .unwrap();
        let mut seen = 0;
        let mut windows = 0;
        while let Some(w) = next_window(&raw, &dev, 1_000, &Rules::default()).unwrap() {
            windows += 1;
            assert!(windows < 50, "no progress");
            seen += w.text.matches('日').count();
            close(&mut raw, &w);
        }
        assert_eq!(seen, 10_000);
    }

    #[test]
    fn a_heading_longer_than_a_window_is_cut() {
        let (_h, mut raw, dev) = store();
        raw.append(&Event {
            session: "s".repeat(50_000),
            ..prompt("hello")
        })
        .unwrap();
        let w = next_window(&raw, &dev, 300, &Rules::default())
            .unwrap()
            .unwrap();
        assert!(crate::budget::estimate(&w.text) <= 300, "{}", w.text.len());
        assert!(w.text.contains("hello"));
    }

    // The curation phase (part 3b).

    use std::cell::Cell;

    fn curating(tokens: u32) -> Summary {
        Summary {
            curate: true,
            window_tokens: tokens,
            ..Summary::default()
        }
    }

    fn answered(provider: &str) -> ChainResult {
        ChainResult {
            provider: provider.into(),
            output: json!({"observations": [{"kind": "decision", "title": "UTC on disk",
                "body": "All timestamps on disk stay UTC."}], "summary": "Timestamps."}),
            fallbacks: Vec::new(),
        }
    }

    fn went_past(skips: &[(&str, &str, Skip)]) -> anyhow::Error {
        let each = skips.iter().map(|(provider, reason, skip)| Fallback {
            provider: (*provider).into(),
            reason: (*reason).into(),
            skip: skip.clone(),
        });
        ChainFailed(each.collect()).into()
    }

    fn windows(raw: &Raw) -> Vec<Value> {
        raw.ops_after(raw.device(), 0, 100_000)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::Window)
            .map(|o| o.body)
            .collect()
    }

    fn open(home: &std::path::Path) -> (Raw, Connection) {
        let raw = crate::raw::open(home).unwrap();
        (raw, providers_db::open(home).unwrap())
    }

    /// Review Focus 4: a worker stopped between the answer and the append moves neither the
    /// window op nor the checkpoint, and the same window is curated again, once.
    #[test]
    fn the_window_and_its_checkpoint_commit_together() {
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        for text in ["one", "two", "three"] {
            raw.append(&prompt(text)).unwrap();
        }
        let calls = Cell::new(0);
        let mut curator = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            calls.set(calls.get() + 1);
            Ok(answered("fake"))
        };
        let (rules, summary) = (Rules::default(), curating(WINDOW_TOKENS));
        STOP_BEFORE_APPEND.with(|s| s.set(true));
        let stopped = run_phase(&mut raw, &db, &rules, &summary, &mut curator);
        STOP_BEFORE_APPEND.with(|s| s.set(false));
        assert!(stopped.is_err());
        assert!(windows(&raw).is_empty());
        assert_eq!(raw.curation_checkpoint(raw.device()).unwrap(), (0, None));
        drop(raw);
        let mut raw = crate::raw::open(home.path()).unwrap();
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut curator).unwrap();
        assert_eq!(phase, Phase::Covered);
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        let kinds: Vec<OpKind> = ops.iter().map(|o| o.kind).collect();
        assert_eq!(kinds, [OpKind::Window, OpKind::Claim]);
        assert_eq!(ops[0].body["to_seq"], 3);
        assert_eq!(ops[0].body["outcome"], "curated");
        assert_eq!(ops[1].body["title"], "UTC on disk");
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut curator).unwrap();
        assert_eq!(phase, Phase::Idle);
        assert_eq!(calls.get(), 2);
    }

    /// D9, Review Focus 3: while the owner works, a free provider is tried and a subscription
    /// waits until ten minutes after the last hook record; a tombstone and a replayed record
    /// written since do not move that time.
    #[test]
    fn a_subscription_waits_while_hooks_arrive_and_a_free_provider_does_not() {
        let now = crate::db::now_ms();
        let at = |ts: i64, text: &str| Event {
            ts,
            ..prompt(&text.repeat(40))
        };
        let tried = Cell::new(0);
        let mut chain =
            |_: &str, _: &str, working: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
                tried.set(tried.get() + 1);
                match working() {
                    Some(until) => Err(went_past(&[
                        ("free", "HTTP 500", Skip::Failed),
                        ("sub", "waiting for the owner to finish", Skip::Wait(until)),
                    ])),
                    None => Ok(answered("sub")),
                }
            };
        // Two windows' worth: the first is full, so it does not wait for more records.
        let (rules, summary) = (Rules::default(), curating(30));
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        raw.append(&at(now - 60_000, "a")).unwrap();
        raw.append(&at(now - 60_000, "b")).unwrap();
        let until = now - 60_000 + 600_000;
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert_eq!(phase, Phase::Waiting { until, up: true });
        let p = providers_db::pending_of(&db, raw.device())
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.hold.as_str(), p.attempts, p.next_attempt_at),
            ("time", 0, until)
        );
        assert!(
            p.reason.contains("waiting for the owner to finish"),
            "{}",
            p.reason
        );
        // Not tried again before then.
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert_eq!(phase, Phase::Waiting { until, up: true });
        assert_eq!(tried.get(), 1);

        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        raw.append(&at(now - 660_000, "a")).unwrap();
        raw.append(&at(now - 660_000, "b")).unwrap();
        let device = raw.device().to_owned();
        raw.append_tombstone(crate::raw::Target::Record { device, seq: 2 })
            .unwrap();
        let replayed = Event {
            source: "replay".into(),
            ..at(now, "c")
        };
        raw.append(&replayed).unwrap();
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert_eq!(phase, Phase::Covered);
        assert_eq!(windows(&raw)[0]["provider"], "sub");
    }

    /// A window that reaches the device's last record is not sent while the owner works: it
    /// would be sent again and again for every few records. It waits as a subscription does.
    #[test]
    fn a_window_at_the_last_record_waits_until_the_owner_stops() {
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        let ts = crate::db::now_ms() - 60_000;
        raw.append(&Event {
            ts,
            ..prompt("one")
        })
        .unwrap();
        let calls = Cell::new(0);
        let mut curator = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            calls.set(calls.get() + 1);
            Ok(answered("groq"))
        };
        let (rules, summary) = (Rules::default(), curating(WINDOW_TOKENS));
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut curator).unwrap();
        let until = ts + 600_000;
        assert_eq!(phase, Phase::Waiting { until, up: true });
        assert_eq!(calls.get(), 0);
        // A wait longer than D10's stay-up is cut to it, so the worker stays up for it.
        let long = Summary {
            idle_minutes: 60,
            ..summary.clone()
        };
        let phase = run_phase(&mut raw, &db, &rules, &long, &mut curator).unwrap();
        let until = ts + STAY_UP_MS;
        assert_eq!(phase, Phase::Waiting { until, up: true });
        // A full window goes at once.
        let phase = run_phase(&mut raw, &db, &rules, &curating(3), &mut curator).unwrap();
        assert_eq!((phase, calls.get()), (Phase::Covered, 1));
    }

    /// D11: an attempt counts only when no provider waits on time or a budget and one was tried
    /// or can never take the window. After three the window is skipped with its reason, and the
    /// next window goes on.
    #[test]
    fn a_window_every_provider_fails_is_skipped_after_three_attempts_and_the_next_one_goes_on() {
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        raw.append(&prompt(&"a".repeat(40))).unwrap();
        raw.append(&prompt(&"b".repeat(40))).unwrap();
        let later = crate::db::now_ms() + 3_600_000;
        // Within D10's 30 minutes: it keeps the worker up.
        let soon = crate::db::now_ms() + 1_200_000;
        let script = [
            (vec![("claude", "stopped", Skip::Owner)], "owner", 0),
            (
                vec![
                    ("groq", "HTTP 400", Skip::Failed),
                    ("paid", "USD", Skip::Budget(later)),
                ],
                "budget",
                0,
            ),
            (
                vec![
                    ("groq", "HTTP 400", Skip::Failed),
                    ("sub", "waiting", Skip::Wait(soon)),
                ],
                "time",
                0,
            ),
            // A budget that resets first is what the window waits for then: no staying up.
            (
                vec![
                    ("sub", "waiting", Skip::Wait(later)),
                    ("paid", "USD", Skip::Budget(soon)),
                ],
                "budget",
                0,
            ),
            (vec![("groq", "HTTP 400", Skip::Failed)], "time", 1),
            (
                vec![
                    ("groq", "too big", Skip::TooBig),
                    ("claude", "stopped", Skip::Owner),
                ],
                "time",
                2,
            ),
        ];
        let step = Cell::new(0);
        let mut chain = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            let i = step.get();
            step.set(i + 1);
            match script.get(i) {
                Some((skips, _, _)) => Err(went_past(skips)),
                None if i == script.len() => Err(went_past(&[("groq", "HTTP 400", Skip::Failed)])),
                None => Ok(answered("groq")),
            }
        };
        let (rules, summary) = (Rules::default(), curating(30));
        for (skips, hold, attempts) in &script {
            let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
            let mut p = providers_db::pending_of(&db, raw.device())
                .unwrap()
                .unwrap();
            assert_eq!(
                (p.hold.as_str(), p.attempts),
                (*hold, *attempts),
                "{skips:?}"
            );
            let up = *hold == "time";
            assert!(
                matches!(phase, Phase::Waiting { up: u, .. } if u == up),
                "{phase:?}"
            );
            assert!(p.reason.contains(skips[0].1), "{}", p.reason);
            // Its time has come.
            p.next_attempt_at = 0;
            providers_db::set_pending(&db, &p).unwrap();
        }
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert_eq!(phase, Phase::Covered);
        assert!(
            providers_db::pending_of(&db, raw.device())
                .unwrap()
                .is_none()
        );
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert_eq!(phase, Phase::Covered);
        let ws = windows(&raw);
        assert_eq!(ws[0]["outcome"], "skipped");
        assert_eq!(ws[0]["to_seq"], 1);
        assert!(ws[0]["reason"].as_str().unwrap().contains("groq: HTTP 400"));
        assert_eq!(ws[1]["outcome"], "curated");
        assert_eq!(ws[1]["from_seq"], 2);
    }

    /// Task 5: a pending row counts only while raw's next window still starts where it does. A
    /// restore that rewinds raw leaves a row for a later window: its attempts and its time are
    /// not the new window's.
    #[test]
    fn a_restore_that_rewinds_raw_drops_the_pending_rows_above_it() {
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        raw.append(&prompt("one")).unwrap();
        let stale = Pending {
            device: raw.device().to_owned(),
            from_seq: 7,
            from_offset: None,
            to_seq: 9,
            to_offset: None,
            reason: "every provider failed".into(),
            hold: "time".into(),
            attempts: 2,
            next_attempt_at: crate::db::now_ms() + 3_600_000,
            since: 0,
            prompt: String::new(),
        };
        providers_db::set_pending(&db, &stale).unwrap();
        let mut chain = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            Err(went_past(&[("groq", "HTTP 400", Skip::Failed)]))
        };
        let phase = run_phase(
            &mut raw,
            &db,
            &Rules::default(),
            &curating(WINDOW_TOKENS),
            &mut chain,
        );
        assert!(matches!(phase.unwrap(), Phase::Waiting { up: true, .. }));
        let p = providers_db::pending_of(&db, raw.device())
            .unwrap()
            .unwrap();
        assert_eq!((p.from_seq, p.to_seq, p.attempts), (1, 1, 1));
        assert!(p.since > 0);
    }

    /// Records added to a window that failed make a longer window: its attempts start again, so
    /// no record is skipped without three attempts on it.
    #[test]
    fn a_window_that_grew_starts_its_attempts_again() {
        let home = tempfile::tempdir().unwrap();
        let (mut raw, db) = open(home.path());
        raw.append(&prompt("one")).unwrap();
        let mut chain = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            Err(went_past(&[("groq", "HTTP 400", Skip::Failed)]))
        };
        let (rules, summary) = (Rules::default(), curating(WINDOW_TOKENS));
        run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        let mut p = providers_db::pending_of(&db, raw.device())
            .unwrap()
            .unwrap();
        assert_eq!((p.to_seq, p.attempts), (1, 1));
        // Two attempts on it so far, and it is due; then the owner adds a record.
        (p.attempts, p.next_attempt_at) = (2, 0);
        providers_db::set_pending(&db, &p).unwrap();
        raw.append(&prompt("two")).unwrap();
        let phase = run_phase(&mut raw, &db, &rules, &summary, &mut chain).unwrap();
        assert!(matches!(phase, Phase::Waiting { .. }), "{phase:?}");
        let mut p = providers_db::pending_of(&db, raw.device())
            .unwrap()
            .unwrap();
        assert_eq!((p.to_seq, p.attempts), (2, 1));
        // The same range asked for in another language is another request too.
        (p.attempts, p.next_attempt_at) = (2, 0);
        providers_db::set_pending(&db, &p).unwrap();
        let english = Summary {
            language: "English".into(),
            ..summary.clone()
        };
        let phase = run_phase(&mut raw, &db, &rules, &english, &mut chain).unwrap();
        assert!(matches!(phase, Phase::Waiting { .. }), "{phase:?}");
        let p = providers_db::pending_of(&db, raw.device())
            .unwrap()
            .unwrap();
        assert_eq!((p.to_seq, p.attempts), (2, 1));
    }

    /// Milestone 2's coverage part, on a replayed day: every seq is in a window op, curated,
    /// covered or skipped, in order, with no gap and no overlap; split events continue at their
    /// offset.
    #[test]
    fn every_seq_is_curated_elided_or_skipped() {
        let home = tempfile::tempdir().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/testdata/fixtures/long-24h.jsonl");
        crate::replay::run(home.path(), &fixture, None, 0, &[1], "claude").unwrap();
        let (mut raw, db) = open(home.path());
        let mut curator = |_: &str, _: &str, _: &dyn Fn() -> Option<i64>| -> Result<ChainResult> {
            Ok(answered("fake"))
        };
        let (rules, summary) = (Rules::default(), curating(WINDOW_TOKENS));
        let mut runs = 0;
        while run_phase(&mut raw, &db, &rules, &summary, &mut curator).unwrap() == Phase::Covered {
            runs += 1;
            assert!(runs < 10_000);
        }
        let ws = windows(&raw);
        assert!(ws.len() > 1);
        let mut next = (1, None);
        for w in &ws {
            let start = (w["from_seq"].as_i64().unwrap(), w["from_offset"].as_i64());
            assert_eq!(start, next, "{w}");
            assert!(matches!(
                w["outcome"].as_str(),
                Some("curated" | "covered" | "skipped")
            ));
            let to = w["to_seq"].as_i64().unwrap();
            next = match w["to_offset"].as_i64() {
                Some(o) => (to, Some(o)),
                None => (to + 1, None),
            };
        }
        assert_eq!(next, (raw.max_seq().unwrap() + 1, None));
    }
}
