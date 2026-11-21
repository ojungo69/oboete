use super::*;
use crate::my_work_ops::{MyWorkOp, Read};
use crate::raw::{Event, OpKind};
use crate::search::b::fixture::Store;
use chrono::{Datelike, FixedOffset, Timelike};
use std::collections::BTreeMap;
use std::path::Path;

const R: &str = "github.com/o/r";
/// A second repository, with a work list of the same name as one in `R`.
const R2: &str = "github.com/o/other";
/// The home's two focus areas: one its goals ask for, and one of an open ask alone.
const PARSER: &str = "fa_aaaaaaaaaaaa";
const DOCS: &str = "fa_bbbbbbbbbbbb";

/// An event of `agent`'s `session` in the home's repository at `ts`: its seq.
fn event(h: &mut Store, kind: &str, agent: &str, session: &str, ts: i64, body: Value) -> i64 {
    let e = Event {
        kind: kind.into(),
        agent: agent.into(),
        session: session.into(),
        repo: Some(R.into()),
        branch: Some("main".into()),
        ts,
        ..crate::raw::test_event(&body.to_string())
    };
    h.raw.append(&e).unwrap()
}

/// A work state write as oboete's server appends it, and the call of `agent`'s `session` that
/// sent it, 20 ms after it.
fn wrote(h: &mut Store, agent: &str, session: &str, list: &str, fields: Value) {
    let fields = fields.as_object().unwrap().clone();
    let seq = h.raw.work_state(R, list, &fields, None).unwrap();
    let ts: i64 = rusqlite::Connection::open(h.home.path().join("raw.db"))
        .unwrap()
        .query_row(
            "SELECT ts FROM ops WHERE device = ?1 AND op_seq = ?2",
            rusqlite::params![h.raw.device(), seq],
            |r| r.get(0),
        )
        .unwrap();
    let input = json!({"list": list, "fields": fields}).to_string();
    let body = json!({"tool": "mcp__oboete__work_state_write", "input": input,
        "output": "Saved.", "failed": false});
    event(h, "tool", agent, session, ts + 20, body);
}

/// A work state write of `repo` naming its request itself (`request_id`), as oboete's server
/// appends it.
fn noted(h: &mut Store, repo: &str, list: &str, fields: Value) {
    h.raw
        .work_state(repo, list, fields.as_object().unwrap(), None)
        .unwrap();
}

/// A progress review of the request `about` at `ts`, as the review job appends it.
fn reviewed(h: &mut Store, about: &str, ts: i64, percent: i64) {
    let fields = json!({"state": "in-progress", "percent": percent, "shipped": "",
        "narrative": "The migration came first.", "milestones": []});
    review(h, about, ts, fields);
}

/// A review at `ts` that found the request shipped on `day`, at 100%.
fn shipped_review(h: &mut Store, about: &str, ts: i64, day: &str) {
    let fields = json!({"state": "shipped", "percent": 100, "shipped": day,
        "narrative": "The docs went out.", "milestones": []});
    review(h, about, ts, fields);
}

/// A review of the request `about` at `ts`, as the progress job appends it.
fn review(h: &mut Store, about: &str, ts: i64, fields: Value) {
    let op = MyWorkOp {
        job: crate::my_work_progress::NAME.into(),
        about: about.into(),
        entry: "fake".into(),
        ts,
        read: Read::default(),
        removed: vec![],
        fields: fields.as_object().unwrap().clone().into_iter().collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
}

/// The Build story's recap at `ts`, as the story job appends it.
fn storied(h: &mut Store, ts: i64, recap: &str) {
    let op = MyWorkOp {
        job: crate::my_work_story::NAME.into(),
        about: crate::my_work_story::RECAP.into(),
        entry: "fake".into(),
        ts,
        read: Read::default(),
        removed: vec![],
        fields: BTreeMap::from([("text".to_owned(), json!(recap))]),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
}

/// A focus area the owner keeps.
fn kept(h: &Store, id: &str, name: &str) {
    let area = json!({"kind": "area", "name": name, "status": "kept", "source": "owner",
        "decided_by": "owner"});
    crate::my_work_ops::record(h.home.path(), "focus", vec![(id.into(), area)]).unwrap();
}

/// The goals op of `ts` over prompt `seq`, with `items`.
fn goals_op(h: &mut Store, ts: i64, seq: i64, items: Value, summary: &str) {
    let op = MyWorkOp {
        job: "goals".into(),
        about: Utc
            .timestamp_millis_opt(ts)
            .unwrap()
            .format("%Y-%m-%d")
            .to_string(),
        entry: "fake".into(),
        ts,
        read: Read {
            spans: vec![(seq, seq)],
            ops: vec![],
        },
        removed: vec![],
        fields: BTreeMap::from([
            ("items".to_owned(), items),
            ("summary".to_owned(), json!(summary)),
        ]),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
}

/// A home of the last minutes: Claude's requests that need the owner, that shipped, that are done,
/// that two reviews found rising and that shipped after a review at 30%, Codex's that only asked, and a request of its own name with a
/// list of one name in two repositories. Goals written over the first prompt ask for the parser (an area with an open
/// ask), leave an open ask in an area nothing was asked for in (its `asked` is null), and one open
/// ask under no area. Goals of the 15th, two months back, make a finished month of Past focus.
fn home() -> (Store, i64) {
    let mut h = Store::new();
    let t = crate::db::now_ms() - 10 * 60_000;
    // Its archived month's prompt and goals write: noon on the 15th, two months back (UTC, the
    // contract's zone), so the two share a month and are past the goals' 31 days, whatever the day.
    let month = (Utc
        .timestamp_millis_opt(t)
        .unwrap()
        .date_naive()
        .with_day(1)
        .unwrap())
        - Months::new(2);
    let old = (month.with_day(15).unwrap().and_hms_opt(12, 0, 0).unwrap())
        .and_utc()
        .timestamp_millis();
    let s0 = h.said("s0", R, old, "Rename the lexer's tokens.");
    let lexer = json!([{"id": "o1", "kind": "priority", "rank": 1, "title": "Lexer",
        "product": "", "asked": 1, "quotes": [{"seq": s0, "ts": old, "text": "the lexer's tokens"}]}]);
    goals_op(&mut h, old + 60_000, s0, lexer, "Worked on the lexer.");
    let said = "Fix the parser's error messages, add a header test, write docs and fail fast.";
    let s1 = h.said("s1", R, t, said);
    let needs = json!({"task": "Report the line", "status": "Needs_Input"});
    wrote(&mut h, "claude", "s1", "parser", needs);
    h.said("s2", R, t + 60_000, "Ship the release notes.");
    let shipped = json!({"task": "Notes", "status": "doing", "request_status": "shipped"});
    wrote(&mut h, "claude", "s2", "release", shipped);
    h.said("s3", R, t + 90_000, "Write the changelog.");
    let done = json!({"task": "Changelog", "status": "doing", "request_status": "done"});
    wrote(&mut h, "claude", "s3", "changelog", done);
    h.said("s4", R, t + 100_000, "Write the migration.");
    let migration = json!({"task": "Migration", "status": "doing"});
    wrote(&mut h, "claude", "s4", "migration", migration);
    for (at, percent) in [(t + 240_000, 20), (t + 480_000, 50)] {
        reviewed(&mut h, "c:claude:s4", at, percent);
    }
    // The Build story's recap: its paragraphs open with an issue's number, in bold and as code,
    // and its heading sits above its body.
    let recap = "**#1087** shipped the read adapter.\n\n`#1088` fixed the gate.\n### Next\nServe the pages.";
    storied(&mut h, t + 360_000, recap);
    // A review at 30%, then a status that ships it: 100% by its status, its review its own.
    h.said("s5", R, t + 110_000, "Deploy the release.");
    reviewed(&mut h, "c:claude:s5", t + 300_000, 30);
    let deployed = json!({"task": "Deploy", "status": "doing", "request_status": "shipped"});
    wrote(&mut h, "claude", "s5", "deploy", deployed);
    // A review that shipped the request on a day of its own, with no status to complete it.
    h.said("s6", R, t + 115_000, "Publish the docs.");
    shipped_review(&mut h, "c:claude:s6", t + 320_000, "2026-11-17");
    for (repo, task) in [(R, "Bump the version"), (R2, "Tag the release")] {
        let fields = json!({"task": task, "status": "doing", "request_id": "rq1",
            "request": "Cut the release"});
        noted(&mut h, repo, "release", fields);
    }
    let asked = json!({"prompt": "Rename the lexer."});
    event(&mut h, "prompt", "codex", "c1", t + 120_000, asked);
    let item = |id: &str, kind: &str, rank: u32, title: &str, quote: &str, area: Option<&str>| {
        let mut item = json!({"id": id, "kind": kind, "rank": rank, "title": title,
            "product": "", "asked": 1, "quotes": [{"seq": s1, "ts": t, "text": quote}]});
        if let Some(area) = area {
            item["area"] = json!(area);
        }
        item
    };
    let items = json!([
        item(
            "g1",
            "priority",
            1,
            "Parser",
            "the parser's error messages",
            Some(PARSER)
        ),
        item(
            "g2",
            "ask",
            1,
            "A header test",
            "a header test",
            Some(PARSER)
        ),
        item("g3", "ask", 2, "Write docs", "write docs", Some(DOCS)),
        item("g4", "ask", 3, "Fail fast", "fail fast", None),
    ]);
    goals_op(&mut h, t + 180_000, s1, items, "Worked on the parser.");
    kept(&h, PARSER, "Parser");
    kept(&h, DOCS, "Docs");
    h.run();
    (h, crate::db::now_ms())
}

/// A My Work op of `job` with `fields`, as grouping and the judge append theirs.
fn my_work_op(h: &mut Store, job: &str, fields: Value) {
    let op = MyWorkOp {
        job: job.into(),
        about: R.into(),
        entry: "fake".into(),
        ts: crate::db::now_ms(),
        read: Read::default(),
        removed: vec![],
        fields: fields.as_object().unwrap().clone().into_iter().collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
}

/// A home of a judge's open suggestion (J4, W3b-3): `session`'s conversation may be part of the
/// parser's request, s1's, which a grouping made a group of.
fn suggested(session: &str) -> (Store, i64) {
    let mut h = Store::new();
    let t = crate::db::now_ms() - 10 * 60_000;
    h.said("s1", R, t, "Build the parser.");
    h.said(session, R, t + 60_000, "The parser drops a trailing comma.");
    let parser = json!({"ignored": [], "groups": [{"conversations": [["claude", "s1"]],
        "title": "Parser", "links": []}]});
    let judged = json!({"conversation": ["claude", session], "outcome": "suggested",
        "groups": [{"conversations": [["claude", session]], "title": null, "links": []}],
        "suggestion": {"request": ["claude", "s1"], "score": 0.6}});
    my_work_op(&mut h, "group", parser);
    my_work_op(&mut h, "judge", judged);
    h.run();
    (h, crate::db::now_ms())
}

/// What the route reads (`read`): the two answers as their readers give them, and the story's kept texts.
fn answers(h: &Store, now: i64) -> (Value, Value) {
    let rules = crate::redact::Rules::load(h.home.path()).unwrap();
    let (mut work, goals) = read(h.home.path(), now, &rules).unwrap();
    refer(h.home.path(), &mut work, &Gate::now()).unwrap();
    (work, goals)
}

/// The one item of `list` whose `key` is `value`.
fn one<'a>(list: &'a Value, key: &str, value: &str) -> &'a Value {
    let found: Vec<&Value> = arr(list).iter().filter(|v| v[key] == value).collect();
    assert_eq!(found.len(), 1, "{key} = {value} in {list}");
    found[0]
}

/// An area of open asks and no priority under it has no count of its own (`asked` is null in
/// oboete's answer). Its asks stay under it in the goals list: the spike's adapter dropped them,
/// and with them the area's whole row of asks on the focus screen.
#[test]
fn an_area_nothing_was_asked_for_in_keeps_its_open_asks() {
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let docs = one(&work["areas"], "id", DOCS);
    assert!(docs["asked"].is_null(), "{docs}");
    assert_eq!(arr(&docs["asks"]).len(), 1, "{docs}");
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let ask = one(&snapshot["goals"]["asks"], "title", "Write docs");
    assert_eq!(
        (&ask["focusAreaId"], &ask["focusArea"]),
        (&json!(DOCS), &json!("Docs"))
    );
    let items = &snapshot["goals"]["areas"];
    assert!(
        arr(items).iter().all(|a| a["focusAreaId"] != DOCS),
        "{items}"
    );
    // The area that was asked for keeps its item and its ask, and the ask under no area is the
    // list's own.
    assert_eq!(one(items, "focusAreaId", PARSER)["timesAsked"], json!(1.0));
    let header = one(&snapshot["goals"]["asks"], "title", "A header test");
    assert_eq!(header["focusAreaId"], json!(PARSER));
    let alone = one(&snapshot["goals"]["asks"], "title", "Fail fast");
    assert!(alone["focusAreaId"].is_null(), "{alone}");
}

/// The pages show a goal's times as written, and label the goals with their time's day as
/// written (`to-work-model.ts:259`, `:321-325`): both are this machine's. The time is one whose
/// day in Tokyo is the next one in UTC.
#[test]
fn the_goals_times_are_written_in_the_machines_zone() {
    let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
    let ms = 1_795_033_800_000; // 2026-11-18T20:30:00Z
    assert_eq!(zoned(ms, &tokyo), json!("2026-11-19T05:30:00.000+09:00"));
    assert_eq!(stamp(&json!(ms), &tokyo), json!("2026-11-19 05:30"));
    assert_eq!(zoned(ms, &Utc), json!("2026-11-18T20:30:00.000+00:00"));
    assert_eq!(stamp(&json!(ms), &Utc), json!("2026-11-18 20:30"));
    assert_eq!(iso(ms), json!("2026-11-18T20:30:00.000Z"));
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &tokyo, &Gate::now());
    let list = &snapshot["goals"];
    let generated = list["generatedAt"].as_str().unwrap_or("");
    assert!(generated.ends_with("+09:00"), "{list}");
    let form = regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$").unwrap();
    let mut times = 0;
    for item in arr(&list["areas"]).iter().chain(arr(&list["asks"])) {
        let at = arr(&item["evidence"]).iter().map(|e| &e["at"]);
        for t in std::iter::once(&item["latestAt"]).chain(at) {
            assert!(form.is_match(t.as_str().unwrap_or("")), "{t} in {item}");
            times += 1;
        }
    }
    assert!(times >= 8, "{list}");
}

/// Every text comes from the gated answers: a value the gate masks in the prompt is in none of the
/// snapshot, whose title and ask are the gated answer's, as they were.
#[test]
fn a_masked_value_stays_masked() {
    let value = format!("q9Zx8mL2vB4nR7tY1wK3{}", "pS6dJ0aF5hU2cE8g");
    let mut h = Store::new();
    let t = crate::db::now_ms() - 60_000;
    h.said(
        "s1",
        R,
        t,
        &format!("Set auth = \"{value}\" for the parser."),
    );
    h.run();
    let now = crate::db::now_ms();
    let (work, goals) = answers(&h, now);
    let gated = one(&work["requests"], "id", "c:claude:s1");
    let title = gated["title"].as_str().unwrap_or("");
    assert!(!title.contains(&value) && title.contains("auth"), "{title}");
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    assert!(!snapshot.to_string().contains(&value));
    let key = json!(["conversation", "claude", "c:claude:s1"]).to_string();
    let request = one(&snapshot["humanRequests"], "id", &key);
    assert_eq!(request["title"], gated["title"]);
    assert_eq!(request["original"]["text"], gated["ask"]["text"]);
}

/// A done request's clock stops when the status that completed it was recorded, as claude-mem's
/// does (`to-work-model.ts:194`): oboete's answer carries that time whatever its status reads.
#[test]
fn a_done_request_is_completed_when_its_status_said_so() {
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let done = one(&work["requests"], "id", "c:claude:s3");
    assert_eq!(done["status"], json!("ready"), "{done}");
    let at = done["completed_at"].as_i64().expect("the time it was done");
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let key = json!(["conversation", "claude", "c:claude:s3"]).to_string();
    assert_eq!(
        one(&snapshot["humanRequests"], "id", &key)["completedAtEpoch"],
        json!(at)
    );
}

/// A work list is one per repository: a list of one name in two repositories is two workstreams.
#[test]
fn a_list_of_one_name_in_two_repositories_is_two_workstreams() {
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let streams = &one(&snapshot["humanRequests"], "id", "r:rq1")["workstreams"];
    let tasks: Vec<Vec<&str>> = (arr(streams).iter())
        .map(|w| {
            arr(&w["items"])
                .iter()
                .filter_map(|i| i["title"].as_str())
                .collect()
        })
        .collect();
    assert_eq!(tasks.len(), 2, "{streams}");
    assert!(tasks.contains(&vec!["Bump the version"]), "{streams}");
    assert!(tasks.contains(&vec!["Tag the release"]), "{streams}");
}

/// A request shipped by its review alone has no completing status: the review's day completes
/// it, at noon with no zone, which each browser reads as that day where it is, and a later update
/// moves nothing (Codex on #1087).
#[test]
fn a_request_shipped_by_its_review_is_completed_on_the_reviews_day() {
    let work = json!({"requests": [{"id": "r:x", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "started": 1_795_000_000_000_i64, "completed_at": null,
        "ask": {"text": "Release it.", "ts": 1_795_000_000_000_i64}, "checked": null,
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64,
            "completed": "2026-11-19", "completed_at": null,
            "readings": [{"ts": 1_795_050_000_000_i64, "percent": 100}]},
        "story": {"narrative": "Shipped.", "milestones": [{"date": "2026-11-18", "text": "Built"}]}}]});
    let snapshot = snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc, &Gate::now());
    let request = &snapshot["humanRequests"][0];
    assert!(request["completedAtEpoch"].is_null(), "{request}");
    let progress = &request["progress"];
    assert_eq!(progress["shippedAt"], json!("2026-11-19T12:00:00"));
    assert_eq!(
        progress["milestones"][0]["at"],
        json!("2026-11-18T12:00:00")
    );
    // Shipped by its status, with no review: no reading, and the status's time completes it.
    let work = json!({"requests": [{"id": "r:y", "status": "shipped", "title": "Notes",
        "updated": 1_795_100_000_000_i64, "completed_at": 1_795_090_000_000_i64,
        "ask": {"text": "Notes.", "ts": 1_795_000_000_000_i64},
        "progress": {"percent": 100, "reviewed": null, "completed": "2026-11-19",
            "completed_at": 1_795_090_000_000_i64}}]});
    let request =
        &snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc, &Gate::now())["humanRequests"][0];
    assert!(request["progress"].is_null(), "{request}");
    assert_eq!(request["completedAtEpoch"], json!(1_795_090_000_000_i64));
}

/// The Build story is the recap and then each week's chapter under its week. A story of chapters
/// alone still opens on one: the mapper hides a story with no paragraph before its headings.
#[test]
fn the_build_story_keeps_its_weeks() {
    let chapters = json!([{"week": "2026-11-16", "text": "Built the adapter."},
        {"week": "2026-11-09", "text": "Ran the spike."}]);
    let work = json!({"story": {"recap": "Two weeks of the work-view.", "chapters": chapters}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    let want = "Two weeks of the work-view.\n\n## 2026-11-16\n\nBuilt the adapter.\n\n## 2026-11-09\n\nRan the spike.";
    assert_eq!(story, &json!(want));
    let work = json!({"story": {"recap": null, "chapters": chapters}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    assert!(
        story
            .as_str()
            .is_some_and(|n| n.starts_with("## 2026-11-16\n\nBuilt")),
        "{story}"
    );
    assert!(snapshot_at(&json!({}), &json!({}), 0, &Utc, &Gate::now())["story"].is_null());
}

/// A list-level update has no task: its list names it, as on oboete's own page, and a missing
/// status is an empty one, as the item's type has it.
#[test]
fn a_list_level_update_is_named_by_its_list() {
    let work = json!({"requests": [{"id": "r:z", "status": "working",
        "work": [{"list": "release", "repo": "r", "task": null, "status": null, "updated": 1}]}]});
    let item =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0]["items"][0];
    assert_eq!(
        (&item["title"], &item["status"]),
        (&json!("release"), &json!(""))
    );
}

/// An item's status is read as My Work reads it (`my_work::normalized`): the pages take a task as
/// done by its word alone, so a `Done` task is done there too.
#[test]
fn an_items_status_is_read_as_my_work_reads_it() {
    let work = json!({"requests": [{"id": "r:s", "status": "working", "work": [
        {"list": "l", "repo": "r", "task": "Ship", "status": " Done ", "updated": 1},
        {"list": "l", "repo": "r", "task": "Ask", "status": "Needs_Input", "updated": 1}]}]});
    let items = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0]["items"];
    assert_eq!(
        (&items[0]["status"], &items[1]["status"]),
        (&json!("done"), &json!("needs-you"))
    );
}

/// A review's day the gate masked is no day: a request shipped by that review alone stays
/// readable, completed at its last update, and a milestone of a masked day has none (Codex on
/// #1087).
#[test]
fn a_masked_review_day_leaves_the_request_readable() {
    let work = json!({"requests": [{"id": "r:x", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "completed_at": null,
        "ask": {"text": "Release it.", "ts": 1_795_000_000_000_i64},
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64,
            "completed": "[REDACTED]", "completed_at": null,
            "readings": [{"ts": 1_795_050_000_000_i64, "percent": 100}]},
        "story": {"narrative": "Shipped.", "milestones": [{"date": "[REDACTED]", "text": "Built"}]}}]});
    let request =
        &snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc, &Gate::now())["humanRequests"][0];
    assert_eq!(request["completedAtEpoch"], json!(1_795_100_000_000_i64));
    assert!(request["progress"]["shippedAt"].is_null(), "{request}");
    assert!(
        request["progress"]["milestones"][0]["at"].is_null(),
        "{request}"
    );
}

/// A conversation is titled by its own request, whichever request names it first: an explicit
/// request that names it too does not relabel it in the other's drawer (Codex on #1087).
#[test]
fn a_conversation_is_titled_by_its_own_request() {
    let c = json!({"agent": "claude", "id": "c:claude:c1"});
    let work = json!({"requests": [
        {"id": "r:x", "status": "working", "title": "Explicit", "projects": ["a"], "sessions": [c]},
        {"id": "c:claude:c1", "status": "working", "title": "Its own", "projects": ["b"],
            "sessions": [c]}]});
    let agents = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["agents"];
    assert_eq!(agents.as_array().unwrap().len(), 1, "{agents}");
    let shown = (&agents[0]["sessionTitle"], &agents[0]["project"]);
    assert_eq!(shown, (&json!("Its own"), &json!("b")));
}

/// The review the pages get is its newest reading, with its own percent: a request shipped by its
/// status after a review at 50% is 100% by its status, and the pages check that the percent is
/// the newest reading's (Codex on #1087).
#[test]
fn a_request_shipped_after_a_partial_review_keeps_the_reviews_percent() {
    let work = json!({"requests": [{"id": "r:z", "status": "shipped", "title": "Deploy",
        "updated": 1_795_100_000_000_i64, "completed_at": 1_795_090_000_000_i64,
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64, "completed": "2026-11-18",
            "completed_at": 1_795_090_000_000_i64, "readings": [
                {"ts": 1_795_040_000_000_i64, "percent": 20}, {"ts": 1_795_050_000_000_i64, "percent": 50}]}}]});
    let progress =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0]["progress"];
    assert_eq!(
        (&progress["percent"], &progress["state"]),
        (&json!(50.0), &json!("in-progress"))
    );
    assert_eq!(progress["readings"].as_array().map(Vec::len), Some(2));
}

/// A review that cannot carry its day, as it does not go (a rule took its time's key) or as it is
/// below 100%, leaves the request its review shipped to the day at noon on this machine: a review
/// at 30% goes as a review at 30%, which the pages do not take as a shipment of its own (CodeRabbit
/// and Codex on #1087).
#[test]
fn a_request_its_review_shipped_is_completed_on_the_reviews_day() {
    let noon = Utc
        .with_ymd_and_hms(2026, 11, 19, 12, 0, 0)
        .unwrap()
        .timestamp_millis();
    let work = json!({"requests": [{"id": "r:w", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "completed_at": null,
        "progress": {"percent": 100, "completed": "2026-11-19", "completed_at": null}}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0];
    assert!(request["progress"].is_null(), "{request}");
    assert_eq!(request["completedAtEpoch"], json!(noon));
    let work = json!({"requests": [{"id": "r:v", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "completed_at": null,
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64, "completed": "2026-11-19",
            "completed_at": null, "readings": [{"ts": 1_795_050_000_000_i64, "percent": 30}]}}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0];
    assert_eq!(
        request["progress"]["state"],
        json!("in-progress"),
        "{request}"
    );
    // A review that did not ship the request has no shipped day (CodeRabbit on #1087).
    assert!(request["progress"]["shippedAt"].is_null(), "{request}");
    assert_eq!(request["completedAtEpoch"], json!(noon));
}

/// A review goes with its own percent, its newest reading's, or not at all: a request a status
/// shipped is at 100% whatever its review found (CodeRabbit on #1087).
#[test]
fn a_review_with_no_reading_of_its_own_does_not_go() {
    let work = json!({"requests": [{"id": "r:n", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "completed_at": 1_795_090_000_000_i64,
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64, "completed": "2026-11-19",
            "completed_at": 1_795_090_000_000_i64, "readings": []}}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0];
    assert!(request["progress"].is_null(), "{request}");
    assert_eq!(request["completedAtEpoch"], json!(1_795_090_000_000_i64));
}

/// The current goals' period is the window the goals page read, the 31 days that end now, in this
/// machine's days: the run's time stays its own (Codex on #1087).
#[test]
fn the_current_goals_period_is_the_window_that_ends_now() {
    let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
    let page = json!({"current": {"written": 1_794_000_000_000_i64, "items": []},
        "window": {"days": 31, "prompts": 4}});
    // 2026-11-19 05:30 in Tokyo.
    let goals = &snapshot_at(&json!({}), &page, 1_795_033_800_000, &tokyo, &Gate::now())["goals"];
    let period = (&goals["periodStart"], &goals["periodEnd"]);
    assert_eq!(period, (&json!("2026-10-19"), &json!("2026-11-20")));
    assert_eq!(goals["generatedAt"], zoned(1_794_000_000_000, &tokyo));
}

/// A recap that opens on a heading keeps its body as the recap: the heading is a block of its own,
/// as the pages take the first block that is not a heading (CodeRabbit on #1087).
#[test]
fn a_recap_that_opens_on_a_heading_stays_the_recap() {
    let chapters = json!([{"week": "2026-11-16", "text": "Built the adapter."}]);
    let work =
        json!({"story": {"recap": "# The work-view\nTwo weeks of it.", "chapters": chapters}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    let blocks: Vec<&str> = (story.as_str().unwrap().split("\n\n"))
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    let first = blocks.iter().find(|b| !b.starts_with('#'));
    assert_eq!(first, Some(&"Two weeks of it."), "{story}");
}

/// A story's prose stays prose and its headings headings: a recap that opens with an issue's
/// number opens with a word joiner, as the pages take every block that opens with `#` for a
/// heading, and a heading directly above its body is a block of its own (Codex on #1087).
#[test]
fn a_story_of_issue_numbers_and_headings_reads_as_written() {
    let chapters = json!([{"week": "2026-11-16", "text": "### Parser\nThe parser reads lines."}]);
    let work = json!({"story": {"recap": "#1087 shipped the adapter.", "chapters": chapters}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    let want = "\u{2060}#1087 shipped the adapter.\n\n## 2026-11-16\n\n### Parser\n\nThe parser reads lines.";
    assert_eq!(story, &json!(want));
}

/// A request's start, its ask and its last update go each from its own field: the contract's
/// comparison writes every time as its kind, so it would not see one put in another's place
/// (Codex on #1087).
#[test]
fn a_requests_times_go_each_from_its_own_field() {
    let work = json!({"requests": [{"id": "r:t", "status": "working", "started": 1_000,
        "updated": 2_000, "ask": {"text": "Ask.", "ts": 1_500}}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0];
    let times = (
        &request["startedAtEpoch"],
        &request["updatedAtEpoch"],
        &request["original"]["atEpoch"],
    );
    assert_eq!(times, (&json!(1_000), &json!(2_000), &json!(1_500)));
}

/// The pages take a block for a heading when it opens with `#` once its bold and code marks are
/// off, so a paragraph that opens with a marked issue number opens with a word joiner too, in a
/// story of that paragraph alone; one the pages show as nothing goes (Codex on #1087).
#[test]
fn a_story_of_marked_issue_numbers_reads_as_written() {
    for (recap, want) in [
        (
            "**#1087** shipped the adapter.",
            "\u{2060}**#1087** shipped the adapter.",
        ),
        ("`#1088` fixed the gate.", "\u{2060}`#1088` fixed the gate."),
        (
            "** #7** has a space in its bold.",
            "\u{2060}** #7** has a space in its bold.",
        ),
        (
            "\u{FEFF}#7 follows a byte-order mark.",
            "\u{2060}#7 follows a byte-order mark.",
        ),
        (
            "**Bold** prose stays as it is.",
            "**Bold** prose stays as it is.",
        ),
    ] {
        let work = json!({"story": {"recap": recap, "chapters": []}});
        let story =
            &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
        assert_eq!(story, &json!(want), "{recap:?}");
    }
    let work = json!({"story": {"recap": "** **", "chapters": []}});
    assert!(snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"].is_null());
}

/// A recap of headings alone gives their text as its paragraphs: the pages take the story's first
/// paragraph for its recap, and would put a chapter's in its place, or show no story (Codex on
/// #1087).
#[test]
fn a_recap_of_headings_alone_gives_their_text() {
    let chapters = json!([{"week": "2026-11-16", "text": "Built the parser."}]);
    for (chapters, want) in [
        (json!([]), "Shipped the adapter"),
        (
            chapters,
            "Shipped the adapter\n\n## 2026-11-16\n\nBuilt the parser.",
        ),
    ] {
        let work = json!({"story": {"recap": "## Shipped the adapter", "chapters": chapters}});
        let story =
            &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
        assert_eq!(story, &json!(want));
    }
    let work = json!({"story": {"recap": "## #1087\n### ", "chapters": []}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    assert_eq!(story, &json!("\u{2060}#1087"));
}

/// The pages split and trim by JavaScript's `\s`, which U+0085 is not of: a line of it alone does
/// not end a paragraph (CodeRabbit on #1087).
#[test]
fn a_line_of_a_next_line_mark_alone_ends_no_paragraph() {
    let work = json!({"story": {"recap": "A\n\u{85}\nB", "chapters": []}});
    let story =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
    assert_eq!(story, &json!("A\n\u{85}\nB"));
}

/// A story with no paragraph at all, which the pages would not show, gives the text of its first
/// heading of its own, not a week's, as its recap; one with nothing of its own has none (Codex on
/// #1087).
#[test]
fn a_story_of_headings_alone_gives_its_first_heading_of_its_own() {
    for (text, want) in [
        (
            "### Parser\n### Lexer",
            "## 2026-11-16\n\nParser\n\n### Lexer",
        ),
        ("", "## 2026-11-16"),
    ] {
        let chapters = json!([{"week": "2026-11-16", "text": text}]);
        let work = json!({"story": {"recap": "", "chapters": chapters}});
        let story =
            &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["story"]["story"]["narrative"];
        assert_eq!(story, &json!(want), "{text:?}");
    }
}

/// A case of a stored Build story: its name, its recap, and its chapters as (week, text).
type Story = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The cases whose stories the Node contract test reads through the pinned mapper
/// (`stories.json`).
const STORIES: &[Story] = &[
    ("a recap of a heading alone", "## Shipped the adapter", &[]),
    (
        "a recap of a heading alone, and a chapter",
        "## Shipped the adapter",
        &[("2026-11-16", "Built the parser.")],
    ),
    (
        "a recap that opens on a heading",
        "## Plan\nShip the adapter.",
        &[],
    ),
    (
        "a recap that opens with an issue number, and a chapter whose heading sits above its body",
        "#1087 shipped the adapter.",
        &[("2026-11-16", "### Parser\nThe parser reads lines.")],
    ),
    (
        "a recap that opens with an issue number in bold, alone",
        "**#1087** shipped the adapter.",
        &[],
    ),
    (
        "a recap that opens with an issue number as code, alone",
        "`#1088` fixed the gate.",
        &[],
    ),
    (
        "a recap after a byte-order mark, alone",
        "\u{FEFF}#7 follows a byte-order mark.",
        &[],
    ),
    ("chapters alone", "", &[("2026-11-16", "Built the parser.")]),
    (
        "chapters of headings alone",
        "",
        &[("2026-11-16", "### Parser")],
    ),
    ("chapters of no text", "", &[("2026-11-16", "")]),
];

/// The contract's Build stories: `ui/work-view/contract/stories.json` holds the story this adapter
/// writes for each case of `STORIES`, which the Node test reads through the pinned mapper. With
/// OBOETE_CONTRACT_WRITE=1 it writes them.
#[test]
fn the_contract_stories_are_the_adapters_answer() {
    let fresh: Vec<Value> = (STORIES.iter())
        .map(|(name, recap, chapters)| {
            let chapters: Vec<Value> = (chapters.iter())
                .map(|(week, text)| json!({"week": week, "text": text}))
                .collect();
            let story = build_story(&json!({"recap": recap, "chapters": chapters}), &Gate::now());
            json!({"name": name, "story": story})
        })
        .collect();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/work-view/contract/stories.json");
    if std::env::var_os("OBOETE_CONTRACT_WRITE").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&fresh).unwrap() + "\n").unwrap();
        return;
    }
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        kept,
        json!(fresh),
        "the contract stories are not this adapter's"
    );
}

/// A workstream goes by its place among its request's, the only use the pages make of its id
/// (`to-work-model.ts:215`): no name of its repository or list is in it, joined or not, and two
/// lists the gate masks alike stay two (Codex on #1087).
#[test]
fn a_workstream_goes_by_its_place() {
    let item = |list: &str, stream: Option<&str>, task: &str| {
        let mut w = json!({"repo": R, "list": list, "task": task, "status": "doing"});
        if let Some(print) = stream {
            w["stream"] = json!(print);
        }
        w
    };
    let work = json!({"requests": [{"id": "r:w", "status": "working", "title": "Release",
        "work": [item("notes", None, "Write"), item("[REDACTED]", Some("aaaaaaaaaaaa"), "A"),
            item("[REDACTED]", Some("bbbbbbbbbbbb"), "B"), item("notes", None, "Read")]}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0];
    let streams: Vec<(&str, usize)> = (request["workstreams"].as_array().unwrap().iter())
        .map(|w| {
            (
                w["id"].as_str().unwrap(),
                w["items"].as_array().unwrap().len(),
            )
        })
        .collect();
    assert_eq!(streams, [("w0", 2), ("w1", 1), ("w2", 1)]);
    let titled: Vec<&str> = (request["humanCopy"]["workstreams"]
        .as_array()
        .unwrap()
        .iter())
    .map(|w| w["id"].as_str().unwrap())
    .collect();
    assert_eq!(titled, ["w0", "w1", "w2"]);
}

/// A review's readings go each at its own time, in their order, the newest its time of review:
/// the contract's comparison writes every time as its kind, so it would not see one put in
/// another's place (Codex on #1087).
#[test]
fn a_reviews_readings_go_each_at_its_own_time() {
    let readings = json!([{"ts": 1_795_040_000_000_i64, "percent": 20},
        {"ts": 1_795_050_000_000_i64, "percent": 50}]);
    let work = json!({"requests": [{"id": "r:p", "status": "working", "title": "Migrate",
        "updated": 1_795_060_000_000_i64,
        "progress": {"percent": 50, "reviewed": 1_795_050_000_000_i64, "readings": readings}}]});
    let progress =
        &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0]["progress"];
    let want = json!([{"at": "2026-11-18T22:13:20.000Z", "percent": 20.0},
        {"at": "2026-11-19T01:00:00.000Z", "percent": 50.0}]);
    assert_eq!(progress["readings"], want, "{progress}");
    assert_eq!(progress["checkedAt"], json!("2026-11-19T01:00:00.000Z"));
}

fn hex(t: &str, n: usize) -> bool {
    t.len() == n && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What changes from home to home and run to run, written as its kind: the times, days and
/// months, the keyed fingerprints, the masks' prints and the references' (`refer`). What is left
/// is what the answer says.
fn steady(v: &Value) -> Value {
    steady_with(v, &mut Vec::new())
}

/// `steady`, each group's print (`g:` and 12 hex digits, keyed by the home's key), each
/// subagent's (`s:`, W4), and each rule's and scope's (`l:`, `u:`, `r:`, #1083) written as its
/// place among those seen first, so that the id a suggestion names stays its row's (W3b-3).
fn steady_with(v: &Value, groups: &mut Vec<String>) -> Value {
    match v {
        Value::Number(n) if n.as_i64().is_some_and(|n| n > 1_000_000_000_000) => json!("<ms>"),
        Value::String(s) => {
            let iso = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T[0-9:.]+(Z|[+-]\d{2}:\d{2})?").unwrap();
            let stamp = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}( \d{2}:\d{2})?$").unwrap();
            let month = regex::Regex::new(r"\b\d{4}-\d{2}\b").unwrap();
            let print = regex::Regex::new(r"#[0-9a-f]{12}").unwrap();
            let reference = regex::Regex::new(r"\bm:[0-9a-f]{12}\b").unwrap();
            let group = regex::Regex::new(r"\b(?:[gs]:[0-9a-f]{12}|[lur]:[0-9a-f]{64})\b").unwrap();
            let s = iso.replace_all(s, "<iso>");
            let s = stamp.replace_all(&s, "<day>");
            let s = month.replace_all(&s, "<month>");
            let s = print.replace_all(&s, "#<print>");
            let s = reference.replace_all(&s, "m:<print>");
            json!(group.replace_all(&s, |c: &regex::Captures| {
                let seen = groups.iter().position(|g| *g == c[0]);
                let at = seen.unwrap_or_else(|| {
                    groups.push(c[0].to_owned());
                    groups.len() - 1
                });
                format!("{}<{at}>", &c[0][..2])
            }))
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| steady_with(v, groups)).collect()),
        Value::Object(o) => Value::Object(
            (o.iter())
                .map(|(k, v)| match (k.as_str(), v.as_str()) {
                    // A version's keyed hash and the snapshot's time, each in its own shape: an
                    // empty or malformed one stays as it is and differs.
                    ("contentFingerprint" | "expectedSuggestionHash" | "ruleVersion", Some(t))
                        if hex(t, 64) =>
                    {
                        (k.clone(), json!("<kept>"))
                    }
                    ("revision", Some(t))
                        if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) =>
                    {
                        (k.clone(), json!("<kept>"))
                    }
                    _ => (k.clone(), steady_with(v, groups)),
                })
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// shortcut: in the first 15 minutes of a month (this machine's), the home's newest goals fall in
/// the month before and make a second month of Past focus, so a contract that holds goals is not
/// compared, nor written, until a later run; upgrade if the home's clock can be set. The home's own
/// clock decides it, the one the goals' read takes, so that a run across a month's start is not
/// compared.
fn first_minutes(now: i64, write: bool) -> bool {
    let local = Local.timestamp_millis_opt(now).unwrap();
    let first = local.day() == 1 && local.hour() == 0 && local.minute() < 15;
    assert!(
        !(first && write),
        "no contract snapshot is written in a month's first 15 minutes: wait for them to pass"
    );
    first
}

/// The contract: `ui/work-view/contract/snapshot.json` is this home's answer, which the Node
/// test there runs claude-mem's mapper over (`node --test 'ui/work-view/contract/*.test.mjs'`).
/// After a change to the answer, run this test with OBOETE_CONTRACT_WRITE=1, which writes this
/// run's answer whatever it is (even one that differs only in what `steady` hides), run the Node
/// test, and commit both.
#[test]
fn the_contract_snapshot_is_this_homes_answer() {
    let (h, now) = home();
    let write = std::env::var_os("OBOETE_CONTRACT_WRITE").is_some();
    if first_minutes(now, write) {
        return;
    }
    let (work, goals) = answers(&h, now);
    let fresh = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/work-view/contract/snapshot.json");
    if write {
        let text = serde_json::to_string_pretty(&fresh).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        steady(&kept),
        steady(&fresh),
        "the contract snapshot is not this home's answer"
    );
}

/// The narrative as claude-mem's pages read it (`to-work-model.ts:434-442`), independent of how
/// the adapter makes it: whole, and each block as it is split at blank lines, its bold and code
/// marks off, its whitespace collapsed and trimmed, and with its leading `#` marks off.
fn pages_views(narrative: &str) -> Vec<String> {
    let js = |c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}';
    // JavaScript's whitespace, which U+0085 is not, as `js`.
    let blank = Regex::new(r"\n[[\s\u{FEFF}]&&[^\u{85}]]*\n").unwrap();
    let bold = Regex::new(r"\*\*([^*]+)\*\*").unwrap();
    let code = Regex::new(r"`([^`]+)`").unwrap();
    let mut views = vec![narrative.to_owned()];
    for block in blank.split(narrative) {
        let block = bold.replace_all(block, "$1");
        let block = code.replace_all(&block, "$1");
        let block: Vec<&str> = block.split(js).filter(|w| !w.is_empty()).collect();
        let block = block.join(" ");
        if block.is_empty() {
            continue;
        }
        let shown = block
            .trim_start_matches('#')
            .trim_start_matches(js)
            .to_owned();
        views.extend([block, shown]);
    }
    views
}

/// The Build story is read whole before any mask (#1089), and keeps its blocks: a rule that needs,
/// in the story the recap and a chapter make, a heading another rule masks first; a rule that runs
/// past where the adapter puts a word joiner and a week's `## `; a heading followed by prose, read
/// as the pages show it, its marks off; a rule that finds a value beside what a mask left, as the
/// gate reads again what it masked; and a rule on a kept text as it was kept, a heading and a line
/// of prose under it, or a recap of a heading alone, which the story writes in another shape; and
/// what the masks leave read again as the pages show each block, a heading masked in part or a
/// paragraph a rule masked in part as it read the next; and a paragraph read without the word
/// joiner the adapter puts before its `#`.
/// `ui/work-view/contract/gated-stories.json` is what it writes of each, which the Node test reads
/// through the mapper; written again as the other contract files are (OBOETE_CONTRACT_WRITE=1).
#[test]
fn a_story_is_read_whole_before_any_mask() {
    if !crate::view::tests::in_child("work_view::tests::a_story_is_read_whole_before_any_mask") {
        return;
    }
    let gate = tempfile::tempdir().unwrap();
    let rules = "[redaction]\nextra_rules = [{ id = \"whole\", regex = '^Project Phoenix$' }, \
                 { id = \"after\", regex = 'Phoenix\\s+[\\d-]+\\s+\\w{14}' }, \
                 { id = \"across\", regex = 'zebracode[\\s#0-9-]+\\w{12,}' }, \
                 { id = \"glued\", regex = '\\[REDACTED\\]\\s+\\d{6}' }, \
                 { id = \"kept\", regex = '^## Release\\n(Orchid)$' }, \
                 { id = \"alone\", regex = '^## Release$' }, \
                 { id = \"token\", regex = '^## (Token) Orchid$' }, \
                 { id = \"shown\", regex = '^\\[REDACTED\\] (Orchid)$' }, \
                 { id = \"context\", regex = '(Alpha) 42 Beta\\s+Gamma' }, \
                 { id = \"paragraph\", regex = '^\\[REDACTED\\] (\\d+) Beta$' }, \
                 { id = \"issue\", regex = '^#1087 (zebravalue)$' }, \
                 { id = \"nel\", regex = '^(?u:\\x{0085})(Orchid)' }, \
                 { id = \"trimmed\", regex = '^(Zinnia)$' }]\n";
    std::fs::write(gate.path().join("config.toml"), rules).unwrap();
    crate::redact::set_home(gate.path()).unwrap();
    let mut written = Vec::new();
    // Each case's kept texts, the value the story must not show, and whether the kept texts, each
    // read alone, hold nothing the rules find.
    for (name, recap, text, hidden, alone) in [
        (
            "a heading masked whole, which another rule needs in the story",
            "## Project Phoenix",
            "qwertyuiopasdf is in the drawer.",
            "qwertyuiopasdf",
            true,
        ),
        (
            "an issue's number after a recap that ends on a rule's keyword",
            "Ask for the zebracode",
            "#1087 asdfghjklzxcvb went out.",
            "asdfghjklzxcvb",
            true,
        ),
        (
            "a heading followed by prose, as the pages show it",
            "## Project Phoenix\nThe rest is prose.",
            "Nothing here.",
            "Project Phoenix",
            true,
        ),
        (
            "a value a rule finds beside what a mask left",
            "Project Phoenix\n\n123456 is the code.",
            "Nothing here.",
            "123456",
            true,
        ),
        (
            "a heading and a line of prose under it, as they were kept",
            "## Release\nOrchid",
            "Nothing here.",
            "Orchid",
            false,
        ),
        (
            "a recap of a heading alone, as it was kept",
            "## Release",
            "Nothing here.",
            "Release",
            false,
        ),
        (
            "a heading masked in part, read again as the pages show it",
            "## Token Orchid",
            "Nothing here.",
            "Orchid",
            false,
        ),
        (
            "a paragraph masked in part by a rule that needs the next, read again alone",
            "Alpha 42 Beta\n\nGamma",
            "Nothing here.",
            "42",
            false,
        ),
        (
            "a paragraph that opens with an issue's number, read without its word joiner",
            "#1087 zebravalue\n\nOther.",
            "Nothing here.",
            "zebravalue",
            true,
        ),
        (
            "a block that would open with a character the gate's reading drops",
            "## \u{0085}Orchid",
            "Nothing here.",
            "\u{0085}",
            true,
        ),
        (
            "a heading over prose whose text opens with a character the gate's reading drops",
            "## \u{0085}Orchid\nOrdinary prose.",
            "Nothing here.",
            "Orchid",
            true,
        ),
        (
            "a heading over prose whose text a rule finds once the gate's reading trims it",
            "## \u{0085}Zinnia\nOrdinary prose.",
            "Nothing here.",
            "Zinnia",
            true,
        ),
    ] {
        assert_eq!(
            [recap, text]
                .iter()
                .all(|k| crate::redact::outbound(k) == *k),
            alone,
            "{name}"
        );
        let story = json!({"recap": recap, "chapters": [{"week": "2026-11-16", "text": text}]});
        let narrative = build_story(&story, &Gate::now())["story"]["narrative"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!narrative.contains(hidden), "{name}: {narrative}");
        // Read as the pages read it, nothing of it is what a rule finds, as the gate reads a
        // stored text or with its edges as they are.
        let rules = crate::redact::rules_now().unwrap();
        for view in pages_views(&narrative) {
            let stored = crate::redact::hidden(&view, &rules);
            assert_eq!(remasked(&view, stored), view, "{name}: {narrative}");
            let exact = crate::redact::hidden_exact(&view, &rules);
            assert_eq!(remasked(&view, exact), view, "{name}: {narrative}");
        }
        assert!(narrative.contains("\n\n## "), "{name}: {narrative}");
        let blocks = if recap.contains('\n') { 4 } else { 3 };
        assert_eq!(
            narrative.split("\n\n").count(),
            blocks,
            "{name}: {narrative}"
        );
        let story = json!({"story": {"narrative": narrative, "writtenAt": null}});
        written.push(json!({"name": name, "hidden": hidden, "story": story}));
    }
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/work-view/contract/gated-stories.json");
    let fresh = Value::Array(written);
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    if kept != fresh && std::env::var_os("OBOETE_CONTRACT_WRITE").is_some() {
        let text = serde_json::to_string_pretty(&fresh).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    assert_eq!(
        kept, fresh,
        "the gated stories are not the adapter's answer"
    );
}

/// A work item's status goes as one of the words the pages read, or as none (#1089): a status of
/// another spelling is text, which a rule may find as it would be sent.
#[test]
fn an_items_status_goes_as_a_word_the_pages_read_or_as_none() {
    let work = json!({"requests": [{"id": "r:s", "status": "working", "work": [
        {"list": "l", "repo": "r", "task": "a", "status": "release_4827", "updated": 1},
        {"list": "l", "repo": "r", "task": "b", "status": "In Progress", "updated": 1},
        {"list": "l", "repo": "r", "task": "c", "status": "wip", "updated": 1}]}]});
    let items = &snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now())["humanRequests"][0]["items"];
    let statuses: Vec<&Value> = arr(items).iter().map(|i| &i["status"]).collect();
    assert_eq!(statuses, [&json!(""), &json!("in-progress"), &json!("")]);
}

/// The gate home's rules (#1089): one on each key the snapshot reads its counts, days and times
/// by, one on a work item's status as the pages would read it, and one that finds an Antigravity
/// conversation's id only beside `gemini`, which the adapter writes in its key.
const GATED: &str = "[redaction]\nextra_rules = [{ id = \"prompts\", regex = '^prompts$' }, \
    { id = \"days\", regex = '^days$' }, { id = \"reviewed\", regex = '^reviewed$' }, \
    { id = \"completed\", regex = '^completed_at$' }, { id = \"spelling\", regex = 'release-4827' }, \
    { id = \"gemini\", regex = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', \
    keywords = [\"gemini\"] }, { id = \"alpha\", regex = '^alpha' }, \
    { id = \"beside\", regex = '^r:\\[REDACTED\\]-(Orchid)$' }, \
    { id = \"printed\", regex = '(Orchid)#[0-9a-f]{12}$' }, \
    { id = \"raw\", regex = '^(tenant-orchid):' }]\n";

/// The Antigravity conversation's id in the gate home.
const AGY: &str = "6f1c2a3b-4d5e-4f60-8a1b-2c3d4e5f6a7b";

/// `home()` read under `GATED`, with an Antigravity conversation and a work item whose status
/// the rules find as the pages would read it.
fn gate_home() -> (Store, i64) {
    let (mut h, _) = home();
    let t = crate::db::now_ms() - 5 * 60_000;
    event(
        &mut h,
        "prompt",
        "agy",
        AGY,
        t,
        json!({"prompt": "Plan the parser."}),
    );
    let spelled = json!({"task": "Cut it", "status": "release_4827"});
    wrote(&mut h, "agy", AGY, "plan", spelled);
    // A request its own id names, a part of which a rule masks as My Work shows the id, and one
    // whose own id is, letter for letter, what the first one's went as when the main page's step
    // read it again: the two stay two.
    let first = json!({"task": "One", "status": "doing", "request_id": "alpha-Orchid",
        "request": "Plan the first one"});
    noted(&mut h, R, "first", first);
    let print = crate::my_work::keyed_print("r:alpha-Orchid", h.home.path(), &mut None).unwrap();
    let literal = format!("{0}-{0}#{print}", crate::redact::MASK);
    let second = json!({"task": "Two", "status": "doing", "request_id": literal,
        "request": "Plan the second one"});
    noted(&mut h, R, "second", second);
    // A conversation of an agent the raw hook named as it was given.
    event(
        &mut h,
        "prompt",
        "tenant-orchid",
        "raw-1",
        t + 1_000,
        json!({"prompt": "Plan the raw one."}),
    );
    h.run();
    std::fs::write(h.home.path().join("config.toml"), GATED).unwrap();
    crate::redact::set_home(h.home.path()).unwrap();
    (h, crate::db::now_ms())
}

/// `text` with each run of `hidden` (`redact::hidden`'s) one mask: `text` itself when the gate
/// finds nothing in it but its own masks.
fn remasked(text: &str, hidden: Option<Vec<(usize, usize)>>) -> String {
    let Some(runs) = hidden else {
        return crate::redact::MASK.to_owned();
    };
    let (mut out, mut pos) = (String::new(), 0);
    for (s, e) in runs {
        out.push_str(&text[pos..s]);
        out.push_str(crate::redact::MASK);
        pos = e;
    }
    out + &text[pos..]
}

/// A rule saved while the stores are read is in force in the answer, in the shapes My Work gates
/// at its source too: here a conversation's session, which no shape of its id the walk reads
/// finds. The stores are read again under it (Codex on #1093), whether it is saved in the one file
/// the viewer's home and its gate share, in the home's alone, whose rules My Work reads under, or
/// in the gate's alone. And My Work and the goals read under the rules they are given, whatever
/// the file says meanwhile.
#[test]
fn a_rule_saved_while_the_stores_are_read_is_in_force() {
    let name = "work_view::tests::a_rule_saved_while_the_stores_are_read_is_in_force";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (mut h, _) = home();
    let t = crate::db::now_ms() - 5 * 60_000;
    let prompt = json!({"prompt": "Plan the secret one."});
    event(&mut h, "prompt", "codex", "secret-session", t, prompt);
    h.run();
    let home = h.home.path();
    let none = "[redaction]\n";
    let anchored = "[redaction]\nextra_rules = [{ id = \"session\", regex = '^secret-session$' }, \
                    { id = \"goal\", regex = '^Write docs$' }]\n";
    let now = crate::db::now_ms();
    std::fs::write(home.join("config.toml"), none).unwrap();
    crate::redact::set_home(home).unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("config.toml"), anchored).unwrap();
    let ruled = crate::redact::Rules::load(other.path()).unwrap();
    let (work, goals) = read(home, now, &ruled).unwrap();
    let sessions: Vec<&Value> = (arr(&work["requests"]).iter())
        .flat_map(|r| arr(&r["sessions"]))
        .collect();
    let codex = (sessions.iter())
        .find(|c| c["agent"] == "codex")
        .expect("the codex conversation");
    assert!(codex["fingerprint"].is_string(), "{codex}");
    assert!(!goals.to_string().contains("Write docs"), "{goals}");
    // The gate reads an id whole: its rule finds the session inside it.
    let unanchored =
        "[redaction]\nextra_rules = [{ id = \"session\", regex = 'secret-session' }]\n";
    let gate = tempfile::tempdir().unwrap();
    for (case, gate_home, saved_in, rule) in [
        ("one file", home, home, anchored),
        ("the home's", gate.path(), home, anchored),
        ("the gate's", gate.path(), gate.path(), unanchored),
    ] {
        std::fs::write(home.join("config.toml"), none).unwrap();
        std::fs::write(gate.path().join("config.toml"), none).unwrap();
        crate::redact::set_home(gate_home).unwrap();
        // The rule saved once the first rules were read, as the stores are read under them.
        let mut reads = 0;
        let snapshot = answered(home, now, || {
            reads += 1;
            if reads == 2 {
                std::fs::write(saved_in.join("config.toml"), rule).unwrap();
            }
            Gate::now()
        })
        .unwrap();
        assert_eq!(reads, 3, "{case}");
        let text = snapshot.to_string();
        assert!(!text.contains("secret-session"), "{case}: {text}");
        assert!(text.contains("Plan the secret one."), "{case}: {text}");
    }
}

/// A line holding a U+0085 alone parts no blocks: the pages' blank lines are of JavaScript's
/// whitespace, which U+0085 is not, as the story's reading (`page_blocks`) and this file's model
/// of the pages (`pages_views`) read them (Codex on #1093).
#[test]
fn a_line_of_a_u0085_alone_parts_no_blocks() {
    let text = "A\n\u{0085}\nB";
    assert_eq!(page_blocks(text), vec![0..text.len()]);
    assert_eq!(pages_views(text).len(), 3, "{:?}", pages_views(text));
}

/// Each pair of strings at one place of `a` and `b` under a key the walk leaves as it is (`OWN`).
fn paired(a: &Value, b: &Value, key: &str, out: &mut Vec<(String, String)>) {
    match (a, b) {
        (Value::String(x), Value::String(y)) if OWN.contains(&key) => {
            out.push((x.clone(), y.clone()));
        }
        (Value::Array(xs), Value::Array(ys)) => {
            assert_eq!(xs.len(), ys.len(), "{key}");
            for (x, y) in xs.iter().zip(ys) {
                paired(x, y, key, out);
            }
        }
        (Value::Object(xs), Value::Object(ys)) => {
            for (k, x) in xs {
                paired(x, ys.get(k).unwrap_or(&Value::Null), k, out);
            }
        }
        _ => {}
    }
}

/// Every stored id the snapshot writes in a selector, whole or within another, goes in none
/// under a rule that finds it whole: a request's, a conversation's, a focus area's (a request's
/// area too) and an open ask's. Each goes by one reference wherever it was written, and two by
/// two (Codex on #1093). A request in Other stays there, and a finished month keeps its name,
/// under rules that find them, as the code's own words and times.
#[test]
fn a_rule_on_a_stored_id_finds_it_in_no_selector() {
    let name = "work_view::tests::a_rule_on_a_stored_id_finds_it_in_no_selector";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, now) = gate_home();
    let home = h.home.path();
    std::fs::write(home.join("config.toml"), "[redaction]\n").unwrap();
    let plain = crate::redact::Rules::load(home).unwrap();
    // A request the owner placed in the parser's area.
    let (work, _) = read(home, now, &plain).unwrap();
    let placed = (arr(&work["requests"]).iter())
        .filter_map(|r| r["id"].as_str())
        .find(|id| id.starts_with("c:claude:"))
        .expect("a conversation's request")
        .to_owned();
    let body = json!({"action": "move-request", "request": placed, "area": PARSER});
    let lock = std::sync::Mutex::new(());
    crate::my_work_focus::act(home, &lock, &serde_json::to_vec(&body).unwrap()).unwrap();
    // And one the owner placed in Other.
    let other = (arr(&work["requests"]).iter())
        .filter_map(|r| r["id"].as_str())
        .find(|id| id.starts_with("c:") && *id != placed)
        .expect("a second conversation's request")
        .to_owned();
    let body = json!({"action": "move-request", "request": other, "area": "other"});
    crate::my_work_focus::act(home, &lock, &serde_json::to_vec(&body).unwrap()).unwrap();
    // The stored ids as My Work gives them under no rule of the owner's.
    let (work, _) = read(home, now, &plain).unwrap();
    let mut held = Vec::new();
    for r in arr(&work["requests"]) {
        held.extend([&r["id"], &r["focus"]]);
        held.extend(arr(&r["sessions"]).iter().map(|c| &c["id"]));
    }
    for a in arr(&work["areas"]) {
        held.push(&a["id"]);
        held.extend(arr(&a["asks"]).iter().map(|k| &k["id"]));
    }
    held.extend(arr(&work["other_asks"]).iter().map(|k| &k["id"]));
    let mut ids: Vec<String> = (held.into_iter())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    ids.retain(|id| id != crate::my_work_focus::OTHER);
    for id in ["g2", "g4", PARSER, placed.as_str()] {
        assert!(ids.iter().any(|i| i == id), "{id}: {ids:?}");
    }
    let before = answered(home, now, Gate::now).unwrap();
    let rules: Vec<String> = (ids.iter().enumerate())
        .map(|(n, id)| format!("{{ id = \"id{n}\", regex = '^{}$' }}", regex::escape(id)))
        .collect();
    let words = [
        "{ id = \"other\", regex = '^a:other$' }",
        "{ id = \"month\", regex = '^\\d{4}-\\d{2}$' }",
    ];
    let rules = [rules, words.map(str::to_owned).to_vec()].concat();
    let config = format!("[redaction]\nextra_rules = [{}]\n", rules.join(", "));
    std::fs::write(home.join("config.toml"), config).unwrap();
    let after = answered(home, now, Gate::now).unwrap();
    let in_other = (arr(&after["humanRequests"]).iter())
        .filter(|r| r["focusAreaId"] == crate::my_work_focus::OTHER)
        .count();
    assert_eq!(in_other, 1, "{after}");
    let months: Vec<&Value> = (arr(&after["goalHistory"]).iter())
        .map(|g| &g["listName"])
        .collect();
    assert!(
        !months.is_empty()
            && months
                .iter()
                .all(|m| m.as_str().is_some_and(|m| m.len() == 7)),
        "{months:?}"
    );
    for (key, value) in selectors(&after, "") {
        for id in &ids {
            assert!(!value.contains(id.as_str()), "{key}: {value} holds {id}");
        }
    }
    // Place by place, one selector before goes as one after, and two as two.
    let mut pairs = Vec::new();
    paired(&before, &after, "", &mut pairs);
    let (mut forth, mut back) = (HashMap::new(), HashMap::new());
    for (b, a) in &pairs {
        assert_eq!(forth.entry(b).or_insert(a), &a, "{b}");
        assert_eq!(back.entry(a).or_insert(b), &b, "{a}");
    }
}

/// Every string of `v`.
fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(xs) => xs.iter().flat_map(strings).collect(),
        Value::Object(m) => m.values().flat_map(strings).collect(),
        _ => Vec::new(),
    }
}

/// Each string of `v` under a key the pages match on, which the walk leaves as it is (`OWN`).
fn selectors(v: &Value, key: &str) -> Vec<(String, String)> {
    const SELECTORS: [&str; 7] = [
        "id",
        "agentIds",
        "conversationKeys",
        "contentSessionId",
        "focusAreaId",
        "listId",
        "platformSource",
    ];
    match v {
        Value::String(s) if SELECTORS.contains(&key) => vec![(key.to_owned(), s.clone())],
        Value::Array(xs) => xs.iter().flat_map(|x| selectors(x, key)).collect(),
        Value::Object(m) => m.iter().flat_map(|(k, x)| selectors(x, k)).collect(),
        _ => Vec::new(),
    }
}

/// The contract under rules (#1089): `ui/work-view/contract/gated.json` is the gate home's
/// answer, which the Node test runs claude-mem's mapper over in two zones. No rule on a key takes
/// the goals, Past focus or a request's completion; the Antigravity conversation goes by one
/// reference in every place the pages match it; and no value a rule finds goes. Written again as
/// the other contract is (OBOETE_CONTRACT_WRITE=1).
#[test]
fn the_contract_gated_snapshot_is_a_gate_homes_answer() {
    let name = "work_view::tests::the_contract_gated_snapshot_is_a_gate_homes_answer";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, now) = gate_home();
    let write = std::env::var_os("OBOETE_CONTRACT_WRITE").is_some();
    if first_minutes(now, write) {
        return;
    }
    let (work, goals) = answers(&h, now);
    let fresh = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    // The request a rule masks a part of the id of, and the one whose own id is what the first's
    // was before the gate read it again: each its own, and the first's shows no part it masks.
    let titled = |title: &str| -> Value {
        let found: Vec<&Value> = (arr(&fresh["humanRequests"]).iter())
            .filter(|r| r["title"] == title)
            .collect();
        assert_eq!(found.len(), 1, "{title}");
        found[0]["id"].clone()
    };
    let (first, second) = (titled("Plan the first one"), titled("Plan the second one"));
    assert_ne!(first, second);
    let text = fresh.to_string();
    assert!(
        !text.contains(AGY) && !text.contains("release-4827") && !text.contains("Orchid"),
        "{text}"
    );
    // Each selector the walk leaves as it is holds nothing a rule finds, nor does any other string.
    for (key, value) in selectors(&fresh, "") {
        assert_eq!(crate::redact::outbound(&value), value, "{key}");
    }
    for value in strings(&fresh) {
        assert_eq!(crate::redact::outbound(&value), value);
    }
    // The raw agent's conversation goes under the platform `other`, its own name in no selector.
    let raw = (arr(&fresh["agents"]).iter())
        .find(|a| a["sessionTitle"] == "Plan the raw one.")
        .expect("the raw agent's conversation");
    assert_eq!(raw["platformSource"], "other", "{raw}");
    assert!(
        raw["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("other:")),
        "{raw}"
    );
    // The counts and days, read by their keys.
    assert!(
        fresh["goals"]["sourcePromptCount"].is_i64(),
        "{}",
        fresh["goals"]
    );
    assert!(fresh["goals"]["windowDays"].is_i64(), "{}", fresh["goals"]);
    assert!(
        fresh["goalHistory"][0]["sourcePromptCount"].is_i64(),
        "{}",
        fresh["goalHistory"]
    );
    // A request its status completed is complete at that status's time, and one its review
    // shipped carries the review's day.
    let request = |id: &str| -> Value {
        let key = conversation_key("claude", id);
        one(&fresh["humanRequests"], "id", &key).clone()
    };
    let completed_at = &one(&work["requests"], "id", "c:claude:s3")["completed_at"];
    assert!(completed_at.is_i64(), "{completed_at}");
    assert_eq!(&request("c:claude:s3")["completedAtEpoch"], completed_at);
    let shipped = request("c:claude:s6");
    assert_eq!(shipped["progress"]["state"], "shipped");
    assert_eq!(shipped["progress"]["shippedAt"], "2026-11-17T12:00:00");
    // The Antigravity conversation by one reference: its request's id, its key, its agent's.
    let agy = one(&work["requests"], "id", &format!("c:agy:{AGY}"));
    assert!(
        agy["sessions"].as_array().is_some_and(|s| !s.is_empty()),
        "{agy}"
    );
    let refs: Vec<&Value> = arr(&fresh["humanRequests"])
        .iter()
        .filter(|r| r["id"].as_str().is_some_and(|id| id.contains("gemini")))
        .collect();
    assert_eq!(refs.len(), 1, "{}", fresh["humanRequests"]);
    let key: Vec<String> = serde_json::from_str(refs[0]["id"].as_str().unwrap()).unwrap();
    assert!(key[2].starts_with("m:"), "{key:?}");
    assert_eq!(refs[0]["conversationKeys"], json!([refs[0]["id"]]));
    let agent = format!("gemini:{}", key[2]);
    assert_eq!(refs[0]["agentIds"], json!([agent]));
    let agents: Vec<&Value> = arr(&fresh["agents"])
        .iter()
        .filter(|a| a["id"] == agent)
        .collect();
    assert_eq!(agents.len(), 1, "{}", fresh["agents"]);
    assert_eq!(agents[0]["contentSessionId"], key[2]);
    assert_eq!(agents[0]["platformSource"], "gemini");
    // The same from read to read.
    let (work, goals) = answers(&h, now);
    assert_eq!(
        snapshot_at(&work, &goals, now, &Utc, &Gate::now())["agents"],
        fresh["agents"]
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/work-view/contract/gated.json");
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    if steady(&kept) == steady(&fresh) {
        return;
    }
    if std::env::var_os("OBOETE_CONTRACT_WRITE").is_some() {
        let text = serde_json::to_string_pretty(&fresh).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    let (kept, fresh) = (steady(&kept), steady(&fresh));
    assert_eq!(
        kept, fresh,
        "the gated contract snapshot is not the gate home's answer"
    );
}

/// The finished snapshot goes through the gate in one walk as it leaves (#1089): each text as it
/// is sent, and none of what the pages match on, which a mask would move or join: a request's id,
/// its conversations' keys, its agent's, and the code's own words and times.
#[test]
fn the_snapshot_leaves_through_the_gate_in_one_walk() {
    let name = "work_view::tests::the_snapshot_leaves_through_the_gate_in_one_walk";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let gate = tempfile::tempdir().unwrap();
    let rules = "[redaction]\nextra_rules = [{ id = \"zebra\", regex = 'zebra\\w+' }]\n";
    std::fs::write(gate.path().join("config.toml"), rules).unwrap();
    crate::redact::set_home(gate.path()).unwrap();
    let work = json!({"requests": [{"id": "c:claude:zebra1", "status": "working",
        "title": "Feed the zebrafish", "ask": {"text": "Feed the zebrafish", "ts": 1},
        "sessions": [{"agent": "claude", "id": "c:claude:zebra1"}],
        "work": [{"list": "zebras", "repo": "r", "task": "Count the zebras", "status": "doing",
            "updated": 1}]}]});
    let snapshot = snapshot_at(&work, &json!({}), 0, &Utc, &Gate::now());
    let request = &snapshot["humanRequests"][0];
    let key = conversation_key("claude", "c:claude:zebra1");
    assert_eq!(request["id"], key);
    assert_eq!(request["conversationKeys"], json!([key]));
    assert_eq!(request["agentIds"], json!(["claude:c:claude:zebra1"]));
    assert_eq!(snapshot["agents"][0]["contentSessionId"], "c:claude:zebra1");
    assert_eq!(request["items"][0]["status"], "doing");
    for text in [
        &request["title"],
        &request["original"]["text"],
        &request["items"][0]["title"],
    ] {
        assert_eq!(text.as_str().unwrap().matches("zebra").count(), 0, "{text}");
    }
}

/// The route reads the Build story's texts as they were kept (#1089), not as My Work's read
/// gates them, so that `build_story` finds what the gate hides in the story it joins before any
/// mask; what leaves is gated all the same.
#[test]
fn the_route_reads_the_story_as_it_was_kept() {
    let name = "work_view::tests::the_route_reads_the_story_as_it_was_kept";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let mut h = Store::new();
    storied(&mut h, crate::db::now_ms() - 60_000, "Project Phoenix");
    h.run();
    let rules = "[redaction]\nextra_rules = [{ id = \"whole\", regex = '^Project Phoenix$' }]\n";
    std::fs::write(h.home.path().join("config.toml"), rules).unwrap();
    crate::redact::set_home(h.home.path()).unwrap();
    let now = crate::db::now_ms();
    let tag = |_: &crate::deliverables::Row| anyhow::Ok(String::new());
    let week = crate::my_work::Range::Week;
    let listed = crate::my_work::listed(h.home.path(), week, now, &tag).unwrap();
    assert_eq!(listed["story"]["recap"], crate::redact::MASK);
    let (work, goals) = answers(&h, now);
    assert_eq!(work["story"]["recap"], "Project Phoenix");
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    assert_eq!(snapshot["story"]["story"]["narrative"], crate::redact::MASK);
}

/// Decision 117: each request the snapshot shows is the one a check names by its id there, its
/// version the snapshot's and its place among the held ones its own, in a home whose
/// conversations' own requests go by their keys and in the gate home, whose ids go by references.
/// Where the snapshot shows a request by another id, neither its stored id, nor the id My Work
/// shows, nor its reference alone names it, nor any other.
#[test]
fn a_check_names_the_request_the_snapshot_shows() {
    let name = "work_view::tests::a_check_names_the_request_the_snapshot_shows";
    if !crate::view::tests::in_child(name) {
        return;
    }
    for (h, now) in [home(), gate_home()] {
        let home = h.home.path();
        crate::redact::set_home(home).unwrap();
        let snapshot = answered(home, now, Gate::now).unwrap();
        let shown = arr(&snapshot["humanRequests"]);
        assert!(shown.len() >= 5, "{snapshot}");
        for r in shown {
            let id = r["id"].as_str().unwrap();
            let Named::One(found, i, held) = named(home, now, id).unwrap() else {
                panic!("{id} names no one request");
            };
            assert_eq!(found["version"], r["contentFingerprint"], "{id}");
            if found["fingerprint"].is_null() {
                assert_eq!(found["id"], held[i].id(), "{id}");
            }
            let others = [
                held[i].id(),
                found["id"].as_str().unwrap_or(""),
                found["ref"].as_str().unwrap_or(""),
            ];
            for other in others.into_iter().filter(|o| !o.is_empty() && *o != id) {
                let other_named = named(home, now, other).unwrap();
                assert!(matches!(other_named, Named::Gone), "{other} for {id}");
            }
        }
    }
}

/// Decision 117: an id two requests would go by, as only two stored ids of one keyed print could
/// make, is told from an id one request goes by, whose place it gives, and from one none goes by:
/// a check of it names neither request (`Named::Shared`).
#[test]
fn an_id_two_requests_go_by_is_told_apart() {
    let by = |id: &str| json!({"id": id, "sessions": []});
    let one = json!({"id": "c:claude:s1", "sessions": [{"agent": "claude", "id": "c:claude:s1"}]});
    let requests = [by("m:0123456789ab"), one, by("m:0123456789ab"), by("r:rel")];
    assert_eq!(going_by(&requests, "m:0123456789ab"), [0, 2]);
    assert_eq!(going_by(&requests, "r:rel"), [3]);
    let key = conversation_key("claude", "c:claude:s1");
    assert_eq!(going_by(&requests, &key), [1]);
    assert!(going_by(&requests, "c:claude:s1").is_empty());
}

/// Decision 117: a rule saved since the pages read their snapshot makes the id they have of a
/// request it finds one no request goes by, and the snapshot after it shows the request by its
/// reference, which names it. A rule saved while a check's read runs is in force in it, as in the
/// snapshot's (`settled`), and rules that keep changing give up (`Unsettled`).
#[test]
fn a_check_reads_under_the_rules_the_snapshot_is_made_under() {
    let name = "work_view::tests::a_check_reads_under_the_rules_the_snapshot_is_made_under";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, now) = home();
    let home = h.home.path();
    crate::redact::set_home(home).unwrap();
    let key = conversation_key("codex", "c:codex:c1");
    let before = answered(home, now, Gate::now).unwrap();
    assert!(
        arr(&before["humanRequests"])
            .iter()
            .any(|r| r["id"] == key.as_str()),
        "{before}"
    );
    let rule = |n: usize| {
        format!(
            "[redaction]\nextra_rules = [{{ id = \"c1\", regex = '^c:codex:c1$' }}, \
             {{ id = \"n{n}\", regex = '^n{n}$' }}]\n"
        )
    };
    std::fs::write(home.join("config.toml"), rule(0)).unwrap();
    assert!(matches!(named(home, now, &key).unwrap(), Named::Gone));
    let after = answered(home, now, Gate::now).unwrap();
    let referred = (arr(&after["humanRequests"]).iter())
        .filter_map(|r| r["id"].as_str())
        .find(|id| id.starts_with(r#"["conversation","codex","m:"#))
        .expect("the codex conversation by its reference")
        .to_owned();
    assert!(matches!(
        named(home, now, &referred).unwrap(),
        Named::One(..)
    ));
    // Saved once the first rules were read, as the stores are read under them.
    std::fs::write(home.join("config.toml"), "[redaction]\n").unwrap();
    let mut reads = 0;
    let found = named_with(home, now, &key, || {
        reads += 1;
        if reads == 2 {
            std::fs::write(home.join("config.toml"), rule(1)).unwrap();
        }
        Gate::now()
    })
    .unwrap();
    assert_eq!(reads, 3);
    assert!(matches!(found, Named::Gone));
    let mut reads = 0;
    let unsettled = named_with(home, now, &referred, || {
        reads += 1;
        std::fs::write(home.join("config.toml"), rule(reads)).unwrap();
        Gate::now()
    });
    let e = unsettled.err().expect("rules that keep changing");
    assert!(e.is::<Unsettled>(), "{e:#}");
}

/// Decision 117: a check's read of My Work is made under the rules it is given, those its snapshot
/// was made under, whatever the home's file says (`my_work::reviewing_under`).
#[test]
fn a_checks_read_is_made_under_the_rules_it_is_given() {
    let (h, now) = home();
    let other = tempfile::tempdir().unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"c1\", regex = '^c1$' }]\n";
    std::fs::write(other.path().join("config.toml"), rule).unwrap();
    let ruled = crate::redact::Rules::load(other.path()).unwrap();
    let (work, held) = crate::my_work::reviewing_under(h.home.path(), now, &ruled).unwrap();
    let codex = (arr(&work["requests"]).iter())
        .flat_map(|r| arr(&r["sessions"]))
        .find(|c| c["agent"] == "codex")
        .expect("the codex conversation");
    assert!(codex["fingerprint"].is_string(), "{codex}");
    assert_eq!(held.len(), arr(&work["requests"]).len());
}

/// J4 on the work-view (W3b-3): a request with a judge's open suggestion carries claude-mem's,
/// its own id and the id of the request it names as the snapshot gives them, that request's
/// title, and My Work's binding, and nothing else of My Work's suggestion.
#[test]
fn a_judges_open_suggestion_goes_with_the_request_it_names() {
    let (h, now) = suggested("s2");
    let (work, goals) = answers(&h, now);
    let mine: Vec<&Value> = (arr(&work["requests"]).iter())
        .filter(|r| r.get("suggestion").is_some())
        .collect();
    assert_eq!(mine.len(), 1, "{work}");
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let with: Vec<&Value> = (arr(&snapshot["humanRequests"]).iter())
        .filter(|r| r.get("suggestion").is_some())
        .collect();
    assert_eq!(with.len(), 1, "{snapshot}");
    let parser = one(&snapshot["humanRequests"], "title", "Parser");
    assert!(parser["id"].as_str().unwrap().starts_with("g:"), "{parser}");
    assert_eq!(
        with[0]["suggestion"],
        json!({"conversationKey": with[0]["id"], "requestId": parser["id"],
            "requestTitle": "Parser", "evidence": "",
            "expectedSuggestionHash": mine[0]["suggestion"]["binding"]})
    );
}

/// The request a suggestion names is the one row with its id and fingerprint (round 2 of W3b's
/// plan review): none goes when two rows go by them, when none does, or when the one by its id
/// has another fingerprint. A request with no title the pages show is named by its ask, else as
/// untitled.
#[test]
fn a_suggestion_names_one_request_or_goes_with_none() {
    let carrier = json!({"id": "c:claude:s2", "sessions": [{"agent": "claude",
        "id": "c:claude:s2"}], "suggestion": {"id": "g:aaaaaaaaaaaa",
        "fingerprint": "aaaaaaaaaaaa", "title": "Parser", "binding": "0".repeat(64)}});
    let target = json!({"id": "g:aaaaaaaaaaaa", "fingerprint": "aaaaaaaaaaaa", "title": null,
        "ask": {"text": "Build the parser."}, "sessions": []});
    let named = |rows: &[&Value]| -> Value {
        let work = json!({"requests": rows});
        let snapshot = snapshot_at(&work, &json!({}), crate::db::now_ms(), &Utc, &Gate::now());
        snapshot["humanRequests"][0]["suggestion"].clone()
    };
    let found = named(&[&carrier, &target]);
    assert_eq!(found["requestId"], "g:aaaaaaaaaaaa");
    assert_eq!(found["requestTitle"], "Build the parser.");
    assert_eq!(
        found["conversationKey"],
        conversation_key("claude", "c:claude:s2")
    );
    assert!(named(&[&carrier, &target, &target]).is_null());
    assert!(named(&[&carrier]).is_null());
    let mut other = target.clone();
    other["fingerprint"] = json!("bbbbbbbbbbbb");
    assert!(named(&[&carrier, &other]).is_null());
    let mut untitled = target.clone();
    untitled["ask"]["text"] = json!(" ");
    let found = named(&[&carrier, &untitled]);
    assert_eq!(found["requestTitle"], crate::my_work_judge::UNTITLED);
    // A title the mapper would show as none (`to-work-model.ts:45`) is passed over for the ask.
    for blank in ["2026", "null", " True "] {
        let mut titled = target.clone();
        titled["title"] = json!(blank);
        let found = named(&[&carrier, &titled]);
        assert_eq!(found["requestTitle"], "Build the parser.", "{blank}");
    }
}

/// A suggestion on a request whose conversation a rule masks (W3b-3): its own id and the id of the
/// request it names go as the snapshot gives them, by their references, the stored id of the
/// conversation is nowhere in it, the binding goes whole though the rule finds hex digits in it,
/// and the route's resolver finds the request by the id the suggestion gives it.
#[test]
fn a_suggestion_on_a_masked_conversation_goes_by_its_references() {
    let name = "work_view::tests::a_suggestion_on_a_masked_conversation_goes_by_its_references";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let session = "0123456789ab";
    let (h, _) = suggested(session);
    let home = h.home.path();
    let rules = "[redaction]\nextra_rules = [{ id = 'hex', regex = '[0-9a-f]{12}' }]\n";
    std::fs::write(home.join("config.toml"), rules).unwrap();
    crate::redact::set_home(home).unwrap();
    let now = crate::db::now_ms();
    let snapshot = answer(home, now).unwrap();
    let text = snapshot.to_string();
    assert!(!text.contains(session), "{text}");
    let requests = arr(&snapshot["humanRequests"]);
    let carrier = (requests.iter())
        .find(|r| r.get("suggestion").is_some())
        .expect("the suggestion");
    let suggestion = &carrier["suggestion"];
    assert_eq!(suggestion["conversationKey"], carrier["id"]);
    assert!(
        (requests.iter()).any(|r| r["id"] == suggestion["requestId"]),
        "{snapshot}"
    );
    assert_eq!(suggestion["requestTitle"], "Parser");
    let binding = suggestion["expectedSuggestionHash"].as_str().unwrap();
    assert!(hex(binding, 64), "{binding}");
    let key = suggestion["conversationKey"].as_str().unwrap();
    assert!(matches!(named(home, now, key).unwrap(), Named::One(..)));
}

/// The contract of a judge's suggestion (W3b-3): `ui/work-view/contract/suggested.json` is the
/// suggestion home's answer, which the Node test reads through claude-mem's mapper; written again
/// as the other contract files are (OBOETE_CONTRACT_WRITE=1).
#[test]
fn the_contract_suggestion_is_a_suggestion_homes_answer() {
    let (h, now) = suggested("s2");
    let (work, goals) = answers(&h, now);
    let fresh = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    contract_holds("suggested.json", &fresh);
}

/// `fresh` is the contract file `file` (`ui/work-view/contract`), as `steady` reads both; with
/// OBOETE_CONTRACT_WRITE=1, `fresh` is written there.
fn contract_holds(file: &str, fresh: &Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui/work-view/contract")
        .join(file);
    if std::env::var_os("OBOETE_CONTRACT_WRITE").is_some() {
        let text = serde_json::to_string_pretty(fresh).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        steady(&kept),
        steady(fresh),
        "{file} is not this home's answer"
    );
}

/// Where each of `keys` is in the contract file `file`, as paths (`/agents/0/isSubagent`).
fn paths_in(file: &str, keys: &[&str]) -> Vec<String> {
    fn walk(v: &Value, path: &str, keys: &[&str], out: &mut Vec<String>) {
        match v {
            Value::Array(a) => {
                (a.iter().enumerate()).for_each(|(i, v)| walk(v, &format!("{path}/{i}"), keys, out))
            }
            Value::Object(o) => {
                for (k, v) in o {
                    let at = format!("{path}/{k}");
                    if keys.contains(&k.as_str()) {
                        out.push(at.clone());
                    }
                    walk(v, &at, keys, out);
                }
            }
            _ => {}
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui/work-view/contract")
        .join(file);
    let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut found = Vec::new();
    walk(&v, "", keys, &mut found);
    found
}

/// Keys the walk leaves as they are (`OWN`) name nothing else in the contract's answers: the
/// suggestion's three, each a request's suggestion's (W3b-3); a subagent's `isSubagent`,
/// `canMarkFinished` and `finishedAt`, each an agent's (W4); a rule's references and the fields
/// oboete writes of it, each a rule's (#1083), but for a focus area's `savedAt`, which is null.
#[test]
fn the_keys_the_walk_leaves_whole_are_theirs_alone() {
    let files = [
        "snapshot.json",
        "gated.json",
        "suggested.json",
        "subagented.json",
        "ruled.json",
    ];
    for (keys, own, least) in [
        (
            &["conversationKey", "requestId", "expectedSuggestionHash"][..],
            r"^/humanRequests/\d+/suggestion/[A-Za-z]+$",
            ("suggested.json", 3),
        ),
        (
            &["isSubagent", "canMarkFinished", "finishedAt"][..],
            r"^/agents/\d+/(isSubagent|canMarkFinished|presence/finishedAt)$",
            ("subagented.json", 9),
        ),
        (
            &[
                "ruleId",
                "scopeId",
                "replacedBy",
                "setBy",
                "scopeTier",
                "when",
                "savedAt",
                "replacedAt",
                "ruleVersion",
            ][..],
            r"^/rules/\d+/(ruleId|scopeId|replacedBy|setBy|scopeTier|when|savedAt|replacedAt|ruleVersion)$|^/focusAreas/\d+/savedAt$",
            ("ruled.json", 36),
        ),
    ] {
        let own = Regex::new(own).unwrap();
        for file in files {
            let found = paths_in(file, keys);
            let at_least = if file == least.0 { least.1 } else { 0 };
            assert!(found.len() >= at_least, "{file}: {found:?}");
            for at in &found {
                assert!(own.is_match(at), "{file}: {at}");
            }
        }
    }
    for file in files {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui/work-view/contract")
            .join(file);
        let kept: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for area in arr(&kept["focusAreas"]) {
            assert!(area["savedAt"].is_null(), "{file}: {area}");
        }
    }
}

/// A step of subagent `id` in `session` at `at`, as Claude Code's hook records it.
fn sub_step(h: &mut Store, session: &str, at: i64, id: &str) {
    let body = json!({"tool": "Read", "agent_id": id, "agent_type": "Explore"});
    event(h, "tool", "claude", session, at, body);
}

/// A stop of subagent `id` in `session` at `at`.
fn sub_stop(h: &mut Store, session: &str, at: i64, id: &str) {
    event(
        h,
        crate::raw::SUBAGENT_STOP,
        "claude",
        session,
        at,
        json!({"agent_id": id}),
    );
}

/// W4: a home whose conversation `s1`, of this week, has four subagents: `a0` working (its last
/// step 2 minutes back), `a1` quiet (its last step 20 minutes back, no stop), `a2` finished 5
/// minutes back, `a3` finished 40 minutes back; and whose conversation `s0`, eight days back, has
/// `b1`, quiet.
fn subagented() -> (Store, i64) {
    let mut h = Store::new();
    let now = crate::db::now_ms();
    let min = 60_000;
    h.said("s1", R, now - 45 * min, "Fix the parser.");
    let old = now - 8 * 24 * 60 * min;
    h.said("s0", R, old, "Tidy the old notes.");
    sub_step(&mut h, "s1", now - 2 * min, "a0");
    sub_step(&mut h, "s1", now - 20 * min, "a1");
    sub_step(&mut h, "s1", now - 6 * min, "a2");
    sub_stop(&mut h, "s1", now - 5 * min, "a2");
    sub_step(&mut h, "s1", now - 41 * min, "a3");
    sub_stop(&mut h, "s1", now - 40 * min, "a3");
    sub_step(&mut h, "s0", old + min, "b1");
    h.run();
    (h, now)
}

/// Decision 118 (W4): the snapshot's agents hold a subagent of a conversation it carries while it
/// has not finished, and for `RECENT_MS` after it finished, each after every conversation's agent,
/// with its own id and its conversation's label, title, project and content session id; none of a
/// conversation the week leaves out.
#[test]
fn a_carried_conversations_subagents_go_while_not_finished_or_lately_finished() {
    let (h, now) = subagented();
    let (work, goals) = answers(&h, now);
    assert_eq!(arr(&work["subagents"]).len(), 3, "{}", work["subagents"]);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let agents = arr(&snapshot["agents"]);
    let subs: Vec<&Value> = agents.iter().filter(|a| a["isSubagent"] == true).collect();
    assert_eq!(subs.len(), 3, "{snapshot}");
    let first = agents.iter().position(|a| a["isSubagent"] == true).unwrap();
    assert!(
        agents[first..].iter().all(|a| a["isSubagent"] == true),
        "after every conversation's: {snapshot}"
    );
    let parent = (agents.iter())
        .find(|a| a["isSubagent"] != true && a["contentSessionId"] == subs[0]["contentSessionId"])
        .expect("the conversation's agent");
    for sub in &subs {
        for key in [
            "label",
            "platformSource",
            "sessionTitle",
            "project",
            "contentSessionId",
        ] {
            assert_eq!(sub[key], parent[key], "{key} of {sub}");
        }
        assert!(sub["id"].as_str().unwrap().starts_with("s:"), "{sub}");
    }
    let ids: std::collections::HashSet<&Value> = subs.iter().map(|s| &s["id"]).collect();
    assert_eq!(ids.len(), 3, "{snapshot}");
    let by = |status: &str| -> Vec<&Value> {
        (subs.iter().copied())
            .filter(|s| s["presence"]["status"] == status)
            .collect()
    };
    for status in ["recent", "quiet"] {
        let one = by(status);
        assert_eq!(one.len(), 1, "{status}: {snapshot}");
        assert_eq!(one[0]["canMarkFinished"], true);
        assert_eq!(one[0]["presence"]["finishedAt"], Value::Null);
    }
    let finished = by("finished");
    assert_eq!(finished.len(), 1, "{snapshot}");
    assert_eq!(finished[0]["canMarkFinished"], false);
    // Its stop's time, 5 minutes before the home's `now`, as an ISO time.
    assert_eq!(finished[0]["presence"]["finishedAt"], iso(now - 5 * 60_000));
}

/// W4 (the plan review's round 1): a subagent goes with the session whose shown id and fingerprint
/// are both its conversation's, though another session shows the same id (a masked one), or with
/// none.
#[test]
fn a_subagent_goes_with_the_session_its_id_and_fingerprint_name() {
    let row = |id: &str, title: &str, fingerprint: &str, reference: &str| {
        json!({"id": id, "title": title, "projects": [title], "sessions": [{"agent": "claude",
            "id": "c:claude:[REDACTED]", "fingerprint": fingerprint, "ref": reference}]})
    };
    let sub = |fingerprint: &str| {
        json!({"conversation": {"id": "c:claude:[REDACTED]", "fingerprint": fingerprint},
            "id": "s:0123456789ab", "state": "quiet", "can_mark": true, "finished_at": null})
    };
    let work = |fingerprint: &str| {
        json!({"requests": [row("r1", "One", "f1", "r:1"), row("r2", "Two", "f2", "r:2")],
            "subagents": [sub(fingerprint)]})
    };
    let agents = |work: &Value| {
        let snapshot = snapshot_at(work, &json!({}), crate::db::now_ms(), &Utc, &Gate::now());
        snapshot["agents"].clone()
    };
    let found = agents(&work("f2"));
    let child = one(&found, "id", "s:0123456789ab");
    assert_eq!(child["sessionTitle"], "Two", "{found}");
    assert_eq!(child["contentSessionId"], "r:2", "{found}");
    let found = agents(&work("f3"));
    assert!(
        arr(&found).iter().all(|a| a["isSubagent"] != true),
        "{found}"
    );
}

/// W4: a subagent's label is gated as its conversation's: a rule that masks the conversation's
/// title masks it alike; its id and `finishedAt` leave whole though rules find hex digits and a
/// time in them (`OWN`).
#[test]
fn a_subagents_label_is_gated_as_its_conversations() {
    let name = "work_view::tests::a_subagents_label_is_gated_as_its_conversations";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, _) = subagented();
    let home = h.home.path();
    let rules = "[redaction]\nextra_rules = [{ id = 'parser', regex = 'parser' }, \
                 { id = 'hex', regex = '[0-9a-f]{12}' }, { id = 'time', regex = 'T[0-9]{2}:[0-9]{2}' }]\n";
    std::fs::write(home.join("config.toml"), rules).unwrap();
    crate::redact::set_home(home).unwrap();
    let snapshot = answer(home, crate::db::now_ms()).unwrap();
    let agents = arr(&snapshot["agents"]);
    let subs: Vec<&Value> = agents.iter().filter(|a| a["isSubagent"] == true).collect();
    assert_eq!(subs.len(), 3, "{snapshot}");
    let parent = (agents.iter())
        .find(|a| a["isSubagent"] != true && a["contentSessionId"] == subs[0]["contentSessionId"])
        .unwrap();
    assert!(
        !parent["sessionTitle"].to_string().contains("parser"),
        "{parent}"
    );
    let print = Regex::new(r"^s:[0-9a-f]{12}$").unwrap();
    for sub in &subs {
        assert_eq!(sub["sessionTitle"], parent["sessionTitle"], "{sub}");
        let id = sub["id"].as_str().unwrap();
        assert!(print.is_match(id), "{id}");
    }
    let finished = (subs.iter())
        .find(|s| s["presence"]["status"] == "finished")
        .unwrap();
    let at = finished["presence"]["finishedAt"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(at).is_ok(), "{at}");
}

/// The contract of a carried conversation's subagents (W4, decision 118):
/// `ui/work-view/contract/subagented.json` is the subagent home's answer, which the Node test reads
/// through claude-mem's mapper; written again as the other contract files are
/// (OBOETE_CONTRACT_WRITE=1).
#[test]
fn the_contract_subagents_are_a_subagent_homes_answer() {
    let (h, now) = subagented();
    let (work, goals) = answers(&h, now);
    contract_holds(
        "subagented.json",
        &snapshot_at(&work, &goals, now, &Utc, &Gate::now()),
    );
}

/// A rules op of suggestion `id` as the rules job keeps one (`my_work_rules`): seen twice, quoted
/// at `at`.
fn suggestion_op(h: &mut Store, id: &str, statement: &str, at: i64) {
    let fields = json!({"statement": statement, "seen": 2,
        "requests": [format!("g:{id}0"), format!("g:{id}1")],
        "quotes": [{"text": format!("Quoted for {id}."), "request": format!("g:{id}0"), "at": at}]});
    let op = MyWorkOp {
        job: crate::my_work_rules::NAME.into(),
        about: id.into(),
        entry: "fake".into(),
        ts: crate::db::now_ms(),
        read: Read::default(),
        removed: vec![],
        fields: fields.as_object().unwrap().clone().into_iter().collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
}

/// A checkout at `dir` with no remote, which a prompt of `session` started in: its repository key.
fn started_in(h: &mut Store, dir: &Path, session: &str, at: i64) -> String {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let repo = crate::repo::key(dir);
    h.said(session, &repo, at, "Hello.");
    repo
}

/// The preference a claim writer recorded.
fn uid_of(receipt: crate::claims::OwnerReceipt) -> String {
    match receipt {
        crate::claims::OwnerReceipt::Applied { uid }
        | crate::claims::OwnerReceipt::Pending { uid, .. } => uid,
        crate::claims::OwnerReceipt::DirectiveOnly { .. } => panic!("no preference recorded"),
    }
}

/// The home of My Rules (#1083, R1): suggestion s1 waiting, s2 approved for every repository, s3
/// rejected; a rule the owner wrote for every repository, turned off; and one for a repository a
/// session started in. With its place of checkouts, that repository's key and the off rule's uid.
fn ruled() -> (Store, i64, tempfile::TempDir, String, String) {
    let mut h = Store::new();
    let now = crate::db::now_ms();
    let statements = [
        ("s1", "Keep commits small."),
        ("s2", "Run the tests before a push."),
        ("s3", "Use tabs."),
    ];
    for (n, (id, statement)) in (0i64..).zip(statements) {
        suggestion_op(&mut h, id, statement, now - (3 - n) * 60_000);
    }
    let place = tempfile::tempdir().unwrap();
    let repo = started_in(
        &mut h,
        &place.path().join("proj"),
        "in-proj",
        now - 4 * 60_000,
    );
    h.run();
    let home = h.home.path().to_owned();
    let act = |body: Value| {
        let body = serde_json::to_vec(&body).unwrap();
        crate::settings::claims::rule(&home, &std::sync::Mutex::new(()), &body).unwrap();
    };
    act(json!({"suggestion": "s2", "action": "approve", "scope": "global"}));
    act(json!({"suggestion": "s3", "action": "reject"}));
    uid_of(crate::claims::pref_add_recorded(&home, "Write the changelog.", Some(&repo)).unwrap());
    let off = crate::claims::pref_add_recorded(&home, "Answer in Japanese.", None).unwrap();
    let off = uid_of(off);
    crate::claims::mute_recorded(&home, &off, true).unwrap();
    h.run();
    (h, now, place, repo, off)
}

/// The one rule of `snapshot` whose `then` is `then`.
fn rule_of<'a>(snapshot: &'a Value, then: &str) -> &'a Value {
    one(&snapshot["rules"], "then", then)
}

/// #1083 (R1): My Rules goes in the snapshot as claude-mem's pages read it: a waiting suggestion
/// learned and suggested, an approved one by its suggestion's id, the owner's rules (one turned
/// off, one of a repository under that repository's scope), and the rejected one left out; each
/// by a keyed print, stable across reads, and no suggestion id, uid or repository key on the wire.
#[test]
fn my_rules_go_by_keyed_prints_with_who_set_them_and_their_states() {
    let (h, now, _place, repo, off) = ruled();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let fields = |r: &Value| {
        ["setBy", "status", "source", "scopeTier"].map(|k| r[k].as_str().unwrap().to_owned())
    };
    let waiting = rule_of(&snapshot, "Keep commits small.");
    assert_eq!(
        fields(waiting),
        ["learned", "suggested", "request-progress", "everywhere"]
    );
    let approved = rule_of(&snapshot, "Run the tests before a push.");
    assert_eq!(
        fields(approved),
        ["approved", "on", "request-progress", "everywhere"]
    );
    let turned_off = rule_of(&snapshot, "Answer in Japanese.");
    assert_eq!(fields(turned_off), ["owner", "off", "you", "everywhere"]);
    let theirs = rule_of(&snapshot, "Write the changelog.");
    assert_eq!(fields(theirs), ["owner", "on", "you", "project"]);
    assert!(
        arr(&snapshot["rules"])
            .iter()
            .all(|r| r["then"] != "Use tabs."),
        "{snapshot}"
    );
    assert_eq!(arr(&snapshot["rules"]).len(), 4, "{snapshot}");
    let scope = one(
        &snapshot["ruleScopes"],
        "id",
        theirs["scopeId"].as_str().unwrap(),
    );
    assert_eq!(
        (scope["tier"].as_str(), scope["name"].as_str()),
        (Some("project"), Some("proj"))
    );
    assert_eq!(theirs["scopeName"], "proj");
    let print = |prefix: &str| Regex::new(&format!("^{prefix}:[0-9a-f]{{64}}$")).unwrap();
    for (r, prefix) in [
        (waiting, "l"),
        (approved, "l"),
        (turned_off, "u"),
        (theirs, "u"),
    ] {
        let id = r["ruleId"].as_str().unwrap();
        assert!(print(prefix).is_match(id), "{id}");
        assert_eq!(r["when"], "", "{r}");
    }
    assert!(
        print("r").is_match(scope["id"].as_str().unwrap()),
        "{scope}"
    );
    assert_eq!(waiting["quotes"][0]["quote"], "Quoted for s1.");
    assert_eq!(waiting["quotes"][0]["kind"], "correction");
    assert!(chrono::DateTime::parse_from_rfc3339(waiting["latestAt"].as_str().unwrap()).is_ok());
    assert!(chrono::DateTime::parse_from_rfc3339(turned_off["savedAt"].as_str().unwrap()).is_ok());
    let wire = json!([snapshot["rules"], snapshot["ruleScopes"]]).to_string();
    for held in [repo.as_str(), off.as_str(), "\"s1\"", "\"s2\""] {
        assert!(!wire.contains(held), "{held} in {wire}");
    }
    let (work, goals) = answers(&h, now);
    let again = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let ids = |s: &Value| -> Vec<Value> {
        arr(&s["rules"])
            .iter()
            .map(|r| r["ruleId"].clone())
            .collect()
    };
    assert_eq!(ids(&again), ids(&snapshot));
}

/// #1083 (R1): a home whose only suggestion was rejected sends no rule, so the pages show that
/// there is none yet.
#[test]
fn a_home_of_a_rejected_suggestion_alone_sends_no_rule() {
    let mut h = Store::new();
    let now = crate::db::now_ms();
    suggestion_op(&mut h, "s1", "Use tabs.", now - 60_000);
    h.run();
    let body = serde_json::to_vec(&json!({"suggestion": "s1", "action": "reject"})).unwrap();
    let saving = std::sync::Mutex::new(());
    crate::settings::claims::rule(h.home.path(), &saving, &body).unwrap();
    h.run();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    assert_eq!(snapshot["rules"], json!([]), "{snapshot}");
}

/// #1083 (R1): two repositories of one name, each with a rule, keep that name as the gate leaves
/// it, nothing appended, each scope with an id of its own; a repository with no rule is no scope.
#[test]
fn two_repositories_of_one_name_keep_it_and_ids_of_their_own() {
    let mut h = Store::new();
    let now = crate::db::now_ms();
    let place = tempfile::tempdir().unwrap();
    let a = started_in(
        &mut h,
        &place.path().join("a/proj"),
        "in-a",
        now - 2 * 60_000,
    );
    let b = started_in(&mut h, &place.path().join("b/proj"), "in-b", now - 60_000);
    assert_ne!(a, b);
    let c = started_in(&mut h, &place.path().join("c/none"), "in-c", now - 60_000);
    h.run();
    for (repo, text) in [(&a, "Write the changelog."), (&b, "Keep the readme short.")] {
        uid_of(crate::claims::pref_add_recorded(h.home.path(), text, Some(repo)).unwrap());
    }
    h.run();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let scopes: Vec<&Value> = (arr(&snapshot["ruleScopes"]).iter())
        .filter(|s| s["tier"] == "project")
        .collect();
    assert_eq!(scopes.len(), 2, "{snapshot}");
    assert!(scopes.iter().all(|s| s["name"] == "proj"), "{scopes:?}");
    assert_ne!(scopes[0]["id"], scopes[1]["id"]);
    assert!(!snapshot["ruleScopes"].to_string().contains("none"), "{c}");
}

/// #1083 (R1): with the gate failing closed, a rule's texts are masked whole and the fields
/// oboete writes stay as written, its empty condition among them, so the pages show no condition.
#[test]
fn a_gate_failing_closed_leaves_a_rules_own_fields_and_its_empty_condition() {
    let (h, now, ..) = ruled();
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate(None));
    for r in arr(&snapshot["rules"]) {
        assert_eq!(r["when"], "", "{r}");
        assert_eq!(r["then"], crate::redact::MASK, "{r}");
        assert!(
            ["learned", "approved", "owner"].contains(&r["setBy"].as_str().unwrap()),
            "{r}"
        );
    }
}

/// #1083 (R1): a redaction rule on oboete's own words of a rule (who set it, its state, its source,
/// its kind) leaves them; one saved in the home's rules while the work view is read makes it read My
/// Rules again under them, though the gate the snapshot leaves through has none.
#[test]
fn a_rule_on_oboetes_words_leaves_them_and_one_saved_during_the_read_is_read_under() {
    let name = "work_view::tests::a_rule_on_oboetes_words_leaves_them_and_one_saved_during_the_read_is_read_under";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, now, ..) = ruled();
    let home = h.home.path();
    let words = "[redaction]\nextra_rules = [{ id = 'words', regex = \
                 'approved|owner|learned|suggested|request-progress|preference|correction|project' }]\n";
    std::fs::write(home.join("config.toml"), words).unwrap();
    crate::redact::set_home(home).unwrap();
    let snapshot = answer(home, now).unwrap();
    let approved = rule_of(&snapshot, "Run the tests before a push.");
    assert_eq!(
        (approved["setBy"].as_str(), approved["source"].as_str()),
        (Some("approved"), Some("request-progress"))
    );
    assert_eq!(approved["quotes"][0]["kind"], "correction");
    let theirs = rule_of(&snapshot, "Write the changelog.");
    assert_eq!(theirs["scopeTier"], "project");
    // A rule on a repository's name, saved in the home's rules once the first were read, and none in
    // the gate's: My Rules is read again under it, as the walk alone would leave the name.
    let gate = tempfile::tempdir().unwrap();
    std::fs::write(gate.path().join("config.toml"), "").unwrap();
    std::fs::write(home.join("config.toml"), "").unwrap();
    crate::redact::set_home(gate.path()).unwrap();
    let name = "[redaction]\nextra_rules = [{ id = 'proj', regex = 'proj' }]\n";
    let mut reads = 0;
    let snapshot = answered(home, now, || {
        reads += 1;
        if reads == 2 {
            std::fs::write(home.join("config.toml"), name).unwrap();
        }
        Gate::now()
    })
    .unwrap();
    assert_eq!(reads, 3);
    let theirs = rule_of(&snapshot, "Write the changelog.");
    let scope = one(
        &snapshot["ruleScopes"],
        "id",
        theirs["scopeId"].as_str().unwrap(),
    );
    assert!(!scope["name"].to_string().contains("proj"), "{scope}");
    assert!(
        !theirs["scopeName"].to_string().contains("proj"),
        "{theirs}"
    );
}

/// The contract of My Rules (#1083, R1): `ui/work-view/contract/ruled.json` is the rules home's
/// answer, which the Node test reads through claude-mem's mapper; written again as the other
/// contract files are (OBOETE_CONTRACT_WRITE=1).
#[test]
fn the_contract_rules_are_a_rules_homes_answer() {
    let (h, now, ..) = ruled();
    let (work, goals) = answers(&h, now);
    contract_holds(
        "ruled.json",
        &snapshot_at(&work, &goals, now, &Utc, &Gate::now()),
    );
}

/// #1083 (R1): a rule on a repository's name masks the scope's name and its rules' scope names,
/// and one on a quote masks the quote; the scope keeps its id.
#[test]
fn a_rule_on_a_repository_name_or_a_quote_masks_it() {
    let name = "work_view::tests::a_rule_on_a_repository_name_or_a_quote_masks_it";
    if !crate::view::tests::in_child(name) {
        return;
    }
    let (h, now, ..) = ruled();
    let home = h.home.path();
    let rules = "[redaction]\nextra_rules = [{ id = 'proj', regex = 'proj' }, \
                 { id = 'quoted', regex = 'Quoted for s1' }]\n";
    std::fs::write(home.join("config.toml"), rules).unwrap();
    crate::redact::set_home(home).unwrap();
    let snapshot = answer(home, now).unwrap();
    let text = json!([snapshot["rules"], snapshot["ruleScopes"]]).to_string();
    assert!(!text.contains("Quoted for s1"), "{text}");
    let theirs = rule_of(&snapshot, "Write the changelog.");
    // The tier, oboete's word, is `project`, which the rule finds too: it is left whole.
    assert!(
        !theirs["scopeName"].to_string().contains("proj"),
        "{theirs}"
    );
    assert_eq!(theirs["scopeTier"], "project");
    let scope = one(
        &snapshot["ruleScopes"],
        "id",
        theirs["scopeId"].as_str().unwrap(),
    );
    assert_eq!(scope["tier"], "project");
    assert!(!scope["name"].to_string().contains("proj"), "{scope}");
    assert_eq!(scope["name"], theirs["scopeName"]);
    assert!(rule_of(&snapshot, "Run the tests before a push.")["quotes"][0]["quote"].is_string());
}

/// #1083 (R1): an approved suggestion whose rule was retracted since is left out (R2 brings it
/// with its Undo), so a home of that alone sends no rule.
#[test]
fn a_home_of_a_removed_approval_alone_sends_no_rule() {
    let mut h = Store::new();
    let now = crate::db::now_ms();
    suggestion_op(&mut h, "s1", "Keep commits small.", now - 60_000);
    h.run();
    let body = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let saving = std::sync::Mutex::new(());
    let saved =
        crate::settings::claims::rule(h.home.path(), &saving, &serde_json::to_vec(&body).unwrap())
            .unwrap();
    h.run();
    let uid = saved["uid"].as_str().unwrap().to_owned();
    crate::claims::correct_recorded(h.home.path(), &uid, Some("retracted"), None).unwrap();
    h.run();
    let page = crate::my_work_rules::page(h.home.path(), now).unwrap();
    assert_eq!(arr(&page["removed"]).len(), 1, "{page}");
    let (work, goals) = answers(&h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    assert_eq!(snapshot["rules"], json!([]), "{snapshot}");
}

/// #1083 (R2a): the rule of `then` in the snapshot read now: its reference and version.
fn read_rule(h: &Store, then: &str) -> (String, String, Value) {
    let now = crate::db::now_ms();
    let (work, goals) = answers(h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    let rule = rule_of(&snapshot, then).clone();
    let id = rule["ruleId"].as_str().unwrap().to_owned();
    let version = rule["ruleVersion"].as_str().unwrap().to_owned();
    (id, version, rule)
}

/// The correction ops the home's raw.db holds.
fn corrections(h: &Store) -> usize {
    let device = h.raw.device().to_owned();
    let ops = h.raw.ops_after(&device, 0, 100_000).unwrap();
    ops.iter()
        .filter(|op| op.kind == OpKind::Correction)
        .count()
}

/// `switch` on `h`'s home, its token asked and taken.
fn switched(h: &Store, reference: &str, version: &str, muted: bool) -> Switched {
    let saving = std::sync::Mutex::new(());
    let take = || saving.lock().ok();
    switch(h.home.path(), (reference, version), muted, &take, &|| true).unwrap()
}

/// The version an applied switch left its rule at.
fn left(s: Switched) -> String {
    match s {
        Switched::Applied(Some(version)) => version,
        other => panic!("not applied with a version: {other:?}"),
    }
}

/// `uid`'s mute correction, appended on a raw.db handle of its own, as another writer appends one.
fn muted_elsewhere(home: &Path, uid: &str, muted: bool) {
    let k = crate::knowledge::open(home).unwrap();
    let op = crate::claims::mute_op(&k, uid, muted).unwrap().unwrap();
    let mut raw = crate::raw::open(home).unwrap();
    raw.append_ops(&[(OpKind::Correction, op)]).unwrap();
}

/// #1083 (R2a): a rule the owner wrote turned off and on again from the pages: each switch is
/// applied, the rule reads as it says, and each leaves the rule at a version of its own, the one the
/// next read gives, another than any before it even where the state is one it had.
#[test]
fn a_rule_switched_off_and_on_reads_so_at_the_version_its_answer_gives() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (id, v0, rule) = read_rule(&h, "Write the changelog.");
    assert_eq!(rule["status"], "on");
    let v1 = left(switched(&h, &id, &v0, true));
    let (again, version, rule) = read_rule(&h, "Write the changelog.");
    assert_eq!(
        (again.as_str(), rule["status"].as_str()),
        (id.as_str(), Some("off"))
    );
    assert_eq!(version, v1);
    assert_ne!(v1, v0);
    let v2 = left(switched(&h, &id, &v1, false));
    let (_, version, rule) = read_rule(&h, "Write the changelog.");
    assert_eq!(
        (rule["status"].as_str(), version.as_str()),
        (Some("on"), v2.as_str())
    );
    assert_ne!(
        v2, v0,
        "the state is the one it had, at a version of its own"
    );
}

/// An approved rule switches as one the owner wrote, and keeps its reference and who set it.
#[test]
fn an_approved_rule_switches_off_and_keeps_its_reference() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (id, v0, rule) = read_rule(&h, "Run the tests before a push.");
    assert_eq!(
        (rule["setBy"].as_str(), id.starts_with("l:")),
        (Some("approved"), true)
    );
    let v1 = left(switched(&h, &id, &v0, true));
    let (again, version, rule) = read_rule(&h, "Run the tests before a push.");
    assert_eq!((again, version), (id, v1));
    assert_eq!(
        (rule["status"].as_str(), rule["setBy"].as_str()),
        (Some("off"), Some("approved"))
    );
}

/// A switch the rule cannot take records nothing: a suggestion waiting, a rule already as asked, a
/// version not the rule's, and a reference no rule goes by.
#[test]
fn a_switch_its_rule_cannot_take_records_nothing() {
    let (h, _now, _place, _repo, _off) = ruled();
    let before = corrections(&h);
    let (waiting, version, _) = read_rule(&h, "Keep commits small.");
    assert_eq!(switched(&h, &waiting, &version, true), Switched::Suggestion);
    let (off, version, _) = read_rule(&h, "Answer in Japanese.");
    assert_eq!(switched(&h, &off, &version, true), Switched::Already);
    let (on, _, _) = read_rule(&h, "Write the changelog.");
    assert_eq!(switched(&h, &on, &"0".repeat(64), true), Switched::Stale);
    let none = format!("u:{}", "0".repeat(64));
    assert_eq!(switched(&h, &none, &version, true), Switched::NotFound);
    assert_eq!(corrections(&h), before);
}

/// The Undo of a switch posts the version that switch left: once the rule was switched on and off
/// again since, it is refused and records nothing, though the rule is off as that switch left it
/// and the Undo would change it.
#[test]
fn an_undo_after_the_rule_was_switched_again_is_refused() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (id, v0, _) = read_rule(&h, "Write the changelog.");
    let a = left(switched(&h, &id, &v0, true));
    let b = left(switched(&h, &id, &a, false));
    let c = left(switched(&h, &id, &b, true));
    assert_ne!(a, c);
    let before = corrections(&h);
    assert_eq!(switched(&h, &id, &a, false), Switched::Stale);
    assert_eq!(corrections(&h), before);
}

/// A correction of the rule's claim appended by another writer between the switch's check and its
/// append moves its fence: nothing more is recorded. An exclusion op does too: this device's own
/// exclusion waits for the readers' hold the switch keeps and is refused busy, and the fence
/// refuses one appended there without that hold as well.
#[test]
fn a_correction_or_an_exclusion_between_the_check_and_the_append_refuses_the_switch() {
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let uid = crate::claims::owned(&crate::knowledge::open(h.home.path()).unwrap(), &repo)
        .unwrap()
        .remove(0)
        .uid;
    let before = corrections(&h);
    let (home, named) = (h.home.path().to_owned(), uid.clone());
    CHECKED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || muted_elsewhere(&home, &named, true)))
    });
    assert_eq!(switched(&h, &id, &version, true), Switched::Behind);
    assert_eq!(corrections(&h), before + 1, "the other writer's alone");
    h.run();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let home = h.home.path().to_owned();
    CHECKED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut raw = crate::raw::open(&home).unwrap();
            let busy = raw.exclude("github.com/o/elsewhere", false).unwrap_err();
            assert!(busy.to_string().contains("busy"), "{busy:#}");
            let op = json!({"repo": "github.com/o/elsewhere", "undo": false});
            raw.append_ops(&[(OpKind::Exclusion, op)]).unwrap();
        }))
    });
    assert_eq!(switched(&h, &id, &version, false), Switched::Behind);
    assert_eq!(corrections(&h), before + 1);
}

/// A claim op of another claim between the check and the append, as curation appends them all day,
/// leaves the switch its append.
#[test]
fn another_claim_between_the_check_and_the_append_leaves_the_switch_its_append() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let home = h.home.path().to_owned();
    CHECKED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let op = json!({"curated": "another claim"});
            crate::raw::open(&home)
                .unwrap()
                .append_ops(&[(OpKind::Claim, op)])
                .unwrap();
        }))
    });
    assert!(matches!(
        switched(&h, &id, &version, true),
        Switched::Applied(Some(_))
    ));
}

/// A correction of the rule appended once the switch's correction is, before its receipt reads the
/// rule, leaves its answer no version: an Undo of it would undo that one too. Its body, or a mute
/// as the switch's.
#[test]
fn a_correction_after_the_switchs_own_leaves_its_answer_no_version() {
    for body in [true, false] {
        let (h, _now, _place, repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Write the changelog.");
        let k = crate::knowledge::open(h.home.path()).unwrap();
        let uid = crate::claims::owned(&k, &repo).unwrap().remove(0).uid;
        let home = h.home.path().to_owned();
        SWITCHED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if body {
                    crate::claims::correct(&home, &uid, None, Some("Write the changelog first."))
                        .unwrap();
                } else {
                    crate::claims::mute(&home, &uid, true).unwrap();
                }
            }))
        });
        assert_eq!(
            switched(&h, &id, &version, true),
            Switched::Applied(None),
            "body: {body}"
        );
    }
}

/// A correction the claims consumer has not applied when the wait runs out is saved, and its
/// answer says it is not applied yet: no version, so no Undo.
#[test]
fn a_switch_not_applied_in_time_is_recorded_and_pending() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let before = corrections(&h);
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = switched(&h, &id, &version, true);
    crate::claims::WAIT.with(|wait| wait.set(None));
    assert_eq!(got, Switched::Pending);
    assert_eq!(corrections(&h), before + 1);
}

/// A claim op of this device naming the rule's claim that the claims consumer has not applied
/// refuses the switch, which would read the rule as it was before it.
#[test]
fn a_correction_not_applied_yet_refuses_the_switch() {
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let k = crate::knowledge::open(h.home.path()).unwrap();
    let uid = crate::claims::owned(&k, &repo).unwrap().remove(0).uid;
    // Not applied, and not waited for: the wait has no time.
    muted_elsewhere(h.home.path(), &uid, false);
    let before = corrections(&h);
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = switched(&h, &id, &version, true);
    crate::claims::WAIT.with(|wait| wait.set(None));
    assert_eq!(got, Switched::Behind);
    assert_eq!(corrections(&h), before);
}

/// The claim op `pref add` wrote for `text`, as a purge writes it again from its directive's masked
/// text (`masked_preferences`): the same evidence, so the same uid, which its body does not name.
fn preference_again(h: &Store, uid: &str, text: &str) -> Value {
    let device = h.raw.device().to_owned();
    let ops = h.raw.ops_after(&device, 0, 100_000).unwrap();
    let op = (ops.into_iter())
        .find(|op| {
            op.kind == OpKind::Claim
                && op.body["recipe"] == crate::claims::PREF_RECIPE
                && op.body["body"] == text
        })
        .unwrap();
    assert_eq!(crate::claims::op_uid(&op.body).as_deref(), Some(uid));
    assert!(
        !op.body.to_string().contains(uid),
        "its body does not name it"
    );
    op.body
}

/// The uid of the rule the owner wrote for the repository of `ruled`.
fn changelog_uid(h: &Store, repo: &str) -> String {
    let k = crate::knowledge::open(h.home.path()).unwrap();
    crate::claims::owned(&k, repo).unwrap().remove(0).uid
}

/// #1128 item 1: a claim op of the rule's claim that the claims consumer has not applied refuses
/// the switch though its body does not name the uid, as a purge's preference op does not: the
/// switch would read the rule as it was before it.
#[test]
fn a_rederivation_of_the_rules_claim_not_applied_yet_refuses_the_switch() {
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let op = preference_again(&h, &changelog_uid(&h, &repo), "Write the changelog.");
    crate::raw::open(h.home.path())
        .unwrap()
        .append_ops(&[(OpKind::Claim, op)])
        .unwrap();
    let before = corrections(&h);
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = switched(&h, &id, &version, true);
    crate::claims::WAIT.with(|wait| wait.set(None));
    assert_eq!(got, Switched::Behind);
    assert_eq!(corrections(&h), before);
}

/// #1128 item 1: between the switch's check and its append, a claim op of the rule's claim whose
/// body does not name its uid refuses it, and so does a claim superseding it, which names it in
/// `supersedes` only.
#[test]
fn a_rederivation_or_a_superseding_claim_between_the_check_and_the_append_refuses_the_switch() {
    for superseding in [false, true] {
        let (h, _now, _place, repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Write the changelog.");
        let uid = changelog_uid(&h, &repo);
        let op = if superseding {
            json!({"supersedes": [uid], "curated": "a claim superseding it"})
        } else {
            preference_again(&h, &uid, "Write the changelog.")
        };
        let home = h.home.path().to_owned();
        CHECKED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                (crate::raw::open(&home).unwrap())
                    .append_ops(&[(OpKind::Claim, op)])
                    .unwrap();
            }))
        });
        let before = corrections(&h);
        let got = switched(&h, &id, &version, true);
        assert_eq!(got, Switched::Behind, "superseding: {superseding}");
        assert_eq!(corrections(&h), before);
    }
}

/// #1128 item 2: a rule whose approval is recorded and not applied yet (its preference applied, its
/// decision not) reads as the owner's own until the decision is: a switch of it then is refused.
#[test]
fn a_switch_while_an_approval_of_its_rule_is_not_applied_is_refused() {
    let (h, _now, _place, _repo, _off) = ruled();
    let home = h.home.path();
    let uid = uid_of(crate::claims::pref_add_recorded(home, "Keep it short.", None).unwrap());
    let op = MyWorkOp {
        job: "rule".into(),
        about: "s1".into(),
        entry: String::new(),
        ts: crate::db::now_ms(),
        read: Read::default(),
        removed: vec![],
        fields: [("action", json!("approve")), ("claim", json!(uid))]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    (crate::raw::open(home).unwrap())
        .append_ops(&[(OpKind::MyWork, op)])
        .unwrap();
    let (id, version, _) = read_rule(&h, "Keep it short.");
    assert!(id.starts_with("u:"), "{id}");
    let before = corrections(&h);
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = switched(&h, &id, &version, true);
    crate::claims::WAIT.with(|wait| wait.set(None));
    assert_eq!(got, Switched::Behind);
    assert_eq!(corrections(&h), before);
}

/// A rule decision of `about` with `fields`, appended on a raw.db handle of its own and not applied:
/// as the settings page's approval leaves it until the my_work consumer runs.
fn decision_waiting(home: &Path, about: &str, fields: Value) {
    decision_at(home, about, fields, crate::db::now_ms());
}

/// `decision_waiting` with its time `ts`.
fn decision_at(home: &Path, about: &str, fields: Value, ts: i64) {
    let op = MyWorkOp {
        job: crate::my_work_rules::RULE.into(),
        about: about.into(),
        entry: String::new(),
        ts,
        read: Read::default(),
        removed: vec![],
        fields: fields.as_object().unwrap().clone().into_iter().collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    (crate::raw::open(home).unwrap())
        .append_ops(&[(OpKind::MyWork, op)])
        .unwrap();
}

/// A mute of claim `uid` that a worker of an older oboete passed: in `claim_skips`, the claims
/// checkpoint past it. `keepable`: one today's consumer keeps; else one that corrects nothing.
fn passed_mute(h: &Store, uid: &str, keepable: bool) {
    let k = crate::knowledge::open(h.home.path()).unwrap();
    let mut op = crate::claims::mute_op(&k, uid, false).unwrap().unwrap();
    if !keepable {
        op.as_object_mut().unwrap().remove("muted");
    }
    let mut raw = crate::raw::open(h.home.path()).unwrap();
    let seq = raw.append_ops(&[(OpKind::Correction, op)]).unwrap()[0];
    let device = raw.device().to_owned();
    k.execute(
        "INSERT INTO claim_skips(op_device, op_seq, reason) VALUES(?1, ?2, 'corrects nothing')",
        rusqlite::params![device, seq],
    )
    .unwrap();
    let ops = crate::knowledge::checkpoint::OPS;
    crate::knowledge::checkpoint::set_in(&k, ops, "claims", &device, seq).unwrap();
}

/// `switched` with `claims::applied`'s wait set to none.
fn switched_at_once(h: &Store, reference: &str, version: &str, muted: bool) -> Switched {
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = switched(h, reference, version, muted);
    crate::claims::WAIT.with(|wait| wait.set(None));
    got
}

/// #1128 item 13: a correction of the rule's claim that a worker of an older oboete passed (in
/// `claim_skips`, the claims checkpoint past it) refuses the switch while today's consumer would
/// keep it, which its next step does; one no consumer can keep leaves the switch its append.
#[test]
fn a_correction_an_older_worker_passed_refuses_the_switch_while_it_can_be_kept() {
    for keepable in [true, false] {
        let (h, _now, _place, repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Write the changelog.");
        passed_mute(&h, &changelog_uid(&h, &repo), keepable);
        let before = corrections(&h);
        let got = switched_at_once(&h, &id, &version, true);
        if keepable {
            assert_eq!(got, Switched::Behind);
            assert_eq!(corrections(&h), before);
        } else {
            assert_eq!(got, Switched::Pending, "the switch is recorded");
            assert_eq!(corrections(&h), before + 1);
        }
    }
}

/// #1130's review: a switch behind an approval of its rule not applied yet, or behind a correction
/// an older worker passed, catches up by itself where this process runs its own passes, with no
/// other activity: its catch-up applies what the switch waits on, so the reference the page read
/// is gone or its version stale, and the next read's switches the rule.
#[test]
fn a_switch_behind_an_approval_or_a_passed_correction_catches_up_by_itself() {
    let (h, _now, _place, _repo, _off) = ruled();
    let home = h.home.path();
    let uid = uid_of(crate::claims::pref_add_recorded(home, "Keep it short.", None).unwrap());
    decision_waiting(home, "s1", json!({"action": "approve", "claim": uid}));
    let (id, version, _) = read_rule(&h, "Keep it short.");
    let first = switched(&h, &id, &version, true);
    assert!(
        matches!(first, Switched::NotFound | Switched::Stale),
        "{first:?}"
    );
    let (id, version, _) = read_rule(&h, "Keep it short.");
    assert!(id.starts_with("l:"), "{id}");
    assert!(matches!(
        switched(&h, &id, &version, true),
        Switched::Applied(Some(_))
    ));
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    passed_mute(&h, &changelog_uid(&h, &repo), true);
    assert_eq!(switched(&h, &id, &version, true), Switched::Stale);
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    assert!(matches!(
        switched(&h, &id, &version, true),
        Switched::Applied(Some(_))
    ));
}

/// #1128 item 2: an undo of an approved rule's suggestion not applied yet refuses its switch, the
/// suggestion compared whole: one of another suggestion whose id begins with it leaves the switch
/// its append.
#[test]
fn a_decision_of_an_approved_rules_suggestion_not_applied_yet_refuses_its_switch() {
    for (about, refuses) in [("s2", true), ("s22", false)] {
        let (h, _now, _place, _repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Run the tests before a push.");
        decision_waiting(h.home.path(), about, json!({"action": "undo"}));
        let before = corrections(&h);
        let got = switched_at_once(&h, &id, &version, true);
        assert_eq!(got == Switched::Behind, refuses, "{about}: {got:?}");
        assert_eq!(corrections(&h), before + usize::from(!refuses), "{about}");
    }
}

/// #1128 item 1: a claim of another uid whose text quotes the rule's uid names no claim of it: not
/// applied yet, or appended between the switch's check and its append, it leaves the switch its
/// append.
#[test]
fn a_claim_that_only_quotes_the_rules_uid_leaves_the_switch_its_append() {
    for between in [false, true] {
        let (h, _now, _place, repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Write the changelog.");
        let uid = changelog_uid(&h, &repo);
        let mut op = preference_again(&h, &uid, "Write the changelog.");
        let quoting = format!("Quoting {uid}.");
        op["body"] = json!(quoting);
        op["evidence"][0]["quote"] = json!(quoting);
        op["evidence"][0]["seq"] = json!(op["evidence"][0]["seq"].as_i64().unwrap() + 100_000);
        assert_ne!(crate::claims::op_uid(&op).as_deref(), Some(uid.as_str()));
        let home = h.home.path().to_owned();
        let append = move || {
            (crate::raw::open(&home).unwrap())
                .append_ops(&[(OpKind::Claim, op)])
                .unwrap();
        };
        if between {
            CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(append)));
        } else {
            append();
        }
        let before = corrections(&h);
        let got = switched_at_once(&h, &id, &version, true);
        assert_eq!(got, Switched::Pending, "between: {between}");
        assert_eq!(corrections(&h), before + 1, "between: {between}");
    }
}

/// A correction of the rule's claim recorded just before the switch, the claims consumer given its
/// time, is waited for and applied first (`caught_up`): the switch is judged against it, so the
/// version the pages read is stale and records nothing, and the one the next read gives switches the
/// rule.
#[test]
fn a_switch_just_after_a_correction_of_its_rule_is_judged_against_it() {
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let k = crate::knowledge::open(h.home.path()).unwrap();
    let uid = crate::claims::owned(&k, &repo).unwrap().remove(0).uid;
    muted_elsewhere(h.home.path(), &uid, false);
    let before = corrections(&h);
    assert_eq!(switched(&h, &id, &version, true), Switched::Stale);
    assert_eq!(corrections(&h), before, "the stale switch records nothing");
    let (again, now, _) = read_rule(&h, "Write the changelog.");
    assert_eq!(again, id);
    assert_ne!(now, version, "the correction moved the rule's version");
    left(switched(&h, &id, &now, true));
}

/// A correction the claims consumer applies while the snapshot reads My Rules, between a rule's
/// text and what decides it, reaches neither (#1083, R2a: one read of knowledge.db): the version
/// the snapshot gives goes with the text it shows, so a switch at it is stale once the correction
/// is applied, and the next read gives the corrected text at a version of its own.
#[test]
fn a_correction_applied_while_my_rules_is_read_reaches_neither_its_text_nor_its_version() {
    let (h, now, _place, _repo, off) = ruled();
    let (home, uid) = (h.home.path().to_owned(), off.clone());
    crate::my_work_rules::READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let body = Some("Answer in Japanese, briefly.");
            let receipt = crate::claims::correct_recorded(&home, &uid, None, body).unwrap();
            // #1128 item 11: the correction the test reads around is applied.
            assert!(
                matches!(receipt, crate::claims::OwnerReceipt::Applied { .. }),
                "the hook's correction is applied"
            );
        }))
    });
    let shown = answer(h.home.path(), now).unwrap();
    assert!(
        crate::my_work_rules::READ.with(|hook| hook.borrow().is_none()),
        "the hook ran"
    );
    let rule = rule_of(&shown, "Answer in Japanese.");
    let id = rule["ruleId"].as_str().unwrap().to_owned();
    let version = rule["ruleVersion"].as_str().unwrap().to_owned();
    let again = answer(h.home.path(), now).unwrap();
    let corrected = rule_of(&again, "Answer in Japanese, briefly.");
    assert_eq!(corrected["ruleId"], json!(id));
    assert_ne!(
        corrected["ruleVersion"],
        json!(version),
        "the text shown went with the version from before the correction"
    );
    assert_eq!(switched(&h, &id, &version, false), Switched::Stale);
}

/// Two rows of one reference, which only two keys of one keyed hash would make: the switch tells
/// neither.
#[test]
fn two_rows_of_one_reference_are_neither() {
    let row = |uid: &str| Row {
        reference: "u:same".into(),
        uid: Some(uid.into()),
        on: Some(true),
        version: "v".into(),
        of: Value::Null,
        kept: None,
    };
    assert_eq!(
        the_one(vec![row("a"), row("b")], "u:same").err(),
        Some(Switched::Ambiguous)
    );
    assert_eq!(
        the_one(vec![row("a")], "u:other").err(),
        Some(Switched::NotFound)
    );
    assert_eq!(
        the_one(vec![row("a")], "u:same").map(|r| r.uid),
        Ok(Some("a".into()))
    );
}

/// `decide` on `h`'s home, its token asked and taken.
fn decided(h: &Store, reference: &str, version: &str, what: Decision) -> Switched {
    let saving = std::sync::Mutex::new(());
    let take = || saving.lock().ok();
    decide(h.home.path(), (reference, version), what, &take, &|| true).unwrap()
}

/// `decided` with `claims::applied`'s wait set to none.
fn decided_at_once(h: &Store, reference: &str, version: &str, what: Decision) -> Switched {
    crate::claims::WAIT.with(|wait| wait.set(Some(std::time::Duration::ZERO)));
    let got = decided(h, reference, version, what);
    crate::claims::WAIT.with(|wait| wait.set(None));
    got
}

/// #1083's R2b: the row of suggestion `id` as the writes find it now: its reference, its version and
/// what it was read from.
fn row_of(h: &Store, id: &str) -> (String, String, Value) {
    let home = h.home.path();
    let settings = crate::capture::Settings::load(home).unwrap();
    let (raw, k) = crate::search::b::stores(home).unwrap().unwrap();
    let found = my_rules_on(home, crate::db::now_ms(), &settings, &raw, &k).unwrap();
    let row = (found.rows.into_iter()).find(|r| r.of["suggestion"] == id);
    let row = row.unwrap_or_else(|| panic!("no row of {id}"));
    (row.reference, row.version, row.of)
}

/// The rule decisions of suggestion `about` raw.db holds.
fn decisions_of(h: &Store, about: &str) -> usize {
    let device = h.raw.device().to_owned();
    let ops = h.raw.ops_after(&device, 0, 100_000).unwrap();
    let rule = crate::my_work_rules::RULE;
    (ops.iter())
        .filter(|op| op.kind == OpKind::MyWork)
        .filter(|op| op.body["job"] == rule && op.body["about"] == about)
        .count()
}

/// Whether the snapshot read now lists a rule whose `then` is `then`.
fn listed(h: &Store, then: &str) -> bool {
    let now = crate::db::now_ms();
    let (work, goals) = answers(h, now);
    let snapshot = snapshot_at(&work, &goals, now, &Utc, &Gate::now());
    arr(&snapshot["rules"]).iter().any(|r| r["then"] == then)
}

/// A rules op of suggestion `id`, as `suggestion_op` makes one, that read record spans `spans`.
fn learned_op(id: &str, statement: &str, at: i64, spans: Vec<(i64, i64)>) -> Value {
    let fields = json!({"statement": statement, "seen": 2,
        "requests": [format!("g:{id}0"), format!("g:{id}1")],
        "quotes": [{"text": format!("Quoted for {id}."), "request": format!("g:{id}0"), "at": at}]});
    let op = MyWorkOp {
        job: crate::my_work_rules::NAME.into(),
        about: id.into(),
        entry: "fake".into(),
        ts: crate::db::now_ms(),
        read: Read { spans, ops: vec![] },
        removed: vec![],
        fields: fields.as_object().unwrap().clone().into_iter().collect(),
        skipped: false,
        forgotten: None,
    };
    serde_json::to_value(op).unwrap()
}

/// A review op of request `about` (`my_work_progress::NAME`) that read `read`.
fn review_op(about: &str, read: Read) -> Value {
    let op = MyWorkOp {
        job: crate::my_work_progress::NAME.into(),
        about: about.into(),
        entry: "fake".into(),
        ts: crate::db::now_ms(),
        read,
        removed: vec![],
        fields: Default::default(),
        skipped: false,
        forgotten: None,
    };
    serde_json::to_value(op).unwrap()
}

/// The batches of the newest correction op and of the newest rule decision op `h`'s raw.db holds.
fn rule_batches(h: &Store) -> [i64; 2] {
    let c = rusqlite::Connection::open(h.home.path().join("raw.db")).unwrap();
    let newest = |filter: &str| -> i64 {
        let sql = format!("SELECT batch FROM ops WHERE {filter} ORDER BY op_seq DESC LIMIT 1");
        c.query_row(&sql, [], |r| r.get(0)).unwrap()
    };
    [
        newest("type = 'correction'"),
        newest("type = 'my_work' AND json_extract(body, '$.job') = 'rule'"),
    ]
}

/// A prompt of `session` in `repo`, recorded on a raw.db handle of its own, as a hook records one.
fn recorded(home: &Path, session: &str, repo: &str) {
    let e = Event {
        kind: "prompt".into(),
        session: session.into(),
        repo: Some(repo.into()),
        ts: crate::db::now_ms(),
        ..crate::raw::test_event(&json!({"prompt": "More."}).to_string())
    };
    crate::raw::open(home).unwrap().append(&e).unwrap();
}

/// #1083's R2b: a suggestion waiting is dismissed by its reference at its version; its row is then a
/// rejected one, which the writes find and the snapshot does not send yet, at the version the
/// dismissal answered. Its rejection undone by that version brings it back to waiting by the same
/// reference, at a version no read gave it before, so the first read's is stale.
#[test]
fn a_dismissal_and_its_undo_go_by_the_versions_each_answers() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (reference, waiting, of) = row_of(&h, "s1");
    assert_eq!(of["rule"], "waiting");
    let rejected = left(decided(&h, &reference, &waiting, Decision::Dismiss));
    let (again, version, of) = row_of(&h, "s1");
    assert_eq!(
        (again.as_str(), of["rule"].as_str()),
        (reference.as_str(), Some("rejected"))
    );
    assert_eq!(version, rejected);
    assert!(!listed(&h, "Keep commits small."));
    let restored = left(decided(&h, &reference, &rejected, Decision::Restore));
    let (again, version, of) = row_of(&h, "s1");
    assert_eq!(
        (again.as_str(), of["rule"].as_str()),
        (reference.as_str(), Some("waiting"))
    );
    assert_eq!(version, restored);
    assert_ne!(restored, waiting, "a version the first read never gave");
    assert!(listed(&h, "Keep commits small."));
    assert_eq!(
        decided(&h, &reference, &waiting, Decision::Dismiss),
        Switched::Stale
    );
}

/// #1083's R2b: a decision takes only the rows it decides: an approval and a dismissal a suggestion
/// waiting, an undo a rejected or an approved one; a version not the row's is stale. None records
/// anything.
#[test]
fn a_decision_takes_only_the_rows_it_decides() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (waiting, at_waiting, _) = row_of(&h, "s1");
    let (approved, at_approved, _) = row_of(&h, "s2");
    let (rejected, at_rejected, _) = row_of(&h, "s3");
    let counts = || ["s1", "s2", "s3"].map(|s| decisions_of(&h, s));
    let before = counts();
    let refused = |got: Switched| matches!(got, Switched::State(_));
    for (reference, version) in [(&rejected, &at_rejected), (&approved, &at_approved)] {
        assert!(refused(decided(&h, reference, version, Decision::Approve)));
    }
    assert!(refused(decided(
        &h,
        &rejected,
        &at_rejected,
        Decision::Dismiss
    )));
    assert!(refused(decided(
        &h,
        &approved,
        &at_approved,
        Decision::Dismiss
    )));
    assert!(refused(decided(
        &h,
        &waiting,
        &at_waiting,
        Decision::Restore
    )));
    let stale = decided(&h, &waiting, &at_rejected, Decision::Dismiss);
    assert_eq!(stale, Switched::Stale);
    assert_eq!(counts(), before);
}

/// #1083's R2b: a decision of the suggestion recorded and not applied yet refuses its dismissal, and
/// so does one appended between the check and the append; nothing is recorded. Another
/// suggestion's, appended between them, leaves the dismissal its append.
#[test]
fn a_decision_of_the_suggestion_not_applied_or_since_the_check_refuses_its_dismissal() {
    for (about, between) in [("s1", false), ("s1", true), ("s3", true)] {
        let (h, _now, _place, _repo, _off) = ruled();
        let (reference, version, _) = row_of(&h, "s1");
        let home = h.home.path().to_owned();
        let append = move || decision_waiting(&home, about, json!({"action": "undo"}));
        if between {
            CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(append)));
        } else {
            append();
        }
        let before = decisions_of(&h, "s1");
        let got = decided_at_once(&h, &reference, &version, Decision::Dismiss);
        if about == "s1" {
            assert_eq!(got, Switched::Behind, "between: {between}");
            assert_eq!(decisions_of(&h, "s1"), before + usize::from(between));
        } else {
            assert_eq!(got, Switched::Pending, "another suggestion's");
            assert_eq!(decisions_of(&h, "s1"), before + 1);
        }
    }
}

/// #1083's R2b: a rules job's op of the suggestion, not applied yet or appended between the check
/// and the append, refuses its dismissal: it may change what the row reads.
#[test]
fn a_learning_op_of_the_suggestion_refuses_its_dismissal() {
    for between in [false, true] {
        let (h, now, _place, _repo, _off) = ruled();
        let (reference, version, _) = row_of(&h, "s1");
        let home = h.home.path().to_owned();
        let learn = move || {
            let op = learned_op("s1", "Keep commits smaller.", now, vec![]);
            (crate::raw::open(&home).unwrap())
                .append_ops(&[(OpKind::MyWork, op)])
                .unwrap();
        };
        if between {
            CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(learn)));
        } else {
            learn();
        }
        let got = decided_at_once(&h, &reference, &version, Decision::Dismiss);
        assert_eq!(got, Switched::Behind, "between: {between}");
        assert_eq!(decisions_of(&h, "s1"), 0, "between: {between}");
    }
}

/// #1083's R2b: a suggestion's write is refused, and writes nothing, when a session it was learned
/// from records an event in a repository the exclusion list holds between the check and the append
/// (a session that touches one is excluded, and what was learned from it with it), the session read
/// by the learning op itself or, as the rules job reads in production, by a review it took, from
/// records or a session's summary; so does any other session's first event in an excluded
/// repository (#1132's review: the write checks who first touches one, not the suggestion's
/// sources), right after the fence too, and in an excluded repository with no event yet; a retry is
/// not refused for that session again. The same session's record in another repository leaves the
/// write its append, as does a session excluded before the read recording again, a source among them
/// whose one record in what was learned is its start (which the source walk this check replaced
/// refused), and 64 records since the check; more than the write reads refuse it, but not while no
/// repository is excluded.
#[test]
fn a_source_sessions_record_in_an_excluded_repository_refuses_a_suggestions_write() {
    let cases = [
        "excluded",
        "elsewhere",
        "many",
        "sixty-four",
        "through a review",
        "through a turn",
        "another session's first event",
        "a session excluded before the read",
        "a first event right after the fence",
        "nothing excluded, 65 records",
        "an excluded repository with no event yet",
        "a source session excluded before the read",
        "another device's first event right after the fence",
    ];
    for case in cases {
        let mut h = Store::new();
        let now = crate::db::now_ms();
        let place = tempfile::tempdir().unwrap();
        let open = started_in(
            &mut h,
            &place.path().join("open"),
            "learned",
            now - 5 * 60_000,
        );
        let secret = started_in(
            &mut h,
            &place.path().join("secret"),
            "other",
            now - 5 * 60_000,
        );
        // A source whose one record in what was learned is its start, in a repository excluded
        // before the read: the read does not hide the suggestion, a start being no line.
        let start = (case == "a source session excluded before the read").then(|| {
            let e = Event {
                kind: "start".into(),
                session: "lifecycle".into(),
                repo: Some(secret.clone()),
                ts: now - 4 * 60_000,
                ..crate::raw::test_event("{}")
            };
            h.raw.append(&e).unwrap()
        });
        let seq = h.said("learned", &open, now - 4 * 60_000, "Keep commits small.");
        let at = now - 4 * 60_000;
        // As the rules job reads in production: the reviews it took, by their ops, and what each
        // review read, records or a session's summary.
        let op = match case {
            "through a review" | "through a turn" => {
                let read = if case == "through a review" {
                    Read {
                        spans: vec![(seq, seq)],
                        ops: vec![],
                    }
                } else {
                    let turn = h.turn(json!({"agent": "claude", "session": "learned",
                        "repo": &open, "ts": at, "from": seq, "through": seq, "read": [],
                        "goals": [], "removed": [], "fields": {}, "skipped": false}));
                    Read {
                        spans: vec![],
                        ops: vec![turn[1..].parse().unwrap()],
                    }
                };
                let review = review_op("g:s10", read);
                let review = h.raw.append_ops(&[(OpKind::MyWork, review)]).unwrap()[0];
                let mut op = learned_op("s1", "Keep commits small.", at, vec![]);
                op["read"]["ops"] = json!([review]);
                op
            }
            _ => learned_op(
                "s1",
                "Keep commits small.",
                at,
                vec![(start.unwrap_or(seq), seq)],
            ),
        };
        h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        let empty = "github.com/x/empty";
        match case {
            "nothing excluded, 65 records" => {}
            "an excluded repository with no event yet" => {
                h.raw.exclude(empty, false).unwrap();
            }
            _ => {
                h.raw.exclude(&secret, false).unwrap();
            }
        }
        h.run();
        let (reference, version, _) = row_of(&h, "s1");
        let home = h.home.path().to_owned();
        let record = move || match case {
            "elsewhere" => recorded(&home, "learned", &open),
            "many" => (0..65).for_each(|_| recorded(&home, "third", &open)),
            "sixty-four" => (0..64).for_each(|_| recorded(&home, "third", &open)),
            "another session's first event" | "a first event right after the fence" => {
                recorded(&home, "newcomer", &secret)
            }
            "a session excluded before the read" => recorded(&home, "other", &secret),
            "a source session excluded before the read" => recorded(&home, "lifecycle", &secret),
            "another device's first event right after the fence" => {
                let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
                raw.execute(
                    "INSERT INTO records(device, seq, type, ts, kind, agent, session, repo,
                       source, body)
                     VALUES('other-device', 1, 'event', 0, 'prompt', 'claude', 'newcomer', ?1,
                       'hook', '{}')",
                    [&secret],
                )
                .unwrap();
            }
            "nothing excluded, 65 records" => (0..65).for_each(|_| recorded(&home, "third", &open)),
            "an excluded repository with no event yet" => recorded(&home, "newcomer", empty),
            _ => recorded(&home, "learned", &secret),
        };
        if case.ends_with("right after the fence") {
            FENCED.with(|hook| *hook.borrow_mut() = Some(Box::new(record)));
        } else {
            CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(record)));
        }
        let got = decided_at_once(&h, &reference, &version, Decision::Dismiss);
        let through = [
            "elsewhere",
            "sixty-four",
            "a session excluded before the read",
            "a source session excluded before the read",
            "nothing excluded, 65 records",
        ];
        if through.contains(&case) {
            assert_eq!(got, Switched::Pending, "{case}");
            assert_eq!(decisions_of(&h, "s1"), 1, "{case}");
        } else {
            assert_eq!(got, Switched::Behind, "{case}");
            assert_eq!(decisions_of(&h, "s1"), 0, "{case}");
        }
        if matches!(case, "excluded" | "through a review" | "through a turn") {
            // #1131 item 18: retried, My Rules hides the suggestion through what it was learned
            // from, a review and a turn included, so the retry finds no row.
            let again = decided_at_once(&h, &reference, &version, Decision::Dismiss);
            assert_eq!(again, Switched::NotFound, "{case}: the retry");
            assert_eq!(decisions_of(&h, "s1"), 0, "{case}: nothing written");
        }
        if case == "another session's first event" {
            // A retry, under a fence of its own, is not refused for that session again.
            let again = decided_at_once(&h, &reference, &version, Decision::Dismiss);
            assert_eq!(again, Switched::Pending, "{case}: the retry");
            assert_eq!(decisions_of(&h, "s1"), 1, "{case}: the retry is saved");
        }
    }
}

/// #1131 item 10: a decision is refused, nothing written, when a session's first event since its read
/// comes under a label first recorded then that a repository never recorded may be (a mask in it):
/// the write reads that label as its read reads one. The event itself is stored, under that label.
#[test]
fn a_label_a_never_recorded_repository_may_be_refuses_a_decision() {
    let (h, _now, _place, _repo, _off) = ruled();
    (crate::raw::open(h.home.path()).unwrap())
        .exclude_capture(("repo", "github.com/x/private-aaa"), false)
        .unwrap();
    let (reference, version, _) = row_of(&h, "s1");
    let (records, decisions) = (stored(&h).0, decisions_of(&h, "s1"));
    let home = h.home.path().to_owned();
    let masked = format!("github.com/x/private-{}", crate::redact::MASK);
    CHECKED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || recorded(&home, "newcomer", &masked)))
    });
    let got = decided_at_once(&h, &reference, &version, Decision::Dismiss);
    assert_eq!(got, Switched::Behind);
    assert_eq!(decisions_of(&h, "s1"), decisions, "nothing written");
    assert_eq!(stored(&h).0, records + 1, "the hook's event is recorded");
}

/// #1131 item 19: a decision refused as pending reads no exclusion baseline: what is not applied
/// yet is checked first, while a repository is excluded.
#[test]
fn a_decision_refused_as_pending_reads_no_exclusion_baseline() {
    let (h, now, _place, _repo, _off) = ruled();
    (crate::raw::open(h.home.path()).unwrap())
        .exclude("github.com/x/elsewhere", false)
        .unwrap();
    let (reference, version, _) = row_of(&h, "s1");
    let op = learned_op("s1", "Keep commits smaller.", now, vec![]);
    (crate::raw::open(h.home.path()).unwrap())
        .append_ops(&[(OpKind::MyWork, op)])
        .unwrap();
    let reads = || crate::raw::BASELINE_QUERIES.with(|n| n.get());
    let (before, decisions) = (reads(), decisions_of(&h, "s1"));
    let got = decided_at_once(&h, &reference, &version, Decision::Dismiss);
    assert_eq!(got, Switched::Behind);
    assert_eq!(reads(), before, "no baseline read");
    assert_eq!(decisions_of(&h, "s1"), decisions, "nothing written");
}

/// #1131 items 22 and 23: a receipt whose read after the write fails answers saved with no
/// version, and writes `RECEIPT_READ_FAILED` and a newline on the process's stderr, read here from
/// a child run of this test; a read that succeeds writes nothing there, and a stderr that takes no
/// write leaves the answer as it is.
#[test]
fn a_receipts_failed_read_is_told_on_stderr() {
    const NAME: &str = "work_view::tests::a_receipts_failed_read_is_told_on_stderr";
    // This test's own: a child runs the case it names (`fail`, `ok`, `closed`).
    const CHILD: &str = "OBOETE_RECEIPT_READ_CHILD";
    if let Ok(case) = std::env::var(CHILD) {
        let (h, _now, _place, _repo, _off) = ruled();
        let (id, version, _) = read_rule(&h, "Write the changelog.");
        if case == "closed" {
            use std::io::Write;
            assert!(
                writeln!(std::io::stderr(), "a write no one reads").is_err(),
                "a stderr whose reader is gone takes no write"
            );
        }
        FRESH_FAILS.with(|fails| fails.set(case != "ok"));
        println!("ANSWER {:?}", switched(&h, &id, &version, true));
        return;
    }
    let run = |case: &str, stderr: std::process::Stdio| {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD, case)
            .stdout(std::process::Stdio::piped())
            .stderr(stderr)
            .output()
            .unwrap();
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert!(out.status.success(), "{case}: {stdout}\n{stderr}");
        (stdout, stderr)
    };
    let line = format!("{RECEIPT_READ_FAILED}\n");
    let (stdout, stderr) = run("fail", std::process::Stdio::piped());
    assert!(stdout.contains("ANSWER Applied(None)"), "{stdout}");
    assert!(!stdout.contains(RECEIPT_READ_FAILED), "{stdout}");
    assert_eq!(stderr.matches(&line).count(), 1, "{stderr}");
    let (stdout, stderr) = run("ok", std::process::Stdio::piped());
    assert!(stdout.contains("ANSWER Applied(Some("), "{stdout}");
    assert!(!stderr.contains(RECEIPT_READ_FAILED), "{stderr}");
    // No reader from before the child starts: each write to its stderr fails (EPIPE, or Windows'
    // ERROR_NO_DATA), unlike a read-only file, whose EBADF std's stderr turns into success.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let (stdout, _) = run("closed", writer.into());
    assert!(stdout.contains("ANSWER Applied(None)"), "{stdout}");
}

/// #1131 item 2: a suggestion's decisions go in op order: an undo appended after its rejection
/// undoes it, even when its time is older than the rejection's.
#[test]
fn a_suggestions_decisions_go_in_op_order_whatever_their_times() {
    let (h, _now, _place, _repo, _off) = ruled();
    decision_at(h.home.path(), "s3", json!({"action": "undo"}), 1);
    h.run();
    let (_, _, of) = row_of(&h, "s3");
    assert_eq!(of["rule"], "waiting");
}

/// #1083's R2b: a dismissal behind a rules job's op of its suggestion catches up by itself where
/// this process runs its own passes, with no other activity: its catch-up applies the op, so the
/// version the page read is stale, and the next read's dismisses it.
#[test]
fn a_dismissal_behind_a_learning_op_catches_up_by_itself() {
    let (h, now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let op = learned_op("s1", "Keep commits smaller.", now, vec![]);
    (crate::raw::open(h.home.path()).unwrap())
        .append_ops(&[(OpKind::MyWork, op)])
        .unwrap();
    let got = decided(&h, &reference, &version, Decision::Dismiss);
    assert_eq!(got, Switched::Stale);
    let (reference, version, _) = row_of(&h, "s1");
    let got = decided(&h, &reference, &version, Decision::Dismiss);
    assert!(matches!(got, Switched::Applied(Some(_))), "{got:?}");
}

/// #1083's R2b (#1128 item 3): a dismissal whose row moved between its append and its receipt's
/// read (a rules job's op of the suggestion applied there) is saved and answers no version: the
/// version a read gives then is not of the row the write left.
#[test]
fn a_dismissal_whose_row_moved_before_its_receipt_answers_no_version() {
    let (h, now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    SWITCHED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let op = learned_op("s1", "Keep commits smaller.", now, vec![]);
            (crate::raw::open(&home).unwrap())
                .append_ops(&[(OpKind::MyWork, op)])
                .unwrap();
            crate::worker::run_once(&home).unwrap();
        }))
    });
    let got = decided(&h, &reference, &version, Decision::Dismiss);
    assert_eq!(got, Switched::Applied(None));
    assert_eq!(row_of(&h, "s1").2["rule"], "rejected");
}

/// #1128 item 3: a switch whose rule My Rules no longer lists once its correction is applied (its
/// claim forgotten there) is saved and answers no version, though what decides its mute is still its
/// correction: the version is a read's of the rule, not its claim's provenance's.
#[test]
fn a_switch_whose_rule_is_gone_before_its_receipt_answers_no_version() {
    let (h, _now, _place, repo, _off) = ruled();
    let (id, version, _) = read_rule(&h, "Write the changelog.");
    let uid = changelog_uid(&h, &repo);
    let (home, gone) = (h.home.path().to_owned(), uid.clone());
    SWITCHED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::worker::run_once(&home).unwrap();
            (crate::raw::open(&home).unwrap())
                .append_ops(&[(OpKind::Forget, json!({"uid": gone, "job": "j"}))])
                .unwrap();
        }))
    });
    assert_eq!(switched(&h, &id, &version, true), Switched::Applied(None));
    let k = crate::knowledge::open(h.home.path()).unwrap();
    let by = crate::claims::decided_by(&k, &uid).unwrap().unwrap();
    assert_eq!(by["muted"], true, "its correction still decides its mute");
}

/// #1131 item 11: a dismissal applied and kept whose receipt reads My Rules while a later decision
/// of its suggestion, or a rules job's op of it, is recorded and not applied answers no version: the
/// row the write left is read, but the write that follows would be judged against that op.
#[test]
fn a_dismissal_whose_suggestion_moved_and_is_not_applied_answers_no_version() {
    for case in ["a later decision", "a rules job's op"] {
        let (h, now, _place, _repo, _off) = ruled();
        let (reference, version, _) = row_of(&h, "s1");
        let home = h.home.path().to_owned();
        SWITCHED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                crate::worker::run_once(&home).unwrap();
                crate::my_work_rules::READ.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        if case == "a later decision" {
                            decision_waiting(&home, "s1", json!({"action": "undo"}));
                        } else {
                            let op = learned_op("s1", "Keep commits smaller.", now, vec![]);
                            (crate::raw::open(&home).unwrap())
                                .append_ops(&[(OpKind::MyWork, op)])
                                .unwrap();
                        }
                    }))
                });
            }))
        });
        let got = decided(&h, &reference, &version, Decision::Dismiss);
        let ran = crate::my_work_rules::READ.with(|hook| hook.borrow().is_none());
        assert!(ran, "{case}: the hook ran");
        assert_eq!(got, Switched::Applied(None), "{case}");
        assert_eq!(row_of(&h, "s1").2["rule"], "rejected", "{case}: saved");
    }
}

/// #1131 item 11 (#1132's plan review): an approval applied and kept whose receipt reads My Rules
/// while a correction of its new preference is recorded and not applied answers no version: the
/// check its Undo makes goes by the preference the row the write left names, not the waiting row's.
#[test]
fn an_approval_whose_preference_has_a_correction_not_applied_answers_no_version() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    SWITCHED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::worker::run_once(&home).unwrap();
            crate::my_work_rules::READ.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    muted_elsewhere(&home, &approved_uid(&home, "s1"), true);
                }))
            });
        }))
    });
    let got = decided(&h, &reference, &version, Decision::Approve);
    let ran = crate::my_work_rules::READ.with(|hook| hook.borrow().is_none());
    assert!(ran, "the hook ran");
    assert_eq!(got, Switched::Applied(None));
    assert_eq!(row_of(&h, "s1").2["rule"], "approved", "saved");
}

/// #1131 item 11: a switch of an approved rule whose receipt reads while a rules job's op of its
/// suggestion is recorded and not applied answers its version: the switch that undoes it does not
/// read that op, and the version leaves it out, so that Undo goes through once it is applied.
#[test]
fn an_approved_rules_switch_answers_its_version_past_a_rules_jobs_op_not_applied() {
    let (h, now, _place, _repo, _off) = ruled();
    let (reference, version, of) = row_of(&h, "s2");
    assert_eq!(of["rule"], "approved");
    let home = h.home.path().to_owned();
    SWITCHED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::worker::run_once(&home).unwrap();
            crate::my_work_rules::READ.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    let op = learned_op("s2", "Run the tests before a push.", now, vec![]);
                    (crate::raw::open(&home).unwrap())
                        .append_ops(&[(OpKind::MyWork, op)])
                        .unwrap();
                }))
            });
        }))
    });
    let off = left(switched(&h, &reference, &version, true));
    let ran = crate::my_work_rules::READ.with(|hook| hook.borrow().is_none());
    assert!(ran, "the hook ran");
    h.run();
    left(switched(&h, &reference, &off, false));
}

/// #1131 item 14: a dismissal whose op the my_work consumer passed without keeping it (a worker of
/// an older oboete moved the checkpoint past it, or it was skipped) is saved and pending: the
/// checkpoint alone is no proof that it decides the row.
#[test]
fn a_dismissal_passed_but_not_kept_is_pending() {
    for skipped in [false, true] {
        let (h, _now, _place, _repo, _off) = ruled();
        let (reference, version, _) = row_of(&h, "s1");
        let before = decisions_of(&h, "s1");
        let home = h.home.path().to_owned();
        SWITCHED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let raw = crate::raw::open(&home).unwrap();
                let device = raw.device().to_owned();
                let seq = raw
                    .ops_after(&device, 0, 100_000)
                    .unwrap()
                    .last()
                    .unwrap()
                    .op_seq;
                let k = crate::knowledge::open(&home).unwrap();
                if skipped {
                    crate::worker::run_once(&home).unwrap();
                    let n = k
                        .execute(
                            "UPDATE my_work SET skipped = 1 WHERE device = ?1 AND op_seq = ?2",
                            rusqlite::params![device, seq],
                        )
                        .unwrap();
                    assert_eq!(n, 1, "the dismissal's row");
                } else {
                    let ops = crate::knowledge::checkpoint::OPS;
                    crate::knowledge::checkpoint::set_in(&k, ops, "my_work", &device, seq).unwrap();
                }
            }))
        });
        let got = decided(&h, &reference, &version, Decision::Dismiss);
        assert_eq!(got, Switched::Pending, "skipped: {skipped}");
        assert_eq!(
            decisions_of(&h, "s1"),
            before + 1,
            "skipped: {skipped}: saved"
        );
    }
}

/// raw.db's records, ledger rows and ops of `h`'s home, read on a connection of their own.
fn stored(h: &Store) -> (i64, i64, i64) {
    let raw = rusqlite::Connection::open(h.home.path().join("raw.db")).unwrap();
    raw.query_row(
        "SELECT (SELECT COUNT(*) FROM records), (SELECT COUNT(*) FROM ledger),
                (SELECT COUNT(*) FROM ops)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

/// The uid an approval of suggestion `about` named, the newest one raw.db holds.
fn approved_uid(home: &Path, about: &str) -> String {
    let raw = crate::raw::open(home).unwrap();
    let device = raw.device().to_owned();
    let ops = raw.ops_after(&device, 0, 100_000).unwrap();
    let decision = (ops.iter().rev()).find(|op| {
        op.body["job"] == crate::my_work_rules::RULE
            && op.body["about"] == about
            && op.body["fields"]["action"] == "approve"
    });
    decision.unwrap().body["fields"]["claim"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// #1083's R2b: a suggestion waiting is approved from the pages by its reference at its version, its
/// own statement as the owner's preference of every repository, as the main page's approve records
/// it: the row is then its approved rule, on, by the same reference, at the version the approval
/// answered, and the decision names the preference the row goes by.
#[test]
fn an_approval_from_the_pages_records_the_main_pages_approval() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let before = stored(&h);
    let approved = left(decided(&h, &reference, &version, Decision::Approve));
    let (again, at, of) = row_of(&h, "s1");
    assert_eq!(
        (again.as_str(), of["rule"].as_str()),
        (reference.as_str(), Some("approved"))
    );
    assert_eq!(at, approved);
    assert_eq!(of["uid"], approved_uid(h.home.path(), "s1"));
    let (_, _, rule) = read_rule(&h, "Keep commits small.");
    let fields = ["setBy", "status", "scopeId"].map(|k| rule[k].as_str().unwrap().to_owned());
    assert_eq!(fields, ["approved", "on", "everywhere"]);
    let after = stored(&h);
    assert_eq!(
        (after.0 - before.0, after.2 - before.2),
        (1, 2),
        "a directive, its two ops"
    );
    assert_eq!(decisions_of(&h, "s1"), 1);
}

/// #1083's R2b: an approval from the pages is refused, and writes nothing (no directive, ledger
/// row, preference or decision), when a decision of its suggestion is appended between the check
/// and the append, and when a session it was learned from records an event in a repository the
/// exclusion list holds there, or under a label first recorded there that a repository never
/// recorded may be (#1131 item 10).
#[test]
fn an_approval_from_the_pages_is_refused_whole_when_its_suggestion_moved() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    let decide_elsewhere = move || decision_waiting(&home, "s1", json!({"action": "reject"}));
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(decide_elsewhere)));
    let before = stored(&h);
    let got = decided_at_once(&h, &reference, &version, Decision::Approve);
    assert_eq!(got, Switched::Behind);
    assert_eq!(
        stored(&h),
        (before.0, before.1, before.2 + 1),
        "the other decision alone"
    );
    let mut h = Store::new();
    let now = crate::db::now_ms();
    let place = tempfile::tempdir().unwrap();
    let open = started_in(
        &mut h,
        &place.path().join("open"),
        "learned",
        now - 5 * 60_000,
    );
    let secret = started_in(
        &mut h,
        &place.path().join("secret"),
        "other",
        now - 5 * 60_000,
    );
    let seq = h.said("learned", &open, now - 4 * 60_000, "Keep commits small.");
    let op = learned_op(
        "s1",
        "Keep commits small.",
        now - 4 * 60_000,
        vec![(seq, seq)],
    );
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    h.raw.exclude(&secret, false).unwrap();
    h.run();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    let excluded = move || recorded(&home, "learned", &secret);
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(excluded)));
    let before = stored(&h);
    let got = decided_at_once(&h, &reference, &version, Decision::Approve);
    assert_eq!(got, Switched::Behind);
    assert_eq!(
        stored(&h),
        (before.0 + 1, before.1, before.2),
        "the hook's record alone"
    );
    // #1131 item 10: the source's first event under a label first recorded since the read, one a
    // repository never recorded may be (a mask in it).
    let mut h = Store::new();
    let now = crate::db::now_ms();
    let place = tempfile::tempdir().unwrap();
    let open = started_in(
        &mut h,
        &place.path().join("open"),
        "learned",
        now - 5 * 60_000,
    );
    let seq = h.said("learned", &open, now - 4 * 60_000, "Keep commits small.");
    let op = learned_op(
        "s1",
        "Keep commits small.",
        now - 4 * 60_000,
        vec![(seq, seq)],
    );
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    (h.raw)
        .exclude_capture(("repo", "github.com/x/private-aaa"), false)
        .unwrap();
    h.run();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    let masked = format!("github.com/x/private-{}", crate::redact::MASK);
    let excluded = move || recorded(&home, "learned", &masked);
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(excluded)));
    let before = stored(&h);
    let got = decided_at_once(&h, &reference, &version, Decision::Approve);
    assert_eq!(got, Switched::Behind, "a masked label");
    assert_eq!(
        stored(&h),
        (before.0 + 1, before.1, before.2),
        "a masked label: the hook's record alone"
    );
}

/// #1083's R2b: an approval from the pages whose preference was muted between its append and its
/// receipt's read is saved and answers no version: what decides its rule is not what it left.
#[test]
fn an_approval_whose_preference_moved_before_its_receipt_answers_no_version() {
    let (h, _now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s1");
    let home = h.home.path().to_owned();
    SWITCHED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::worker::run_once(&home).unwrap();
            muted_elsewhere(&home, &approved_uid(&home, "s1"), true);
            crate::worker::run_once(&home).unwrap();
        }))
    });
    let got = decided(&h, &reference, &version, Decision::Approve);
    assert_eq!(got, Switched::Applied(None));
    assert_eq!(row_of(&h, "s1").2["rule"], "approved");
}

/// #1083's R2b: an approved rule, on or off, is undone from the pages by its reference at its
/// version (`restore-suggestion`): its preference retracted and its approval undone in one append,
/// its suggestion waits again by the same reference, at the version the undo answered.
#[test]
fn an_approved_rules_undo_from_the_pages_brings_its_suggestion_back() {
    for off in [false, true] {
        let (h, _now, _place, _repo, _off) = ruled();
        let (reference, mut version, of) = row_of(&h, "s2");
        assert_eq!(of["rule"], "approved");
        if off {
            version = left(switched(&h, &reference, &version, true));
        }
        let before = corrections(&h);
        let restored = left(decided(&h, &reference, &version, Decision::Restore));
        let (again, at, of) = row_of(&h, "s2");
        let state = (again.as_str(), of["rule"].as_str());
        assert_eq!(state, (reference.as_str(), Some("waiting")), "off: {off}");
        assert_eq!(at, restored);
        assert_eq!(corrections(&h), before + 1, "its retraction");
        let [retraction, undo] = rule_batches(&h);
        assert_eq!(
            retraction, undo,
            "its retraction and its undo in one batch, off: {off}"
        );
        let (_, _, rule) = read_rule(&h, "Run the tests before a push.");
        assert_eq!(rule["status"], "suggested");
    }
}

/// #1083's R2b: an approved rule's undo is refused, and records nothing, when between the check and
/// the append a session its suggestion was learned from records an event in a repository the
/// exclusion list holds, or a correction of its preference is appended: its preference and its
/// approval are as they were, the preference listed still.
#[test]
fn an_approved_rules_undo_is_refused_when_its_rule_moved() {
    for case in ["excluded", "muted"] {
        let mut h = Store::new();
        let now = crate::db::now_ms();
        let place = tempfile::tempdir().unwrap();
        let open = started_in(
            &mut h,
            &place.path().join("open"),
            "learned",
            now - 5 * 60_000,
        );
        let secret = started_in(
            &mut h,
            &place.path().join("secret"),
            "other",
            now - 5 * 60_000,
        );
        let seq = h.said("learned", &open, now - 4 * 60_000, "Keep commits small.");
        let op = learned_op(
            "s1",
            "Keep commits small.",
            now - 4 * 60_000,
            vec![(seq, seq)],
        );
        h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        h.raw.exclude(&secret, false).unwrap();
        h.run();
        let (reference, version, _) = row_of(&h, "s1");
        let version = left(decided(&h, &reference, &version, Decision::Approve));
        let home = h.home.path().to_owned();
        let moved = move || match case {
            "excluded" => recorded(&home, "learned", &secret),
            _ => muted_elsewhere(&home, &approved_uid(&home, "s1"), true),
        };
        CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(moved)));
        let (before, decisions) = (corrections(&h), decisions_of(&h, "s1"));
        let got = decided_at_once(&h, &reference, &version, Decision::Restore);
        assert_eq!(got, Switched::Behind, "{case}");
        let theirs = usize::from(case == "muted");
        assert_eq!(corrections(&h), before + theirs, "{case}");
        assert_eq!(decisions_of(&h, "s1"), decisions, "{case}");
        // The preference stands: listed by its suggestion, or, its source excluded since, as the
        // owner's own.
        assert!(listed(&h, "Keep commits small."), "{case}");
    }
}

/// #1083's R2b (#1132's review): a rule's switch waits for no learning op, which it does not read.
/// With one the my_work consumer has not applied, the switch's first wait is its own receipt's,
/// after its correction is appended.
#[test]
fn a_switch_waits_for_no_learning_op() {
    let (mut h, now, _place, _repo, _off) = ruled();
    let (reference, version, _) = read_rule(&h, "Write the changelog.");
    let op = learned_op("s9", "Use spaces.", now, vec![]);
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    let before = corrections(&h);
    let raw = h.home.path().join("raw.db");
    let seen = std::rc::Rc::new(std::cell::Cell::new(None));
    let saw = seen.clone();
    crate::claims::WAITING.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let c = rusqlite::Connection::open(&raw).unwrap();
            let sql = "SELECT COUNT(*) FROM ops WHERE type = 'correction'";
            saw.set(Some(
                c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap() as usize
            ));
        }))
    });
    let got = switched(&h, &reference, &version, true);
    assert!(matches!(got, Switched::Applied(Some(_))), "{got:?}");
    assert_eq!(
        seen.get(),
        Some(before + 1),
        "its first wait is its receipt's"
    );
}

/// #1083's R2b (#1132's round 2): an approved rule's version goes by what its switch and its undo
/// act on, its claim and its decision, not by its suggestion's learning op: after a rules round that
/// only adds a quote to it, the switch's Undo still goes through, as on main.
#[test]
fn an_approved_rules_switch_is_undone_after_a_learning_op_of_it() {
    let (mut h, now, _place, _repo, _off) = ruled();
    let (reference, version, _) = row_of(&h, "s2");
    let off = left(switched(&h, &reference, &version, true));
    let op = learned_op("s2", "Run the tests before a push.", now, vec![]);
    h.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    h.run();
    let (again, at, of) = row_of(&h, "s2");
    let state = (again.as_str(), of["rule"].as_str());
    assert_eq!(state, (reference.as_str(), Some("approved")));
    assert_eq!(at, off, "a quote added moves no version");
    let on = switched(&h, &reference, &off, false);
    assert!(matches!(on, Switched::Applied(Some(_))), "{on:?}");
}
