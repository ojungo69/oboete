//! claude-mem 15.19.0's work-view snapshot (decision 114, docs/page.md): the answer of
//! `GET /api/work-view`, which the vendored pages read in claude-mem's legacy mode
//! (`ui/work-view/`). It is made from oboete's My Work and goals answers as their readers give
//! them, each text gated at its source, and the Build story's kept texts (`read`), and reads
//! nothing else: each text in it is one of theirs or a word of this file's. It goes through the
//! gate in one walk as it leaves (`leaving`, #1089). The names follow claude-mem's `live/work-view-api-types.ts`. The contract test runs
//! claude-mem's own mapper, `live/to-work-model.ts`, over the answer of a synthetic home
//! (`ui/work-view/contract/`).

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{Local, Months, NaiveDate, SecondsFormat, TimeZone, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};

/// `GET /api/work-view`'s answer: what `read` reads, made into the snapshot, its goals' times in
/// this machine's zone, which the pages show as written; Refresh now runs while a request the
/// pages left stands (decision 117), a word of the snapshot's own that no gate need read.
pub(crate) fn answer(home: &Path, now: i64) -> anyhow::Result<Value> {
    let mut snapshot = answered(home, now, Gate::now)?;
    if crate::my_work_run::refresh_request(home, now).is_some() {
        snapshot["refresh"] = json!({"state": "running"});
    }
    Ok(snapshot)
}

/// How many times a read under one generation of rules (`settled`) runs before it gives up on
/// rules that keep changing.
const READS: usize = 3;

/// `answer`, the rules it leaves through as `rules_now` reads them: My Work and its goals read
/// under one generation of rules (`settled`), and the references of what the gate would mask
/// (`refer`), the walk and the story of the snapshot made under that generation's gate.
fn answered(home: &Path, now: i64, rules_now: impl FnMut() -> Gate) -> anyhow::Result<Value> {
    let ((mut work, goals), gate) = settled(home, rules_now, |source| read(home, now, source))?;
    refer(home, &mut work, &gate)?;
    Ok(snapshot_at(&work, &goals, now, &Local, &gate))
}

/// What `read` reads under one generation of rules, and that generation's gate: two sets of rules,
/// read before `read` runs and again once it ran, the home's, which My Work and its goals gate
/// their texts under at their source and which `read` is given, and those `rules_now` gives, which
/// the references of what the gate would mask (`refer`) and the walk read with (the same file's in
/// the viewer). When either changed while `read` ran, it runs again under the new sets, so that a
/// rule saved meanwhile is in force in the shapes My Work gates at its source and the walk does
/// not read again too (an id's parts, a work item's text beside its key: Codex on #1093); past
/// `READS` runs, the rules kept changing (`Unsettled`).
fn settled<T>(
    home: &Path,
    mut rules_now: impl FnMut() -> Gate,
    mut read: impl FnMut(&crate::redact::Rules) -> anyhow::Result<T>,
) -> anyhow::Result<(T, Gate)> {
    let mut gate = rules_now();
    let mut source = crate::redact::Rules::load(home)?;
    for _ in 0..READS {
        let got = read(&source)?;
        let after = rules_now();
        let after_source = crate::redact::Rules::load(home)?;
        if after.version() == gate.version() && after_source.version() == source.version() {
            return Ok((got, gate));
        }
        (gate, source) = (after, after_source);
    }
    Err(Unsettled.into())
}

/// The redaction rules changed while each of `READS` reads of the work view ran: a moment to try
/// again in, as the pages are told (`view::work_view_failed`).
#[derive(Debug)]
pub(crate) struct Unsettled;

impl Display for Unsettled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the redaction rules changed while each of {READS} reads of the work view ran"
        )
    }
}

impl std::error::Error for Unsettled {}

/// What a check from the pages names by a request's id in the snapshot (decision 117).
pub(crate) enum Named {
    /// The one request that goes by it: its answer as My Work reads it, its place among every
    /// range's requests, and their held list, by which a check's write prunes (`set_review`).
    One(Value, usize, Vec<crate::my_work::Held>),
    /// No request goes by it now.
    Gone,
    /// Two requests or more go by it, which only two stored ids of one keyed print would make: a
    /// check can tell none of them.
    Shared,
}

/// The request whose id in the snapshot (`selector`) is `id` (decision 117): read under one
/// generation of rules (`settled`) and referred to as the snapshot refers to it (`refer`), so that
/// a rule saved since the pages read their snapshot makes the id they have one no request goes
/// by, never another's; from every range, as a request the pages read may have left the week
/// since. It takes no readers' hold, as `reviewing` takes none: a check reads it under the
/// reviews' lock, which a purge waits for while it holds that hold (Codex on slice 2).
pub(crate) fn named(home: &Path, now: i64, id: &str) -> anyhow::Result<Named> {
    named_with(home, now, id, Gate::now)
}

/// `named`, its gate's rules as `rules_now` reads them.
fn named_with(
    home: &Path,
    now: i64,
    id: &str,
    rules_now: impl FnMut() -> Gate,
) -> anyhow::Result<Named> {
    let reviewing =
        |source: &crate::redact::Rules| crate::my_work::reviewing_under(home, now, source);
    let ((mut work, held), gate) = settled(home, rules_now, reviewing)?;
    refer(home, &mut work, &gate)?;
    let requests = arr(&work["requests"]);
    Ok(match going_by(requests, id)[..] {
        [i] => Named::One(requests[i].clone(), i, held),
        [] => Named::Gone,
        _ => Named::Shared,
    })
}

/// The places of the requests that go by `id` (`selector`), two at most: enough to tell one from
/// several.
fn going_by(requests: &[Value], id: &str) -> Vec<usize> {
    (0..requests.len())
        .filter(|&i| selector(&requests[i]) == id)
        .take(2)
        .collect()
}

/// The snapshot of `work` and `goals`, its goals' times in `tz`, through `gate`.
pub(crate) fn snapshot_at<Tz: TimeZone>(
    work: &Value,
    goals: &Value,
    now: i64,
    tz: &Tz,
    gate: &Gate,
) -> Value
where
    Tz::Offset: Display,
{
    let mut agents = Vec::new();
    let mut seen = HashMap::new();
    let requests: Vec<Value> = (arr(&work["requests"]).iter())
        .map(|r| request(r, &mut agents, &mut seen, tz))
        .collect();
    let areas: Vec<Value> = arr(&work["areas"]).iter().map(focus_area).collect();
    // The pages count unlinked work and read nothing of it.
    let unlinked = usize::try_from(work["unlinked"].as_u64().unwrap_or(0)).unwrap_or(0);
    let snapshot = json!({
        "version": 1, "generatedAt": iso(now), "revision": now.to_string(),
        "source": {"kind": "oboete", "connected": true, "error": null, "lastCheckedAt": iso(now)},
        "agents": agents, "humanRequests": requests, "unlinkedWork": vec![json!({}); unlinked],
        "focusAreas": areas, "goals": goals_list(work, goals, now, tz),
        "goalHistory": history(goals, now, tz),
        "ruleScopes": [{"id": "everywhere", "tier": "everywhere", "name": "Everywhere"}],
        "rules": [], "story": build_story(&work["story"], gate), "refresh": null,
        // oboete has no cloud sync: a valid "unconfigured" block (`sync-presentation.ts:23-36`)
        // shows "Cloud sync off" where a missing one shows "Cloud freshness unknown".
        "sync": {"configured": false, "state": "unconfigured", "mode": "disabled",
            "checkedAt": null, "observedAt": now,
            "message": "oboete keeps your work on this computer only.", "download": null,
            "projection": {"remaining": null},
            "completeness": {"pushQuarantine": 0, "pullQuarantine": 0, "excludedNative": null,
                "scope": "unknown"}},
    });
    leaving(snapshot, "", gate)
}

/// What the snapshot is made of, read as `GET /api/work-view` reads it, in the readers' hold
/// (#1089): My Work's requests of the last 7 days and the goals as their readers give them, each
/// text gated at its source and none of their keys, so that the counts, days and times the pages
/// need are read by the keys this file names whatever a rule finds in a key; and the Build story's
/// texts as they were kept (`build_story` gates the story it makes of them). Both gate their texts
/// under `rules`, the home's as the answer read them.
pub(crate) fn read(
    home: &Path,
    now: i64,
    rules: &crate::redact::Rules,
) -> anyhow::Result<(Value, Value)> {
    // The work-view shows no deliverables yet: their links are not made.
    let tag = |_: &crate::deliverables::Row| anyhow::Ok(String::new());
    let week = crate::my_work::Range::Week;
    let work = crate::my_work::listed_kept(home, week, now, &tag, rules)?;
    let goals = crate::my_work_goals::page_with(home, now, rules)?;
    Ok((work, goals))
}

/// The snapshot of a home with no store yet.
pub(crate) fn empty(now: i64) -> Value {
    snapshot_at(&json!({}), &json!({}), now, &Local, &Gate::now())
}

/// Marks each request and each conversation of `work` whose id goes by a reference in the
/// snapshot (`ref`): one My Work masked (its `fingerprint`), or one the gate would change in a
/// shape the snapshot writes it in (a conversation's id, its agent's id and its key; a request's
/// id). The reference is `m:` and the keyed print of the stored id, as My Work's own prints are
/// (`my_work::shown_id`): the same from read to read and in every request that names it, and
/// nothing of a guess at the id. No id My Work gives begins `m:` (its ids are `c:`, `r:` and
/// `g:`), so no request's or conversation's own id is another's reference, and each reference is
/// one stored id's (#1089). A group's id is its print alone already, and goes as it is. A focus
/// area's id, which the snapshot writes whole (an area's, a request's area, an ask's area) and
/// within a goals item's (`a:<id>`), and an open ask's id go so too, by the print of the id in a
/// namespace of its kind (Codex on #1093).
fn refer(home: &Path, work: &mut Value, gate: &Gate) -> anyhow::Result<()> {
    let mut key = None;
    let mut refer = |v: &mut Value, shapes: Vec<String>| -> anyhow::Result<()> {
        let id = v["id"].as_str().unwrap_or_default().to_owned();
        let print = v["fingerprint"].as_str().map(str::to_owned);
        let group = print.as_ref().is_some_and(|p| id == format!("g:{p}"));
        if (print.is_some() && !group) || shapes.iter().any(|s| gate.out(s) != *s) {
            // A masked id's fingerprint is the keyed print of the stored one; an id My Work shows
            // is the stored one.
            let print = match print {
                Some(print) => print,
                None => crate::my_work::keyed_print(&id, home, &mut key)?,
            };
            v["ref"] = json!(format!("m:{print}"));
        }
        Ok(())
    };
    let requests = work.get_mut("requests").and_then(Value::as_array_mut);
    for r in requests.into_iter().flatten() {
        let sessions = r.get_mut("sessions").and_then(Value::as_array_mut);
        for c in sessions.into_iter().flatten() {
            let agent = c["agent"].as_str().unwrap_or("").to_owned();
            let id = c["id"].as_str().unwrap_or("").to_owned();
            let shapes = vec![agent_id(&agent, &id), conversation_key(&agent, &id), id];
            refer(c, shapes)?;
        }
        let id = r["id"].as_str().unwrap_or("").to_owned();
        refer(r, vec![id])?;
    }
    let mut swap = |v: Option<&mut Value>, kind: &str| -> anyhow::Result<()> {
        let Some(v) = v else {
            return Ok(());
        };
        let Some(id) = v.as_str().map(str::to_owned) else {
            return Ok(());
        };
        // Other is a placement of the code's, not an area's id: the pages read it by its word.
        if kind == "area" && id == crate::my_work_focus::OTHER {
            return Ok(());
        }
        let mut shapes = vec![id.clone()];
        if kind == "area" {
            shapes.push(format!("a:{id}"));
        }
        if shapes.iter().any(|s| gate.out(s) != *s) {
            let print = crate::my_work::keyed_print(&format!("{kind}:{id}"), home, &mut key)?;
            *v = json!(format!("m:{print}"));
        }
        Ok(())
    };
    let requests = work.get_mut("requests").and_then(Value::as_array_mut);
    for r in requests.into_iter().flatten() {
        swap(r.get_mut("focus"), "area")?;
    }
    let areas = work.get_mut("areas").and_then(Value::as_array_mut);
    for a in areas.into_iter().flatten() {
        swap(a.get_mut("id"), "area")?;
        let asks = a.get_mut("asks").and_then(Value::as_array_mut);
        for k in asks.into_iter().flatten() {
            swap(k.get_mut("id"), "ask")?;
        }
    }
    let others = work.get_mut("other_asks").and_then(Value::as_array_mut);
    for k in others.into_iter().flatten() {
        swap(k.get_mut("id"), "ask")?;
    }
    Ok(())
}

/// A conversation's agent's id in the snapshot, by which the pages give a request its agent
/// (`agentIds`, `to-work-model.ts:154`): its platform, a word of this file's in place of the
/// agent's own name, which the raw hook takes as given, and the conversation's id or reference.
/// The conversation's id, `c:`, its agent's name and its session, goes as it is only where the gate
/// leaves it and each of these shapes as they are (`refer`); else its reference goes (#1089).
fn agent_id(agent: &str, id: &str) -> String {
    format!("{}:{id}", platform(agent))
}

/// A conversation's key, which the pages parse for its platform and id (`to-work-model.ts:88-92`).
fn conversation_key(agent: &str, id: &str) -> String {
    json!(["conversation", platform(agent), id]).to_string()
}

/// The keys of the snapshot's strings that its walk through the gate leaves as they are (#1089):
/// the selectors the pages match on (an id, a key, an agent's), whose mask would move or join what
/// they name, and the code's own words and times, which hold none of the owner's.
const OWN: &[&str] = &[
    "id",
    "agentIds",
    "conversationKeys",
    "contentSessionId",
    "contentFingerprint",
    "focusAreaId",
    "listId",
    // A list's name: `goals`, or a finished month, which the pages take as its id.
    "listName",
    "platformSource",
    "label",
    "kind",
    "status",
    "state",
    "source",
    "decidedBy",
    "titleSource",
    "tier",
    "mode",
    "scope",
    "url",
    "revision",
    "generatedAt",
    "lastCheckedAt",
    "updatedAt",
    "checkedAt",
    "observedAt",
    "at",
    "shippedAt",
    "latestAt",
    "periodStart",
    "periodEnd",
    "writtenAt",
];

/// `v` with each string under a key not in `OWN` through the gate as it is sent (#1089), and no
/// key gated: each is this file's. The Build story went through it whole as it was made.
fn leaving(v: Value, key: &str, gate: &Gate) -> Value {
    match v {
        Value::String(s) if !OWN.contains(&key) => Value::String(gate.out(&s)),
        Value::Array(xs) => Value::Array(xs.into_iter().map(|x| leaving(x, key, gate)).collect()),
        Value::Object(m) => Value::Object(
            (m.into_iter())
                .map(|(k, x)| {
                    let x = if k == "story" {
                        x
                    } else {
                        leaving(x, &k, gate)
                    };
                    (k, x)
                })
                .collect(),
        ),
        other => other,
    }
}

/// The Build story: the recap, then each week's chapter under its week. The mapper splits the
/// narrative at blank lines and takes its first block that is not a heading as the recap
/// (`to-work-model.ts:433-442`), so a story of chapters alone opens on its newest chapter, and a
/// recap of headings alone gives their text as its paragraphs, as the pages show a heading's. A
/// story with no paragraph at all, which the pages would not show, gives the text of its first
/// heading of its own, not a week's, as one.
///
/// `story` holds the texts as they were kept (`my_work::listed_kept`): the gate of their writing
/// read them text by text. The story made of them goes through the gate once, as it leaves
/// (#1089). What the gate hides is found before any of it is masked: in each kept text as it was
/// kept, in the story joined with none of the adapter's marks but its blank lines, in each block
/// as the pages split it, whole and with a heading's `#` marks off, and in the story as it is
/// sent. So no mask takes the context a rule needs elsewhere, and no word joiner of the adapter's
/// stands inside a match. Then the rules read each of those views again as the masks leave it,
/// until they find nothing new (`Narrative::leave`). Its own marks stay: the blank lines between
/// its blocks, a week heading's `## ` and a paragraph's word joiner.
fn build_story(story: &Value, gate: &Gate) -> Value {
    let recap = story["recap"].as_str().unwrap_or_default();
    let mut texts = vec![recap];
    let mut out = blocks(recap, 0);
    if !out.iter().any(|b| !b.heading) {
        out = out.iter().filter_map(|h| h.as_prose(recap)).collect();
    }
    for c in arr(&story["chapters"]) {
        if let (Some(week), Some(text)) = (c["week"].as_str(), c["text"].as_str()) {
            out.push(Block::week(week, texts.len()));
            texts.push(week);
            out.extend(blocks(text, texts.len()));
            texts.push(text);
        }
    }
    if out.iter().all(|b| b.heading) {
        let first = (0..out.len()).filter(|&i| !out[i].week).find_map(|i| {
            let shown = out[i].as_prose(texts[out[i].from])?;
            Some((i, shown))
        });
        if let Some((i, block)) = first {
            out[i] = block;
        }
    }
    if out.is_empty() {
        return Value::Null;
    }
    let mut n = Narrative::new(&texts);
    for (i, b) in out.iter().enumerate() {
        n.block(b, i > 0);
    }
    json!({"story": {"narrative": n.leave(&texts, gate), "writtenAt": null}})
}

/// The rules of one answer (`redact::rules_now`): none while their table is wrong, which masks
/// every text whole, as `redact::outbound` does.
pub(crate) struct Gate(Option<std::sync::Arc<crate::redact::Rules>>);

impl Gate {
    pub(crate) fn now() -> Gate {
        Gate(crate::redact::rules_now())
    }

    /// The version of its rules (`Rules::version`), which a change of a rule or a kept value
    /// changes; none while their table is wrong.
    fn version(&self) -> Option<&str> {
        self.0.as_deref().map(crate::redact::Rules::version)
    }

    /// `text` through the gate, as `redact::outbound` sends it.
    fn out(&self, text: &str) -> String {
        match &self.0 {
            Some(rules) => crate::redact::outbound_with(text, rules),
            None => crate::redact::MASK.to_owned(),
        }
    }

    /// What the gate hides in `text`, a stored text as it is stored, as oboete's own page reads it;
    /// none when it masks the whole of it.
    fn hidden(&self, text: &str) -> Option<Vec<(usize, usize)>> {
        crate::redact::hidden(text, self.0.as_ref()?)
    }

    /// What the gate hides in `text` read with its edges as they are (`redact::hidden_exact`),
    /// where the pages keep a character the gate's own reading trims (U+0085); none when it masks
    /// the whole of it.
    fn hidden_exact(&self, text: &str) -> Option<Vec<(usize, usize)>> {
        crate::redact::hidden_exact(text, self.0.as_ref()?)
    }
}

/// A block of a kept text as the pages read it: a heading line or a paragraph, trimmed as the
/// pages trim, each piece of it at its place in the kept text it is of (`from`: the recap, a week
/// or a chapter's text).
struct Block {
    pieces: Vec<Piece>,
    heading: bool,
    week: bool,
    joiner: bool,
    from: usize,
}

/// Words of a kept text at `at` in it; or the line break between two lines of a paragraph, which
/// stands for the kept text's own `\n` at `at`.
struct Piece {
    text: String,
    at: usize,
}

/// Where `part`, a slice of `text`, starts in it.
fn offset(text: &str, part: &str) -> usize {
    part.as_ptr() as usize - text.as_ptr() as usize
}

impl Block {
    fn heading(text: &str, line: &str, from: usize) -> Block {
        let line = line.trim_matches(edge);
        let pieces = vec![Piece {
            text: line.to_owned(),
            at: offset(text, line),
        }];
        Block {
            pieces,
            heading: true,
            week: false,
            joiner: false,
            from,
        }
    }

    /// A chapter's heading: its week, with `## ` before it.
    fn week(week: &str, from: usize) -> Block {
        let pieces = vec![Piece {
            text: week.to_owned(),
            at: 0,
        }];
        Block {
            pieces,
            heading: true,
            week: true,
            joiner: false,
            from,
        }
    }

    /// A paragraph of `lines` (slices of `text`) as the pages read it, or none when they would
    /// show it as nothing. The pages take a block for a heading when it opens with `#` once its
    /// bold and code marks are off, so a paragraph that would (an issue's number: `#1087`,
    /// `**#1087**`) opens with a word joiner, and the pages read it as the prose it is.
    fn prose(text: &str, lines: &[&str], from: usize) -> Option<Block> {
        static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
        static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
        let last = lines.len().checked_sub(1)?;
        let mut pieces = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                let at = offset(text, line) - 1;
                pieces.push(Piece {
                    text: "\n".to_owned(),
                    at,
                });
            }
            let mut words: &str = line;
            if i == 0 {
                words = words.trim_start_matches(edge);
            }
            if i == last {
                words = words.trim_end_matches(edge);
            }
            pieces.push(Piece {
                text: words.to_owned(),
                at: offset(text, words),
            });
        }
        let shown: String = pieces.iter().map(|p| p.text.as_str()).collect();
        let shown = BOLD.replace_all(&shown, "$1");
        let shown = CODE.replace_all(&shown, "$1");
        let first = shown.split(space).find(|w| !w.is_empty())?;
        let joiner = first.starts_with('#');
        Some(Block {
            pieces,
            heading: false,
            week: false,
            joiner,
            from,
        })
    }

    /// A heading as a paragraph of its text, its marks off, as the pages show a recap of headings
    /// alone; `text` is the kept text it is of.
    fn as_prose(&self, text: &str) -> Option<Block> {
        let p = self.pieces.first()?;
        let line = &text[p.at..p.at + p.text.len()];
        Block::prose(text, &[line.trim_start_matches('#')], self.from)
    }
}

/// A text's blocks as the pages read a narrative (`to-work-model.ts:433-442`): a Markdown heading
/// line is a block of its own, and the lines between headings and blank lines are a paragraph.
fn blocks(text: &str, from: usize) -> Vec<Block> {
    fn heading(line: &str) -> bool {
        let t = line.trim_start_matches(space);
        let n = t.bytes().take_while(|&b| b == b'#').count();
        (1..=6).contains(&n) && t[n..].chars().next().is_none_or(space)
    }
    let (mut out, mut lines) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let blank = line.chars().all(space);
        if blank || heading(line) {
            out.extend(Block::prose(text, &lines, from));
            lines.clear();
        }
        if heading(line) {
            out.push(Block::heading(text, line, from));
        } else if !blank {
            lines.push(line);
        }
    }
    out.extend(Block::prose(text, &lines, from));
    out
}

/// The Build story as it is made (#1089): `sent`, the narrative as it leaves, and which of its
/// bytes are the adapter's own marks (`mark`); for each kept text, where each of its bytes went in
/// `sent`, if it went (`went`); and `plain`, the story's words joined with none of the adapter's
/// marks but its blank lines, each of its bytes at `at` in `sent`.
struct Narrative {
    sent: String,
    mark: Vec<bool>,
    went: Vec<Vec<Option<usize>>>,
    plain: String,
    at: Vec<Option<usize>>,
}

/// The most times the story is read again for what its masks left: as many as the gate reads one
/// text (`redact`'s own bound). Still finding then, every word of it is masked.
const PASSES: usize = 64;

impl Narrative {
    fn new(texts: &[&str]) -> Narrative {
        Narrative {
            sent: String::new(),
            mark: Vec::new(),
            went: texts.iter().map(|t| vec![None; t.len()]).collect(),
            plain: String::new(),
            at: Vec::new(),
        }
    }

    /// `b`, after a blank line when it is not the first block.
    fn block(&mut self, b: &Block, after: bool) {
        if after {
            self.mark("\n\n", true);
        }
        if b.week {
            self.mark("## ", false);
        }
        if b.joiner {
            self.mark("\u{2060}", false);
        }
        for p in &b.pieces {
            let at = self.sent.len();
            let went = &mut self.went[b.from][p.at..p.at + p.text.len()];
            for (i, slot) in went.iter_mut().enumerate() {
                *slot = Some(at + i);
            }
            self.words(&p.text);
        }
    }

    /// One of the adapter's own marks in `sent`, and in `plain` too when it is the blank line
    /// between two blocks, so that no two blocks run together there.
    fn mark(&mut self, mark: &str, plain: bool) {
        let at = self.sent.len();
        self.sent.push_str(mark);
        self.mark.resize(self.sent.len(), true);
        if plain {
            self.plain.push_str(mark);
            self.at.extend((at..self.sent.len()).map(Some));
        }
    }

    /// A kept text's words, in both.
    fn words(&mut self, text: &str) {
        let at = self.sent.len();
        self.sent.push_str(text);
        self.mark.resize(self.sent.len(), false);
        self.plain.push_str(text);
        self.at.extend((at..self.sent.len()).map(Some));
    }

    /// `sent` with what the gate hides masked, found before any mask and then read again until it
    /// finds nothing new, as the gate reads one text (#1089). Each pass reads the views of the story
    /// as its masks so far leave it: the story as sent; its words joined with no mark of the
    /// adapter's but its blank lines; and each block of both as the pages split it, whole and, as
    /// the pages show every block, with a heading's leading `#` marks off, so that a paragraph is
    /// read with its word joiner and without it. The first pass, before
    /// any mask, reads each kept text as it was kept too. A pass reads all its views before it
    /// masks what they found, so that no mask of one takes the context a rule needs in another. The
    /// adapter's marks stay as they are: a mask that runs over one is cut on each side of it. Still
    /// finding after `PASSES`, every word is masked.
    fn leave(self, texts: &[&str], gate: &Gate) -> String {
        let mut masked = vec![false; self.sent.len()];
        let sent_at: Vec<Option<usize>> = (0..self.sent.len()).map(Some).collect();
        for pass in 0..=PASSES {
            let sent = shown(&self.sent, &sent_at, &masked);
            let plain = shown(&self.plain, &self.at, &masked);
            let blocks = [
                (&sent, page_blocks(&sent.0)),
                (&plain, page_blocks(&plain.0)),
            ];
            let mut found = Vec::new();
            // Each view is read where it is, none copied: a kept text as the gate reads a stored
            // one, and a view of the story as the pages receive or show it so too and with its
            // edges as they are, as a heading's text after its marks may open with a U+0085 the
            // pages keep and the gate's own reading trims (Codex on #1093).
            let mut read = |text: &str, at: &[Option<usize>], stored: bool| {
                let whole = || vec![(0, text.len())];
                let mut hidden = gate.hidden(text).unwrap_or_else(whole);
                if !stored {
                    hidden.extend(gate.hidden_exact(text).unwrap_or_else(whole));
                }
                for (s, e) in hidden {
                    let words = at[s..e].iter().flatten().copied();
                    found.extend(words.filter(|&p| !self.mark[p] && !masked[p]));
                }
            };
            for ((text, at), ranges) in &blocks {
                for r in ranges {
                    read(&text[r.clone()], &at[r.clone()], false);
                }
            }
            if pass == 0 {
                for (text, went) in texts.iter().zip(&self.went) {
                    read(text, went, true);
                }
            }
            read(&plain.0, &plain.1, false);
            read(&sent.0, &sent.1, false);
            if found.is_empty() {
                break;
            }
            if pass == PASSES {
                masked = self.mark.iter().map(|m| !m).collect();
                break;
            }
            for p in found {
                masked[p] = true;
            }
        }
        shown(&self.sent, &sent_at, &masked).0
    }
}

/// `text` with each run of its bytes that stand for a masked byte of the story as sent (`at`)
/// shown as one mask, and, for each byte of what comes back, the byte of the story it stands for:
/// none for a mask, which is masked already, or for what the story does not send.
fn shown(text: &str, at: &[Option<usize>], masked: &[bool]) -> (String, Vec<Option<usize>>) {
    let hidden = |i: usize| at[i].is_some_and(|p| masked[p]);
    let (mut out, mut from) = (
        String::with_capacity(text.len()),
        Vec::with_capacity(text.len()),
    );
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if hidden(i) {
            while chars.next_if(|&(j, _)| hidden(j)).is_some() {}
            out.push_str(crate::redact::MASK);
            from.resize(out.len(), None);
        } else {
            out.push(c);
            from.extend_from_slice(&at[i..i + c.len_utf8()]);
        }
    }
    (out, from)
}

/// Where the blocks of a narrative are as the pages split it (`to-work-model.ts:435`: at a line of
/// nothing but whitespace), each trimmed, and each that opens with `#` once more with those marks
/// and the whitespace after them off, as the pages show every block (`to-work-model.ts:439`).
fn page_blocks(text: &str) -> Vec<std::ops::Range<usize>> {
    // Where each blank line is: from a line break to the last line break of the whitespace after it.
    let mut cuts = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '\n' {
            continue;
        }
        let mut last = None;
        while let Some((j, w)) = chars.next_if(|&(_, w)| space(w)) {
            if w == '\n' {
                last = Some(j);
            }
        }
        if let Some(last) = last {
            cuts.push((i, last + 1));
        }
    }
    let starts = std::iter::once(0).chain(cuts.iter().map(|c| c.1));
    let ends = cuts.iter().map(|c| c.0).chain(std::iter::once(text.len()));
    let mut out = Vec::new();
    for (start, end) in starts.zip(ends) {
        let block = text[start..end].trim_matches(space);
        if block.is_empty() {
            continue;
        }
        let from = offset(text, block);
        out.push(from..from + block.len());
        let shown = block.trim_start_matches('#').trim_start_matches(space);
        if shown.len() < block.len() && !shown.is_empty() {
            let from = offset(text, shown);
            out.push(from..from + shown.len());
        }
    }
    out
}

/// A work item's status as the pages read it (`status-normalization.ts:22-47`), as one of the
/// words they read or as none: another spelling is text, which a rule may find as it is sent and
/// not as it was kept, and the pages read nothing of it (#1089).
fn item_status(status: &str) -> &'static str {
    const READ: [&str; 15] = [
        "needs-you",
        "needs-review",
        "blocked",
        "failed",
        "done",
        "completed",
        "dropped",
        "doing",
        "in-progress",
        "active",
        "running",
        "open",
        "todo",
        "planned",
        "queued",
    ];
    let status = crate::my_work::normalized(status);
    READ.into_iter().find(|w| *w == status).unwrap_or("")
}

/// JavaScript's `\s`, by which the pages split and trim: Rust's whitespace but U+0085, and U+FEFF.
fn space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}

/// What a block is trimmed of at its edges: JavaScript's whitespace, which the pages trim, and
/// Rust's, which the gate trims before it reads a text (`redact::hidden`). So no block of the
/// story opens or ends with a character the pages keep and the gate's reading drops (U+0085), and
/// a rule anchored at a block's edge reads the block as the pages show it (#1089).
fn edge(c: char) -> bool {
    space(c) || c.is_whitespace()
}

/// Unix ms as ISO time with 3 fraction digits, in UTC: the times the mapper parses.
fn iso(ms: i64) -> Value {
    (Utc.timestamp_millis_opt(ms).single()).map_or(Value::Null, |t| {
        json!(t.to_rfc3339_opts(SecondsFormat::Millis, true))
    })
}

/// Unix ms as ISO time in `tz`, with its offset: the goals list's, whose day the pages show as
/// written (`to-work-model.ts:321-325`).
fn zoned<Tz: TimeZone>(ms: i64, tz: &Tz) -> Value
where
    Tz::Offset: Display,
{
    (tz.timestamp_millis_opt(ms).single()).map_or(Value::Null, |t| {
        json!(t.to_rfc3339_opts(SecondsFormat::Millis, false))
    })
}

/// Unix ms as `YYYY-MM-DD HH:MM` in `tz`: how the mapper reads a goal's times
/// (`to-work-model.ts:259`), showing the digits as written.
fn stamp<Tz: TimeZone>(ms: &Value, tz: &Tz) -> Value {
    (ms.as_i64()
        .and_then(|ms| tz.timestamp_millis_opt(ms).single()))
    .map_or(Value::Null, |t| {
        json!(t.naive_local().format("%Y-%m-%d %H:%M").to_string())
    })
}

/// A civil day (`YYYY-MM-DD`) at noon with no zone: the browser reads it in its own zone, so the
/// day shown is the one written, wherever it is shown. None for anything else, a day the gate
/// masked among them: the pages would take it for a date they cannot read.
fn noon(day: &Value) -> Value {
    match day
        .as_str()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    {
        Some(d) => json!(format!("{d}T12:00:00")),
        None => Value::Null,
    }
}

fn s(v: &Value) -> Value {
    match v {
        Value::String(_) => v.clone(),
        _ => Value::Null,
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

/// claude-mem's platform for an oboete agent. The same word goes in the agents' ids and the
/// conversation keys, which find their agent by it (`to-work-model.ts:226`): one of oboete's own
/// agents' names, or `other` for any other, so that a name the raw hook took as given goes in no
/// selector. claude-mem shows a platform it does not know as the tool "other".
fn platform(agent: &str) -> &'static str {
    match agent {
        "agy" | "gemini" => "gemini",
        "claude" => "claude",
        "codex" => "codex",
        "grok" => "grok",
        "cursor" => "cursor",
        "opencode" => "opencode",
        "pi" => "pi",
        _ => "other",
    }
}

fn label(agent: &str) -> &'static str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "grok" => "Grok",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "pi" => "Pi",
        "agy" => "Antigravity",
        _ => "Agent",
    }
}

/// oboete's status as claude-mem's explicit lifecycle (`status-normalization.ts:94-107`).
fn lifecycle(status: &str) -> &'static str {
    match status {
        "needs" => "needs-you",
        "ready" => "needs-review",
        "working" => "in-progress",
        "blocked" => "blocked",
        "quiet" => "quiet",
        "todo" => "todo",
        "closed" => "closed",
        "shipped" => "shipped",
        _ => "unknown",
    }
}

/// The id the pages know a request by, as the snapshot gives it (`request`) and a check names it
/// (`named`, decision 117), matched whole: a conversation's own request goes by its
/// conversation's key, as claude-mem's does (its drawer gives that conversation the ask's time,
/// `to-work-model.ts:228`), with the conversation's reference there where it goes by one; any
/// other request goes by its reference where it goes by one, else by its id (`refer`).
fn selector(r: &Value) -> Value {
    let mut own = if r["ref"].is_string() {
        r["ref"].clone()
    } else {
        r["id"].clone()
    };
    for c in arr(&r["sessions"]) {
        let (agent, held) = (
            c["agent"].as_str().unwrap_or(""),
            c["id"].as_str().unwrap_or(""),
        );
        if r["id"] == held {
            own = json!(conversation_key(agent, c["ref"].as_str().unwrap_or(held)));
        }
    }
    own
}

/// One My Work request as claude-mem's human request; each conversation's agent goes in `agents`
/// once, titled by the conversation's own request, else by the first request that names it.
fn request<Tz: TimeZone>(
    r: &Value,
    agents: &mut Vec<Value>,
    seen: &mut HashMap<String, usize>,
    tz: &Tz,
) -> Value {
    let status = r["status"].as_str().unwrap_or("unknown");
    let own = selector(r);
    let mut agent_ids = Vec::new();
    let mut keys = Vec::new();
    for c in arr(&r["sessions"]) {
        let (agent, held) = (
            c["agent"].as_str().unwrap_or(""),
            c["id"].as_str().unwrap_or(""),
        );
        // A conversation whose id goes by a reference goes by it in each shape below (`refer`).
        let id = c["ref"].as_str().unwrap_or(held);
        let key = agent_id(agent, id);
        let at = *seen.entry(key.clone()).or_insert_with(|| {
            agents.push(json!({
                "id": key, "label": label(agent), "platformSource": platform(agent),
                "sessionTitle": s(&r["title"]),
                "project": arr(&r["projects"]).first().cloned().unwrap_or(Value::Null),
                "contentSessionId": id,
            }));
            agents.len() - 1
        });
        let conversation = conversation_key(agent, id);
        // A conversation's own request (`selector`): its title and project are the
        // conversation's, whichever request named it first.
        if r["id"] == held {
            agents[at]["sessionTitle"] = s(&r["title"]);
            agents[at]["project"] = arr(&r["projects"]).first().cloned().unwrap_or(Value::Null);
        }
        keys.push(json!(conversation));
        agent_ids.push(json!(key));
    }
    // A list-level update has no task: the list's name stands for it, as on oboete's own page. A
    // status is read as My Work reads it: the pages take a task as done by its word alone
    // (`to-work-model.ts:216`), so `Done` is `done` here too, and goes as one of the words they
    // read, or as none (`item_status`).
    let items: Vec<Value> = (arr(&r["work"]).iter().enumerate())
        .map(|(i, w)| {
            let title = if w["task"].is_string() {
                &w["task"]
            } else {
                &w["list"]
            };
            json!({"id": format!("w{i}"), "kind": "work-state", "title": s(title),
                "status": item_status(w["status"].as_str().unwrap_or("")), "nextStep": null,
                "lastUpdateEpoch": w["updated"], "sourceRefs": [], "url": ""})
        })
        .collect();
    // Each work list is a workstream, as claude-mem's drawer shows a request's tasks by stream
    // (`to-work-model.ts:214-217`). A list is one per repository, as oboete keeps it; a masked
    // one is known by its fingerprint, in a namespace of its own. The pages use a workstream's id
    // only to find its title in the request's copy, so it goes by its place among the request's,
    // which carries no name: a rule can find in a repository's name and a list's, joined, what it
    // finds in neither (Codex on #1087).
    let mut streams: Vec<(String, Value, Vec<Value>)> = Vec::new();
    for (w, item) in arr(&r["work"]).iter().zip(&items) {
        let id = match w["stream"].as_str() {
            Some(print) => format!("masked:{print}"),
            None => json!([&w["repo"], &w["list"]]).to_string(),
        };
        match streams.iter_mut().find(|(known, ..)| *known == id) {
            Some((.., of)) => of.push(item.clone()),
            None => streams.push((id, s(&w["list"]), vec![item.clone()])),
        }
    }
    let mut copy = Map::new();
    copy.insert("state".into(), json!("current"));
    for field in ["title", "summary", "next"] {
        if r[field].is_string() {
            copy.insert(field.into(), r[field].clone());
        }
    }
    let titles: Vec<Value> = (streams.iter().enumerate())
        .map(|(i, (_, title, _))| json!({"id": format!("w{i}"), "title": title}))
        .collect();
    copy.insert("workstreams".into(), json!(titles));
    let workstreams: Vec<Value> = (streams.into_iter().enumerate())
        .map(
            |(i, (_, title, items))| json!({"id": format!("w{i}"), "title": title, "items": items}),
        )
        .collect();
    let activity: Vec<Value> = (arr(&r["updates"]).iter().enumerate())
        .map(|(i, u)| {
            json!({"id": format!("u{i}"), "kind": "work-state", "title": s(&u["text"]),
                "detail": null, "atEpoch": u["ts"], "url": ""})
        })
        .collect();
    let follow: Vec<Value> = (arr(&r["follow_ups"]).iter())
        .map(|f| json!({"text": s(&f["text"]), "atEpoch": f["ts"]}))
        .collect();
    // The review the page shows is its newest reading: its own percent and state go, as
    // claude-mem's review does. A request shipped by its status after a review below 100 is
    // 100% by its status, not by its review (`to-work-model.ts:198-200`), and the pages check
    // that the percent is the newest reading's (`data.ts:282`).
    let progress = &r["progress"];
    let readings: Vec<Value> = (arr(&progress["readings"]).iter())
        .filter_map(|r| {
            Some(json!({"at": iso(r["ts"].as_i64()?), "percent": r["percent"].as_f64()?}))
        })
        .collect();
    // A review goes with its own percent, its newest reading's, or not at all: the request's
    // percent is another's when a status shipped it after a review at 30%.
    let reviewed =
        (progress["reviewed"].as_i64()).zip(readings.last().and_then(|r| r["percent"].as_f64()));
    // A review at 100% of a shipped request goes as a shipped review, with the day or time it
    // shipped: the pages take that day for the request's completion when no status gave one
    // (`to-work-model.ts:148-149`), in the browser's zone, as the day is written with none. Any
    // other review has no shipped day.
    let shipped_review =
        reviewed.is_some_and(|(_, percent)| status == "shipped" && percent >= 100.0);
    let shipped_at = match (shipped_review, progress["completed_at"].as_i64()) {
        (false, _) => Value::Null,
        (true, Some(at)) => iso(at),
        (true, None) => noon(&progress["completed"]),
    };
    // When the newest status recorded completed the request: a done one's clock stops by it. A
    // request shipped by its review alone has no such time: its shipped review's day completes it.
    // When the review cannot carry that day (it is below 100% or does not go, or its day is not
    // one), the day at noon on this machine does, and the request's last update is the last
    // resort: a shipped request needs a completion (`data.ts:276`).
    let noon_here = (progress["completed"].as_str())
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(12, 0, 0))
        .and_then(|t| tz.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis());
    let completed = match (r["completed_at"].as_i64(), status) {
        (Some(at), _) => json!(at),
        (None, "shipped") if !shipped_at.is_null() => Value::Null,
        (None, "shipped") => noon_here.map_or_else(|| r["updated"].clone(), |at| json!(at)),
        _ => Value::Null,
    };
    // A review's progress, when a review was recorded: a request shipped by its status alone is
    // complete by definition (`to-work-model.ts:198-200`), with no reading of its own.
    let story = &r["story"];
    let progress = match reviewed {
        Some((reviewed, percent)) => json!({
            "state": if shipped_review { "shipped" } else { "in-progress" },
            "percent": percent,
            "checkedAt": iso(reviewed),
            "shippedAt": shipped_at,
            "readings": readings,
            "narrative": s(&story["narrative"]),
            "milestones": (arr(&story["milestones"]).iter())
                .map(|m| json!({"at": noon(&m["date"]), "text": s(&m["text"])}))
                .collect::<Vec<_>>(),
        }),
        None => Value::Null,
    };
    let mut out = json!({
        "id": own, "contentFingerprint": r["version"],
        "title": s(&r["title"]),
        "titleSource": if r["title"].is_string() { json!("title-cache") } else { Value::Null },
        "humanCopy": Value::Object(copy),
        "original": {"text": s(&r["ask"]["text"]), "atEpoch": r["ask"]["ts"]},
        "scopeUpdates": follow, "items": items, "workstreams": workstreams, "activity": activity,
        "agentIds": agent_ids, "conversationKeys": keys, "requestedBy": null,
        "startedAtEpoch": r["started"], "updatedAtEpoch": r["updated"],
        "completedAtEpoch": completed,
        "lifecycle": {"status": lifecycle(status), "explicit": true, "complete": null,
            "atEpoch": null},
        "review": {"checked": !r["checked"].is_null(), "checkedAtEpoch": r["checked"],
            "available": status == "ready"},
        "progress": progress, "anchorConversationKey": null,
    });
    if r["focus"].is_string() {
        out["focusAreaId"] = r["focus"].clone();
    }
    out
}

/// A focus area as claude-mem lists them. A nameless area sorts by an empty name: the pages
/// compare names whole (`to-focus-model.ts:23`), and a null would stop every screen.
fn focus_area(a: &Value) -> Value {
    json!({
        "focusAreaId": a["id"], "name": a["name"].as_str().unwrap_or(""), "goal": s(&a["goal"]),
        "about": s(&a["about"]), "status": a["status"], "priorityRank": a["priority_rank"],
        "ownerRank": a["owner_rank"], "source": a["source"], "mergedInto": null,
        "evidence": s(&a["evidence"]), "decidedBy": a["decided_by"], "savedAt": null,
    })
}

fn evidence<Tz: TimeZone>(quotes: &Value, tz: &Tz) -> Vec<Value> {
    (arr(quotes).iter())
        .map(|q| json!({"at": stamp(&q["ts"], tz), "quote": s(&q["text"])}))
        .collect()
}

/// A goal item as claude-mem's goals list holds it (`WorkViewGoalItem`), every field of its type.
fn goal_item(item: Value) -> Value {
    let mut out = json!({"task": "", "goal": null, "status": "todo"});
    if let (Value::Object(base), Value::Object(item)) = (&mut out, item) {
        base.extend(item);
    }
    out
}

/// The goals list the focus screens read (`to-work-model.ts:338-357`), from the newest goals the
/// page shows: an item per area that was asked for, and one per open ask, under its area or under
/// none. oboete's `areas[]` already fold its priorities in. None when no goals were written.
fn goals_list<Tz: TimeZone>(work: &Value, page: &Value, now: i64, tz: &Tz) -> Value
where
    Tz::Offset: Display,
{
    let Some(written) = page["current"]["written"].as_i64() else {
        return Value::Null;
    };
    let mut areas = Vec::new();
    let mut asks = Vec::new();
    for a in arr(&work["areas"]) {
        // An area with open asks and no priority under it has no count of its own (`asked` is
        // null): it has no item of its own, and its asks stay under it.
        if let Some(asked) = a["asked"].as_f64() {
            areas.push(goal_item(json!({
                "id": format!("a:{}", a["id"].as_str().unwrap_or("")), "kind": "focus-area",
                "rank": areas.len() + 1, "title": s(&a["name"]), "goal": s(&a["goal"]),
                "focusArea": s(&a["name"]), "focusAreaId": a["id"], "product": null,
                "latestAt": stamp(&a["latest"], tz), "timesAsked": asked,
                "evidence": evidence(&a["quotes"], tz),
            })));
        }
        asks.extend(
            arr(&a["asks"])
                .iter()
                .map(|k| (k, s(&a["name"]), a["id"].clone())),
        );
    }
    asks.extend(
        arr(&work["other_asks"])
            .iter()
            .map(|k| (k, Value::Null, Value::Null)),
    );
    let asks: Vec<Value> = (asks.into_iter().enumerate())
        .map(|(i, (k, area, id))| {
            goal_item(json!({"id": s(&k["id"]), "kind": "ask", "rank": i + 1,
                "title": s(&k["title"]), "focusArea": area, "focusAreaId": id, "product": null,
                "latestAt": stamp(&k["latest"], tz),
                "timesAsked": k["asked"].as_f64().unwrap_or(0.0),
                "evidence": evidence(&k["quotes"], tz)}))
        })
        .collect();
    // The page reads the window that ends now, its quotes and its prompts (`my_work_goals::page`),
    // not the one that ended at the run: its days are the period's, the run's time its own.
    let days = page["window"]["days"].as_i64();
    let day = |ms: i64| tz.timestamp_millis_opt(ms).single().map(|t| t.date_naive());
    let start = days
        .and_then(|d| day(now - d * 86_400_000))
        .map(|d| d.to_string());
    let end = (day(now).and_then(|d| d.succ_opt())).map(|d| d.to_string());
    json!({
        "listId": "goals", "listName": "goals", "history": false, "project": "",
        "updatedAt": iso(now), "periodStart": start, "periodEnd": end, "summary": null,
        "generatedAt": zoned(written, tz),
        "windowDays": page["window"]["days"], "sourcePromptCount": page["window"]["prompts"],
        "mapPath": null, "areas": areas, "asks": asks,
    })
}

/// Past focus (`to-work-model.ts:309-318`): each finished month of goals as a list of its own,
/// from its first day to the next month's first, its priorities as its areas.
fn history<Tz: TimeZone>(page: &Value, now: i64, tz: &Tz) -> Vec<Value> {
    (arr(&page["months"]).iter())
        .filter_map(|m| {
            let month = m["month"].as_str()?;
            let start = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()?;
            let end = start.checked_add_months(Months::new(1))?;
            let areas: Vec<Value> = (arr(&m["priorities"]).iter().enumerate())
                .map(|(i, p)| {
                    let quote = &p["quote"];
                    let evidence: Vec<Value> = (quote.is_object())
                        .then(|| json!({"at": stamp(&quote["ts"], tz), "quote": s(&quote["text"])}))
                        .into_iter()
                        .collect();
                    // A priority's product names it where no area does; an empty one, none.
                    let product = (p["product"].as_str()).filter(|t| !t.is_empty());
                    goal_item(
                        json!({"id": format!("p:{month}:{}", i + 1), "kind": "focus-area",
                        "rank": i + 1, "title": s(&p["title"]), "focusArea": null,
                        "focusAreaId": null, "product": product, "latestAt": null,
                        "timesAsked": p["asked"].as_f64().unwrap_or(0.0), "evidence": evidence}),
                    )
                })
                .collect();
            Some(json!({
                "listId": format!("goals:{month}"), "listName": month, "history": true,
                "project": "", "updatedAt": iso(now), "periodStart": start.to_string(),
                "periodEnd": end.to_string(), "summary": s(&m["summary"]), "generatedAt": null,
                "windowDays": null, "sourcePromptCount": m["prompts"], "mapPath": null,
                "areas": areas, "asks": [],
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests;
