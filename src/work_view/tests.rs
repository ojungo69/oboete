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
const DAY: i64 = 86_400_000;
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
/// ask under no area. Goals of 45 days ago make a finished month of Past focus.
fn home() -> (Store, i64) {
    let mut h = Store::new();
    let t = crate::db::now_ms() - 10 * 60_000;
    let old = t - 45 * DAY;
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

/// The two answers the route reads, through the gate as it sends them.
fn answers(h: &Store, now: i64) -> (Value, Value) {
    let tag = |_: &crate::deliverables::Row| anyhow::Ok(String::new());
    let week = crate::my_work::Range::Week;
    let work = crate::my_work::listed(h.home.path(), week, now, &tag).unwrap();
    let goals = crate::my_work_goals::page(h.home.path(), now).unwrap();
    (
        crate::view::answered("my-work", work),
        crate::view::answered("my-goals", goals),
    )
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
    let snapshot = snapshot_at(&work, &goals, now, &Utc);
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
    let snapshot = snapshot_at(&work, &goals, now, &tokyo);
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
    let snapshot = snapshot_at(&work, &goals, now, &Utc);
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
    let snapshot = snapshot_at(&work, &goals, now, &Utc);
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
    let snapshot = snapshot_at(&work, &goals, now, &Utc);
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
            "completed": "2026-11-19", "completed_at": null},
        "story": {"narrative": "Shipped.", "milestones": [{"date": "2026-11-18", "text": "Built"}]}}]});
    let snapshot = snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc);
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
    let request = &snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc)["humanRequests"][0];
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
    let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
    let want = "Two weeks of the work-view.\n\n## 2026-11-16\n\nBuilt the adapter.\n\n## 2026-11-09\n\nRan the spike.";
    assert_eq!(story, &json!(want));
    let work = json!({"story": {"recap": null, "chapters": chapters}});
    let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
    assert!(
        story
            .as_str()
            .is_some_and(|n| n.starts_with("## 2026-11-16\n\nBuilt")),
        "{story}"
    );
    assert!(snapshot_at(&json!({}), &json!({}), 0, &Utc)["story"].is_null());
}

/// A list-level update has no task: its list names it, as on oboete's own page, and a missing
/// status is an empty one, as the item's type has it.
#[test]
fn a_list_level_update_is_named_by_its_list() {
    let work = json!({"requests": [{"id": "r:z", "status": "working",
        "work": [{"list": "release", "repo": "r", "task": null, "status": null, "updated": 1}]}]});
    let item = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0]["items"][0];
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
    let items = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0]["items"];
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
            "completed": "[REDACTED]", "completed_at": null},
        "story": {"narrative": "Shipped.", "milestones": [{"date": "[REDACTED]", "text": "Built"}]}}]});
    let request = &snapshot_at(&work, &json!({}), 1_795_200_000_000, &Utc)["humanRequests"][0];
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
    let agents = &snapshot_at(&work, &json!({}), 0, &Utc)["agents"];
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
    let progress = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0]["progress"];
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
    let request = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0];
    assert!(request["progress"].is_null(), "{request}");
    assert_eq!(request["completedAtEpoch"], json!(noon));
    let work = json!({"requests": [{"id": "r:v", "status": "shipped", "title": "Release",
        "updated": 1_795_100_000_000_i64, "completed_at": null,
        "progress": {"percent": 100, "reviewed": 1_795_050_000_000_i64, "completed": "2026-11-19",
            "completed_at": null, "readings": [{"ts": 1_795_050_000_000_i64, "percent": 30}]}}]});
    let request = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0];
    assert_eq!(
        request["progress"]["state"],
        json!("in-progress"),
        "{request}"
    );
    assert_eq!(request["completedAtEpoch"], json!(noon));
}

/// The current goals' period is the window the goals page read, the 31 days that end now, in this
/// machine's days: the run's time stays its own (Codex on #1087).
#[test]
fn the_current_goals_period_is_the_window_that_ends_now() {
    let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
    let page = json!({"current": {"written": 1_794_000_000_000_i64, "items": []},
        "window": {"days": 31, "prompts": 4}});
    // 2026-11-19 05:30 in Tokyo.
    let goals = &snapshot_at(&json!({}), &page, 1_795_033_800_000, &tokyo)["goals"];
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
    let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
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
    let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
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
    let request = &snapshot_at(&work, &json!({}), 0, &Utc)["humanRequests"][0];
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
        let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
        assert_eq!(story, &json!(want), "{recap:?}");
    }
    let work = json!({"story": {"recap": "** **", "chapters": []}});
    assert!(snapshot_at(&work, &json!({}), 0, &Utc)["story"].is_null());
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
        let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
        assert_eq!(story, &json!(want));
    }
    let work = json!({"story": {"recap": "## #1087\n### ", "chapters": []}});
    let story = &snapshot_at(&work, &json!({}), 0, &Utc)["story"]["story"]["narrative"];
    assert_eq!(story, &json!("\u{2060}#1087"));
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
            let story = build_story(&json!({"recap": recap, "chapters": chapters}));
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

fn hex(t: &str, n: usize) -> bool {
    t.len() == n && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What changes from home to home and run to run, written as its kind: the times, days and
/// months, the keyed fingerprints and the masks' prints. What is left is what the answer says.
fn steady(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.as_i64().is_some_and(|n| n > 1_000_000_000_000) => json!("<ms>"),
        Value::String(s) => {
            let iso = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T[0-9:.]+(Z|[+-]\d{2}:\d{2})?").unwrap();
            let stamp = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}( \d{2}:\d{2})?$").unwrap();
            let month = regex::Regex::new(r"\b\d{4}-\d{2}\b").unwrap();
            let print = regex::Regex::new(r"#[0-9a-f]{12}").unwrap();
            let s = iso.replace_all(s, "<iso>");
            let s = stamp.replace_all(&s, "<day>");
            let s = month.replace_all(&s, "<month>");
            json!(print.replace_all(&s, "#<print>"))
        }
        Value::Array(a) => Value::Array(a.iter().map(steady).collect()),
        Value::Object(o) => Value::Object(
            (o.iter())
                .map(|(k, v)| match (k.as_str(), v.as_str()) {
                    // A version's keyed hash and the snapshot's time, each in its own shape: an
                    // empty or malformed one stays as it is and differs.
                    ("contentFingerprint", Some(t)) if hex(t, 64) => (k.clone(), json!("<kept>")),
                    ("revision", Some(t))
                        if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) =>
                    {
                        (k.clone(), json!("<kept>"))
                    }
                    _ => (k.clone(), steady(v)),
                })
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// The contract: `ui/work-view/contract/snapshot.json` is this home's answer, which the Node
/// test there runs claude-mem's mapper over (`node --test 'ui/work-view/contract/*.test.mjs'`).
/// After a change to the answer, run this test with OBOETE_CONTRACT_WRITE=1, which writes this
/// run's answer whatever it is (even one that differs only in what `steady` hides), run the Node
/// test, and commit both.
#[test]
fn the_contract_snapshot_is_this_homes_answer() {
    // shortcut: in the first 15 minutes of a month (this machine's), the home's newest goals fall
    // in the month before and make a second month of Past focus, so the comparison waits for a
    // later run; upgrade if the home's clock can be set.
    let local = Local::now();
    let first_minutes = local.day() == 1 && local.hour() == 0 && local.minute() < 15;
    let write = std::env::var_os("OBOETE_CONTRACT_WRITE").is_some();
    assert!(
        !(first_minutes && write),
        "no contract snapshot is written in a month's first 15 minutes: wait for them to pass"
    );
    if first_minutes {
        return;
    }
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let fresh = snapshot_at(&work, &goals, now, &Utc);
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
