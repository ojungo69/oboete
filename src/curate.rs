//! Milestone 3 Task 5 (docs/milestone-3-plan.md D2, D8, D12; issue #54): a device's next window
//! of records, cut where a curator can read it whole, and its text as it may leave the machine.
//! Every record in a window's range is covered by it: curated with its text, elided with a marker
//! (a tool output larger than a window), or carrying no text (a session's start or end, a
//! tombstone).
// The curation phase (Task 5, part 3) is its caller.
#![allow(dead_code)]

use crate::raw::{Event, Item, Raw};
use crate::redact::Rules;
use anyhow::Result;
use serde_json::Value;

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
}
