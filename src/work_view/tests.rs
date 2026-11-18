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

/// A home of the last minutes: Claude's requests that need the owner, that shipped and that are
/// done, Codex's that only asked, and a request of its own name with a list of one name in two
/// repositories. Goals written over the first prompt ask for the parser (an area with an open
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
/// it, at noon with no zone so the browser shows that day, and a later update moves nothing.
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

/// What changes from home to home and run to run, written as its kind: the times, days and
/// months, the keyed fingerprints and the masks' prints. What is left is what the answer says.
fn steady(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.as_i64().is_some_and(|n| n > 1_000_000_000_000) => json!("<ms>"),
        Value::String(s) => {
            let iso = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T[0-9:.]+(Z|[+-]\d{2}:\d{2})").unwrap();
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
                .map(|(k, v)| match k.as_str() {
                    "contentFingerprint" | "revision" if v.is_string() => {
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
    if local.day() == 1 && local.hour() == 0 && local.minute() < 15 {
        return;
    }
    let (h, now) = home();
    let (work, goals) = answers(&h, now);
    let fresh = snapshot_at(&work, &goals, now, &Utc);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/work-view/contract/snapshot.json");
    if std::env::var_os("OBOETE_CONTRACT_WRITE").is_some() {
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
