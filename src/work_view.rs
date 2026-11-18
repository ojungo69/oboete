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

/// The snapshot, its goals' times in this machine's zone: the pages show those as written.
pub(crate) fn snapshot(work: &Value, goals: &Value, now: i64) -> Value {
    snapshot_at(work, goals, now, &Local)
}

/// `snapshot`, its goals' times in `tz`.
pub(crate) fn snapshot_at<Tz: TimeZone>(work: &Value, goals: &Value, now: i64, tz: &Tz) -> Value
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
        "rules": [], "story": build_story(&work["story"]), "refresh": null,
        // oboete has no cloud sync: a valid "unconfigured" block (`sync-presentation.ts:23-36`)
        // shows "Cloud sync off" where a missing one shows "Cloud freshness unknown".
        "sync": {"configured": false, "state": "unconfigured", "mode": "disabled",
            "checkedAt": null, "observedAt": now,
            "message": "oboete keeps your work on this computer only.", "download": null,
            "projection": {"remaining": null},
            "completeness": {"pushQuarantine": 0, "pullQuarantine": 0, "excludedNative": null,
                "scope": "unknown"}},
    });
    leaving(snapshot, "")
}

/// What the snapshot is made of, read as `GET /api/work-view` reads it, in the readers' hold
/// (#1089): My Work's requests of the last 7 days and the goals as their readers give them, each
/// text gated at its source and none of their keys, so that the counts, days and times the pages
/// need are read by the keys this file names whatever a rule finds in a key; the Build story's
/// texts as they were kept (`build_story` gates the story it makes of them); and each conversation
/// the gate would mask in a shape this file writes it in, marked with its reference (`refer`).
pub(crate) fn read(home: &Path, now: i64) -> anyhow::Result<(Value, Value)> {
    // The work-view shows no deliverables yet: their links are not made.
    let tag = |_: &crate::deliverables::Row| anyhow::Ok(String::new());
    let mut work = crate::my_work::listed(home, crate::my_work::Range::Week, now, &tag)?;
    let goals = crate::my_work_goals::page(home, now)?;
    work["story"] = crate::my_work_story::kept(home)?;
    refer(home, &mut work)?;
    Ok((work, goals))
}

/// Marks each conversation of `work`'s requests whose id the gate would change in a shape the
/// snapshot writes it in (its agent's id, its key: `request`) with a reference that stands for it
/// there: `c:[REDACTED]#` and a keyed print of the id, the same from read to read and in every
/// request that names the conversation, and nothing of a guess at the id, as My Work's own prints
/// are (`my_work::shown_id`).
fn refer(home: &Path, work: &mut Value) -> anyhow::Result<()> {
    let mut key = None;
    let requests = work.get_mut("requests").and_then(Value::as_array_mut);
    for r in requests.into_iter().flatten() {
        let sessions = r.get_mut("sessions").and_then(Value::as_array_mut);
        for c in sessions.into_iter().flatten() {
            let agent = c["agent"].as_str().unwrap_or("").to_owned();
            let id = c["id"].as_str().unwrap_or("").to_owned();
            let shapes = [
                id.clone(),
                agent_id(&agent, &id),
                conversation_key(&agent, &id),
            ];
            if shapes.iter().any(|s| crate::redact::outbound(s) != *s) {
                let print = crate::my_work::keyed_print(&id, home, &mut key)?;
                c["ref"] = json!(format!("c:{}#{print}", crate::redact::MASK));
            }
        }
    }
    Ok(())
}

/// A conversation's agent's id in the snapshot, by which the pages give a request its agent
/// (`agentIds`, `to-work-model.ts:154`).
fn agent_id(agent: &str, id: &str) -> String {
    format!("{agent}:{id}")
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
fn leaving(v: Value, key: &str) -> Value {
    match v {
        Value::String(s) if !OWN.contains(&key) => Value::String(crate::redact::outbound(&s)),
        Value::Array(xs) => Value::Array(xs.into_iter().map(|x| leaving(x, key)).collect()),
        Value::Object(m) => Value::Object(
            (m.into_iter())
                .map(|(k, x)| {
                    let x = if k == "story" { x } else { leaving(x, &k) };
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
/// `story` holds the texts as they were kept (`my_work_story::kept`): the gate of their writing
/// read them text by text. The story made of them goes through the gate once, as it leaves
/// (#1089). What the gate hides is found before any of it is masked: in the story joined with none
/// of the adapter's marks but its blank lines, in each kept text, each block and each heading as
/// the pages show it (its `#` marks off), and in the story as it is sent. So no mask takes the
/// context a rule needs elsewhere, and no word joiner of the adapter's stands inside a match. Its
/// own marks stay: the blank lines between its blocks, a week heading's `## ` and a paragraph's
/// word joiner.
fn build_story(story: &Value) -> Value {
    let recap = story["recap"]
        .as_str()
        .map(|t| blocks(t, 0))
        .unwrap_or_default();
    let mut out: Vec<Block> = match recap.iter().any(|b| !b.heading) {
        true => recap,
        false => (recap.iter())
            .filter_map(|h| Block::prose(h.text.trim_start_matches('#'), 0))
            .collect(),
    };
    for (i, c) in arr(&story["chapters"]).iter().enumerate() {
        if let (Some(week), Some(text)) = (c["week"].as_str(), c["text"].as_str()) {
            out.push(Block::week(week, 2 * i + 1));
            out.extend(blocks(text, 2 * i + 2));
        }
    }
    if out.iter().all(|b| b.heading) {
        let first = (0..out.len()).filter(|&i| !out[i].week).find_map(|i| {
            let shown = Block::prose(out[i].text.trim_start_matches('#'), out[i].from)?;
            Some((i, shown))
        });
        if let Some((i, block)) = first {
            out[i] = block;
        }
    }
    if out.is_empty() {
        return Value::Null;
    }
    let mut n = Narrative::default();
    for (i, b) in out.iter().enumerate() {
        n.block(b, i > 0);
    }
    json!({"story": {"narrative": n.leave(), "writtenAt": null}})
}

/// A block of a kept text as the pages read it: a heading line or a paragraph, trimmed as the
/// pages trim, and the kept text it is of (`from`: the recap, a week or a chapter's text).
struct Block {
    text: String,
    heading: bool,
    week: bool,
    joiner: bool,
    from: usize,
}

impl Block {
    fn heading(line: &str, from: usize) -> Block {
        let text = line.trim_matches(space).to_owned();
        Block {
            text,
            heading: true,
            week: false,
            joiner: false,
            from,
        }
    }

    /// A chapter's heading: its week, with `## ` before it.
    fn week(week: &str, from: usize) -> Block {
        let text = week.to_owned();
        Block {
            text,
            heading: true,
            week: true,
            joiner: false,
            from,
        }
    }

    /// A paragraph as the pages read it, or none when they would show it as nothing. The pages
    /// take a block for a heading when it opens with `#` once its bold and code marks are off, so
    /// a paragraph that would (an issue's number: `#1087`, `**#1087**`) opens with a word joiner,
    /// and the pages read it as the prose it is.
    fn prose(block: &str, from: usize) -> Option<Block> {
        static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
        static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
        let text = block.trim_matches(space);
        let shown = BOLD.replace_all(text, "$1");
        let shown = CODE.replace_all(&shown, "$1");
        let first = shown.split(space).find(|w| !w.is_empty())?;
        let joiner = first.starts_with('#');
        let text = text.to_owned();
        Some(Block {
            text,
            heading: false,
            week: false,
            joiner,
            from,
        })
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
            out.extend(Block::prose(&lines.join("\n"), from));
            lines.clear();
        }
        if heading(line) {
            out.push(Block::heading(line, from));
        } else if !blank {
            lines.push(line);
        }
    }
    out.extend(Block::prose(&lines.join("\n"), from));
    out
}

/// The Build story as it is made (#1089): `sent`, the narrative as it leaves, the adapter's marks
/// in it at `marks`; `plain`, the kept texts' words joined with none of those marks but the blank
/// lines, each of its bytes at `at` in `sent`; and the parts of `plain` the gate reads alone too:
/// each kept text (`texts`), each block and each heading as the pages show it (`views`).
#[derive(Default)]
struct Narrative {
    sent: String,
    marks: Vec<(usize, usize)>,
    plain: String,
    at: Vec<usize>,
    texts: Vec<(usize, std::ops::Range<usize>)>,
    views: Vec<std::ops::Range<usize>>,
}

impl Narrative {
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
        let start = self.plain.len();
        self.words(&b.text);
        let block = start..self.plain.len();
        // A heading as the pages show it, its marks off.
        if b.heading && !b.week {
            let shown = b.text.trim_start_matches('#').trim_start_matches(space);
            self.views.push(block.end - shown.len()..block.end);
        }
        match self.texts.last_mut() {
            Some((from, text)) if *from == b.from => text.end = block.end,
            _ => self.texts.push((b.from, block.clone())),
        }
        self.views.push(block);
    }

    /// One of the adapter's own marks in `sent`, and in `plain` too when it is the blank line
    /// between two blocks, so that no two blocks run together there.
    fn mark(&mut self, mark: &str, plain: bool) {
        let at = self.sent.len();
        self.sent.push_str(mark);
        self.marks.push((at, self.sent.len()));
        if plain {
            self.plain.push_str(mark);
            self.at.extend(at..self.sent.len());
        }
    }

    /// A kept text's words, in both.
    fn words(&mut self, text: &str) {
        let at = self.sent.len();
        self.sent.push_str(text);
        self.plain.push_str(text);
        self.at.extend(at..self.sent.len());
    }

    /// `sent` with what the gate hides in `plain`, in each part of it read alone or in `sent`
    /// itself, all found before any of it is masked, and masked once. The adapter's marks stay as
    /// they are: a mask that runs over one is split on each side of it.
    fn leave(self) -> String {
        let parts = self.texts.iter().map(|(_, t)| t.clone());
        let views = std::iter::once(0..self.plain.len())
            .chain(parts)
            .chain(self.views.iter().cloned());
        let mut runs = Vec::new();
        for v in views.filter(|v| !v.is_empty()) {
            match crate::redact::hidden_now(&self.plain[v.clone()]) {
                Some(found) => {
                    runs.extend(found.into_iter().map(|(s, e)| (v.start + s, v.start + e)))
                }
                None => runs.push((v.start, v.end)),
            }
        }
        let mut sent: Vec<(usize, usize)> = (runs.into_iter())
            .filter(|(s, e)| s < e)
            .map(|(s, e)| (self.at[s], self.at[e - 1] + 1))
            .collect();
        match crate::redact::hidden_now(&self.sent) {
            Some(found) => sent.extend(found),
            None => sent.push((0, self.sent.len())),
        }
        let runs = outside(&crate::redact::merged_runs(sent), &self.marks);
        crate::redact::masked_part(&self.sent, 0..self.sent.len(), &runs)
    }
}

/// `runs` (sorted, apart) with what falls in `marks` (sorted, apart) taken out of them.
fn outside(runs: &[(usize, usize)], marks: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(start, end) in runs {
        let mut from = start;
        for &(m, n) in marks.iter().filter(|&&(m, n)| n > start && m < end) {
            if m > from {
                out.push((from, m));
            }
            from = from.max(n);
        }
        if from < end {
            out.push((from, end));
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

/// claude-mem's platform for an oboete agent. The same name goes in the conversation keys, which
/// find their agent by it (`to-work-model.ts:226`); a name claude-mem does not know shows as the
/// tool "other".
fn platform(agent: &str) -> &str {
    match agent {
        "agy" => "gemini",
        a => a,
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

/// One My Work request as claude-mem's human request; each conversation's agent goes in `agents`
/// once, titled by the conversation's own request, else by the first request that names it.
fn request<Tz: TimeZone>(
    r: &Value,
    agents: &mut Vec<Value>,
    seen: &mut HashMap<String, usize>,
    tz: &Tz,
) -> Value {
    let status = r["status"].as_str().unwrap_or("unknown");
    let mut own = r["id"].clone();
    let mut agent_ids = Vec::new();
    let mut keys = Vec::new();
    for c in arr(&r["sessions"]) {
        let (agent, held) = (
            c["agent"].as_str().unwrap_or(""),
            c["id"].as_str().unwrap_or(""),
        );
        // A conversation the gate would mask in one of the shapes below goes by its reference
        // there (`refer`).
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
        // A conversation's own request is known by the conversation's key, as claude-mem's is:
        // its drawer gives that conversation the ask's time (`to-work-model.ts:228`). A write
        // that names it takes oboete's id back from the key's last part, or from the reference
        // there, whose print is the id's.
        if r["id"] == held {
            own = json!(conversation);
            // Its title and project are the conversation's, whichever request named it first.
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
