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
    fn turn(&mut self, session: &str, (from, through): (i64, i64), at: i64, f: Value) -> String {
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

/// #841: where a prompt was typed (`entrypoint`, #659) is not its text: the ask and the
/// follow-ups read its words alone.
#[test]
fn an_entry_point_stays_out_of_the_ask_and_the_follow_ups() {
    let mut h = Home::new();
    for (at, text, entry) in [
        (-60_000, "Fix the parser's error messages.", "cli"),
        (-59_000, "Then run the tests.", "claude-vscode"),
    ] {
        let ts = h.t + at;
        let body = json!({"prompt": text, "entrypoint": entry});
        h.s.event("prompt", "s1", (R, "main"), ts, body);
    }
    let v = h.read(Range::All, h.t);
    let s1 = by_id(&v, "c:claude:s1");
    assert_eq!(s1["ask"]["text"], "Fix the parser's error messages.", "{v}");
    assert_eq!(s1["follow_ups"][0]["text"], "Then run the tests.", "{v}");
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
        super::products(h.s.home.path(), &held[i], &|_| Ok(String::new()))
            .unwrap()
            .0
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
    let out =
        "[main def5678] Fix B\nTo https://github.com/o/b.git\n   0000000..def5678  main -> main";
    let pushed = tool("git commit -m 'Fix B' && git push origin main", out);
    h.s.event("tool", "s1", ("github.com/o/b", "main"), t - 58_000, pushed);
    h.run_read();
    let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    let i = ids(&answer)
        .iter()
        .position(|id| id == "c:claude:s1")
        .unwrap();
    let products = super::products(h.s.home.path(), &held[i], &|_| Ok(String::new()))
        .unwrap()
        .0;
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
        (super::products(h.s.home.path(), &held[i], &|_| Ok(String::new()))
            .unwrap()
            .0
            .iter())
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
    let products = super::products(h.s.home.path(), &held[i], &|_| Ok(String::new()))
        .unwrap()
        .0;
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
    let card = json!([{"type": "change", "title": "Went on", "subtitle": "s", "narrative": "n"}]);
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
    let sent = |tool: &str| json!({"tool": tool, "input": input, "output": "", "failed": false});
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

/// Slice 2 of docs/subagents.md: the answer lists the subagents of the conversations its
/// requests list, each with its conversation's id as the request's `sessions` give it and an
/// id of its own, its type, state and its launch's task; a subagent of a conversation no
/// request lists is not shown, and a forget of the conversation leaves none.
#[test]
fn a_listed_conversations_subagents_are_in_its_answer() {
    let mut h = Home::new();
    let asked = h.said("s1", -60_000, "Fix the parser.");
    let input = json!({"description": "Find the parser"}).to_string();
    let launched = json!({"tool": "Agent", "input": input, "launched": {"agent_id": "agent-x1",
            "status": "async_launched", "async": true}});
    h.s.event("tool", "s1", (R, "main"), h.t - 55_000, launched);
    let step = json!({"tool": "Read", "agent_id": "agent-x1", "agent_type": "Explore"});
    h.s.event("tool", "s1", (R, "main"), h.t - 50_000, step);
    let other = json!({"tool": "Read", "agent_id": "agent-x2"});
    h.s.event("tool", "s9", (R, "main"), h.t - 50_000, other);
    let tag = |_: &crate::deliverables::Row| Ok(String::new());
    let page = |h: &Home| {
        h.run_read();
        listed(h.s.home.path(), Range::All, h.t, &tag).unwrap()
    };
    assert_eq!(h.read(Range::All, h.t)["subagents"], json!([]));
    let v = page(&h);
    let subagents = v["subagents"].as_array().unwrap();
    assert_eq!(subagents.len(), 1, "{v}");
    let s = &subagents[0];
    assert_eq!(
        s["conversation"]["id"],
        by_id(&v, "c:claude:s1")["sessions"][0]["id"]
    );
    assert_eq!(s["conversation"]["id"], "c:claude:s1");
    let id = s["id"].as_str().unwrap();
    assert!(id.starts_with("s:") && !id.contains("agent-x1"), "{s}");
    // Read for a day, a conversation older than that lists its subagents still, as Mark
    // finished looks them up (Codex and cubic on #850): the page reads all and filters.
    h.said("s2", -3 * 86_400_000, "Rename the module.");
    let old = json!({"tool": "Read", "agent_id": "agent-x3"});
    h.s.event("tool", "s2", (R, "main"), h.t - 3 * 86_400_000 + 1_000, old);
    h.run_read();
    let day = listed(h.s.home.path(), Range::Day, h.t, &tag).unwrap();
    assert!(ids(&day).iter().all(|id| id != "c:claude:s2"), "{day}");
    assert_eq!(day["subagents"].as_array().unwrap().len(), 2, "{day}");
    assert_eq!(
        (&s["type"], &s["state"], &s["task"], &s["can_mark"]),
        (
            &json!("Explore"),
            &json!("working"),
            &json!("Find the parser"),
            &json!(true)
        )
    );
    let home = h.s.home.path();
    let device = h.s.raw.device().to_owned();
    let target = crate::forget::session_target(home, &format!("{device}:{asked}")).unwrap();
    crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
    let v = page(&h);
    let left = (v["subagents"].as_array().unwrap().iter())
        .filter(|s| s["conversation"]["id"] == "c:claude:s1")
        .count();
    assert_eq!(left, 0, "{v}");
}

/// Slice 2 (Codex on slice 2): the owner's actions read My Work under the reviews' lock, which
/// a purge waits for while it holds the readers' hold; their read takes no readers' hold.
#[test]
fn the_actions_read_takes_no_readers_hold() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Fix the parser.");
    h.run_read();
    let home = h.s.home.path();
    let purging = crate::dispatch::exclusive(home).unwrap();
    std::thread::scope(|scope| {
        let reading = scope.spawn(|| reviewing(home, h.t).map(|(v, _)| v));
        for _ in 0..50 {
            if reading.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let done = reading.is_finished();
        drop(purging);
        reading.join().unwrap().unwrap();
        assert!(done, "the actions' read waits for no readers' hold");
    });
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
/// needs review before blocked (and its aliases, claude-mem 15.1), blocked, ready on the
/// request's own recorded completion or review, shipped apart from it (claude-mem 15.2), in
/// progress (`open` too), to do, and no recent update after 6 quiet hours; work that is all
/// done is "Update recorded", and work all dropped closes a request with no other record.
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
            "shipped",
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
        (
            "shipped",
            json!({"task": "t", "status": "doing", "request_status": "delivered"}),
        ),
        ("ready", json!({"task": "t", "status": "Ready for review"})),
        (
            "ready",
            json!({"task": "t", "status": "doing", "request_status": "in_review"}),
        ),
        ("working", json!({"task": "t", "status": "open"})),
        ("ready", json!({"task": "t", "status": "review"})),
    ];
    for (i, (_, fields)) in cases.iter().enumerate() {
        let session = format!("s{i}");
        h.said(&session, -60_000, "Do the thing.");
        h.wrote(&session, &format!("list {i}"), fields.clone());
    }
    h.said("review", -60_000, "Do the thing.");
    h.wrote("review", "a", json!({"task": "a", "status": "failed"}));
    h.wrote(
        "review",
        "b",
        json!({"task": "b", "status": "awaiting_review"}),
    );
    // Each request of a conversation that names two owns only its prompts, not their times,
    // so it has no record besides its work.
    h.said("ids", -60_000, "Do the two things.");
    h.wrote(
        "ids",
        "d",
        json!({"task": "t", "request_id": "D", "status": "dropped"}),
    );
    h.wrote(
        "ids",
        "e",
        json!({"task": "u", "request_id": "E", "status": "doing"}),
    );
    let v = h.read(Range::All, h.t + 10_000);
    for (i, (status, _)) in cases.iter().enumerate() {
        assert_eq!(
            by_id(&v, &format!("c:claude:s{i}"))["status"],
            *status,
            "{i}"
        );
    }
    assert_eq!(by_id(&v, "c:claude:review")["status"], "ready");
    assert_eq!(by_id(&v, "r:D")["status"], "closed");
    // P5: a ship recorded with no review is 100% on the card.
    assert_eq!(by_id(&v, "c:claude:s4")["progress"]["percent"], 100);
    assert_eq!(by_id(&v, "c:claude:s5")["progress"], Value::Null);
    let later = h.read(Range::All, h.t + QUIET_MS + 60_000);
    assert_eq!(by_id(&later, "c:claude:s5")["status"], "quiet");
    assert_eq!(by_id(&later, "c:claude:s2")["status"], "quiet");
    assert_eq!(by_id(&later, "c:claude:s0")["status"], "needs");
    // Needs review and closed come before no recent update, as needs you does.
    assert_eq!(by_id(&later, "c:claude:s11")["status"], "ready");
    assert_eq!(by_id(&later, "r:D")["status"], "closed");
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
    let rebuilt =
        json!([{"type": "change", "title": "Index rebuilt", "subtitle": " ", "narrative": "x"}]);
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
    let detail = |task: &str| work.iter().find(|w| w["task"] == task).unwrap()["detail"].clone();
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
    let config = "[redaction]\nextra_rules = [{ id = \"canary\", regex = 'canary-[0-9]{4}' }]\n";
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
    let rule = "[redaction]\nextra_rules = [{ id = \"id\", regex = '^(claude|sess-[0-9]+)$' }]\n";
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
    let rule = "[redaction]\nextra_rules = [{ id = \"repo\", regex = 'github\\.com/o/[a-z]+' }]\n";
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
        (shown.iter()).all(|id| id.starts_with("c:claude:[REDACTED]#") && !id.contains("sess-")),
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

/// F2 (CodeRabbit on #746): a placement is about its request's anchor conversation as the stores
/// hold it, so it holds when the conversation joins a group, and when the home's key is new.
#[test]
fn a_placement_holds_when_its_conversation_joins_a_group() {
    let mut h = Home::new();
    let first = h.said("s1", 0, "Build the parser.");
    let second = h.said("s2", 1_000, "Add tests for the parser.");
    let home = h.s.home.path().to_owned();
    let id = "fa_0123456789ab";
    let area = json!({"kind": "area", "name": "Parser", "status": "kept", "source": "owner",
            "decided_by": "owner"});
    crate::my_work_ops::record(&home, "focus", vec![(id.into(), area)]).unwrap();
    let focus = |h: &Home, prefix: &str| {
        let v = h.read(Range::All, crate::db::now_ms());
        (v["requests"].as_array().unwrap().iter())
            .find(|r| r["id"].as_str().unwrap().starts_with(prefix))
            .map(|r| r["focus"].clone())
    };
    assert_eq!(
        focus(&h, "c:claude:s1"),
        Some(Value::Null),
        "not sorted yet"
    );
    let body = json!({"action": "move-request", "request": "c:claude:s1", "area": id});
    crate::my_work_focus::act(
        &home,
        &std::sync::Mutex::new(()),
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap();
    assert_eq!(focus(&h, "c:claude:s1"), Some(json!(id)));
    grouped(&mut h, &[(&["s1", "s2"], "The parser")], &[first, second]);
    assert_eq!(focus(&h, "g:"), Some(json!(id)));
    std::fs::remove_file(home.join("state").join("my-work.key")).unwrap();
    assert_eq!(focus(&h, "g:"), Some(json!(id)));
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
    let products = super::products(h.s.home.path(), &held[0], &|_| Ok(String::new()))
        .unwrap()
        .0;
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

/// A copy's row for `session`'s call at `seq` (`at` from the home's time), as the phase makes
/// one (docs/deliverables.md), its source at `source`, and its file on disk.
fn copy_row(h: &Home, session: &str, seq: i64, at: i64, source: &str) {
    let title = source.rsplit('/').next().unwrap();
    let folder = h.s.home.path().join("deliverables").join("r");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(title), b"abc").unwrap();
    let db = crate::deliverables::open(h.s.home.path()).unwrap();
    let device = crate::raw::open(h.s.home.path())
        .unwrap()
        .device()
        .to_owned();
    db.execute(
            "INSERT INTO deliverables(device, agent, session, repo, turn_seq, turn_ts, call_seq,
               call_ts, title, kind, source_path, copy, bytes, sha256, declared, list, field, copied)
             VALUES(?1, 'claude', ?2, ?3, ?4, ?5, ?4, ?5, ?6, 'document', ?7, ?8, 3, 'ab', 0, NULL,
               NULL, ?9)",
            params![
                device,
                session,
                R,
                seq,
                h.t + at,
                title,
                source,
                format!("r/{title}"),
                h.t + at + 1
            ],
        )
        .unwrap();
}

/// Slice 2 of docs/deliverables.md: a request's page lists its copies with their links' tags,
/// newest first, a copy hiding the recorded file of the path it covers (its source gone); a
/// copy of another conversation is not this request's; the model's review input
/// (`products_read`) has no copy; the list answer's `copies` names each request's copies.
#[test]
fn a_requests_copies_join_its_work_product() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Write the report.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    h.wrote(
        "s1",
        "report",
        json!({"task": "Report", "status": "done", "work_product": "/gone/report.pdf"}),
    );
    h.said("s2", -50_000, "Something else.");
    let other = bash(&mut h, "s2", -49_000, "ls", "");
    h.run_read();
    let shown = |h: &Home| {
        let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
        let i = (ids(&answer).iter())
            .position(|id| id == "c:claude:s1")
            .unwrap();
        let tag = |r: &crate::deliverables::Row| Ok(format!("tag{}", r.id));
        super::products(h.s.home.path(), &held[i], &tag).unwrap()
    };
    let shown = |h: &Home| {
        let (list, unread) = shown(h);
        assert!(!unread, "the copies were read");
        list
    };
    let before: Vec<Value> = shown(&h);
    assert!(
        before.iter().any(|p| p["path"] == "/gone/report.pdf"),
        "{before:?}"
    );
    copy_row(&h, "s1", call, -59_000, "/gone/report.pdf");
    copy_row(&h, "s2", other, -49_000, "/gone/other.pdf");
    let after = shown(&h);
    let files: Vec<(&str, &str)> = (after.iter())
        .filter_map(|p| Some((p["title"].as_str()?, p["file"].as_str()?)))
        .collect();
    assert_eq!(files, [("report.pdf", "tag1")], "{after:?}");
    assert_eq!(
        (after.iter())
            .filter(|p| p["path"] == "/gone/report.pdf")
            .count(),
        1,
        "the copy hides the recorded file it covers: {after:?}"
    );
    // A copy whose file is gone is left out and covers nothing, as claude-mem's mapper and
    // `copyCoversSource` read it (slice 2b): the record shows again.
    let file = h.s.home.path().join("deliverables/r/report.pdf");
    std::fs::rename(&file, file.with_extension("kept")).unwrap();
    let gone = shown(&h);
    assert!(gone.iter().all(|p| p.get("file").is_none()), "{gone:?}");
    assert!(
        gone.iter().any(|p| p["path"] == "/gone/report.pdf"),
        "{gone:?}"
    );
    std::fs::rename(file.with_extension("kept"), &file).unwrap();
    let (raw, k) = crate::search::b::stores(h.s.home.path()).unwrap().unwrap();
    let rules = Rules::load(h.s.home.path()).unwrap();
    let (_, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    for held in &held {
        let read = products_read(&raw, &k, &rules, &held.sources).unwrap();
        assert!(
            read.iter().all(|(p, _)| p.get("file").is_none()),
            "{read:?}"
        );
    }
    let answer = super::read(h.s.home.path(), Range::All, h.t).unwrap();
    let none = crate::deliverables::fingerprint(std::iter::empty());
    for id in ["c:claude:s1", "c:claude:s2"] {
        assert_ne!(by_id(&answer, id)["copies"], json!(none), "{id}");
    }
    // A copies' store that does not read leaves the list and the page without copies only.
    let store = h.s.home.path().join("deliverables.db");
    std::fs::remove_file(&store).unwrap();
    std::fs::create_dir(&store).unwrap();
    let answer = super::read(h.s.home.path(), Range::All, h.t).unwrap();
    assert!(
        by_id(&answer, "c:claude:s1").get("copies").is_none(),
        "{answer}"
    );
    let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    let i = (ids(&answer).iter())
        .position(|id| id == "c:claude:s1")
        .unwrap();
    let (unread, flagged) =
        super::products(h.s.home.path(), &held[i], &|_| Ok(String::new())).unwrap();
    assert!(flagged, "the page is told its copies were not read");
    assert!(unread.iter().all(|p| p.get("file").is_none()), "{unread:?}");
    assert!(
        unread.iter().any(|p| p["path"] == "/gone/report.pdf"),
        "{unread:?}"
    );
}

/// Slice 2: a request's products are one answer (#439), built in the readers' hold, so a
/// forget's registration (which takes the hold whole) cannot come while it is being built.
#[test]
fn a_requests_products_are_built_in_the_readers_hold() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Write the report.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    h.run_read();
    copy_row(&h, "s1", call, -59_000, "/gone/a.pdf");
    let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    let i = (ids(&answer).iter())
        .position(|id| id == "c:claude:s1")
        .unwrap();
    let held = held.into_iter().nth(i).unwrap();
    let home = h.s.home.path().to_owned();
    let (inside, building) = std::sync::mpsc::channel();
    let (go, paused) = std::sync::mpsc::channel::<()>();
    let reader = std::thread::spawn({
        let home = home.clone();
        move || {
            // The answer pauses while it is being built, at its copy's tag.
            let tag = |r: &crate::deliverables::Row| {
                inside.send(()).unwrap();
                paused.recv().unwrap();
                Ok(format!("tag{}", r.id))
            };
            super::products(&home, &held, &tag).map(|(list, _)| list.len())
        }
    });
    building.recv().unwrap();
    let wait = std::time::Duration::from_millis(100);
    assert!(
        crate::dispatch::exclusive_within(&home, wait).is_err(),
        "a registration waits for the answer"
    );
    go.send(()).unwrap();
    assert_eq!(reader.join().unwrap().unwrap(), 1);
    assert!(crate::dispatch::exclusive_within(&home, wait).is_ok());
}

/// Slice 2: where a request holds prompts of a conversation, a copy joins it when a held span
/// holds its call and raw still has that call; a copy whose call raw no longer has (a restore
/// gave its seq to another record) joins only the conversation's one holder. A forget over a
/// copy's call leaves it out from its registration on, the request kept and the copy not yet
/// swept.
#[test]
fn a_copy_joins_the_request_that_holds_its_call() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Write the report.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    h.run_read();
    let device = crate::raw::open(h.s.home.path())
        .unwrap()
        .device()
        .to_owned();
    copy_row(&h, "s1", call, -59_000, "/gone/a.pdf");
    // A row whose call time is not the record's: raw no longer has that call.
    copy_row(&h, "s1", call, -58_500, "/gone/b.pdf");
    let (raw, _) = crate::search::b::stores(h.s.home.path()).unwrap().unwrap();
    let db = crate::deliverables::open_read(h.s.home.path())
        .unwrap()
        .unwrap();
    let key: Key = ("claude".into(), "s1".into());
    let held = |spans: Option<Vec<Span>>, sole: bool| Held {
        key: String::new(),
        id: String::new(),
        sessions: Vec::new(),
        sources: Sources {
            convs: vec![(key.clone(), spans)],
            fields: Vec::new(),
            files: Vec::new(),
        },
        suggested: None,
        about: None,
        sole: if sole { vec![key.clone()] } else { Vec::new() },
    };
    let titles = |held: &Held| -> Vec<String> {
        (copies_of(&db, &raw, held).unwrap().into_iter())
            .map(|r| r.title)
            .collect()
    };
    let holding = Some(vec![(device.clone(), call - 1, call + 1)]);
    let elsewhere = Some(vec![(device.clone(), call + 10, call + 20)]);
    assert_eq!(titles(&held(holding.clone(), false)), ["a.pdf"]);
    assert_eq!(
        titles(&held(elsewhere.clone(), false)),
        Vec::<String>::new()
    );
    assert_eq!(titles(&held(elsewhere, true)), ["b.pdf"]);
    assert_eq!(titles(&held(holding.clone(), true)), ["a.pdf", "b.pdf"]);
    assert_eq!(titles(&held(None, false)), ["a.pdf", "b.pdf"]);
    // A forget of the time of a.pdf's call, not of the prompt: the request stays.
    let target = crate::forget::Target::Range {
        device,
        since: h.t - 59_100,
        until: h.t - 58_900,
    };
    let preview = crate::forget::preview(h.s.home.path(), target).unwrap();
    crate::forget::start(h.s.home.path(), &preview).unwrap();
    assert_eq!(titles(&held(holding, true)), ["b.pdf"]);
    assert_eq!(titles(&held(None, false)), ["b.pdf"]);
    let (answer, _) = super::reviewing(h.s.home.path(), h.t).unwrap();
    assert!(ids(&answer).contains(&"c:claude:s1".to_owned()), "{answer}");
}

/// Slice 2b of docs/deliverables.md: the list's card of a request is its page's products as
/// claude-mem's `isCardDeliverable` takes them, the first three and how many: not its commits
/// and branches, which stay on its page, copies among them. A request of history alone, and
/// the list read without the page's links (`read`), have no card.
#[test]
fn a_card_shows_the_first_deliverables_of_its_page() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Ship the report.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    let switched = "Switched to a new branch 'report'";
    bash(&mut h, "s1", -20_000, "git checkout -b report", switched);
    let committed = "[report abc1234] Add the report";
    bash(
        &mut h,
        "s1",
        -10_000,
        "git commit -m 'Add the report'",
        committed,
    );
    let products = r#"[{"title": "Spec", "url": "https://example.com/spec"},
            {"title": "Deck", "path": "/gone/deck.pptx"}, {"title": "Sheet", "path": "/gone/sheet.csv"}]"#;
    h.wrote(
        "s1",
        "report",
        json!({"task": "Report", "status": "doing", "work_products": products}),
    );
    h.said("s2", -50_000, "Tidy the parser.");
    bash(
        &mut h,
        "s2",
        -49_000,
        "git commit -m 'Tidy'",
        "[main def5678] Tidy",
    );
    h.run_read();
    copy_row(&h, "s1", call, -59_000, "/gone/report.pdf");
    let tag = |r: &crate::deliverables::Row| Ok(format!("tag{}", r.id));
    let listed = super::listed(h.s.home.path(), Range::All, h.t, &tag).unwrap();
    let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    let i = (ids(&answer).iter())
        .position(|id| id == "c:claude:s1")
        .unwrap();
    let (page, _) = super::products(h.s.home.path(), &held[i], &tag).unwrap();
    let kinds: Vec<&str> = page.iter().filter_map(|p| p["kind"].as_str()).collect();
    assert!(
        kinds.contains(&"commit") && kinds.contains(&"branch"),
        "{page:?}"
    );
    let deliverables: Vec<&Value> = (page.iter())
        .filter(|p| !["commit", "branch"].contains(&p["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(deliverables.len(), 4, "{page:?}");
    assert!(deliverables.iter().any(|p| p["file"] == "tag1"), "{page:?}");
    let card = by_id(&listed, "c:claude:s1");
    assert_eq!(card["deliverable_count"], 4, "{card}");
    let shown: Vec<&Value> = card["deliverables"].as_array().unwrap().iter().collect();
    assert_eq!(shown, deliverables[..3], "{card}");
    for none in [by_id(&listed, "c:claude:s2"), by_id(&answer, "c:claude:s1")] {
        assert!(
            none.get("deliverables").is_none() && none.get("deliverable_count").is_none(),
            "{none}"
        );
    }
    // The copies' fingerprints are the same with the card as without it.
    assert_eq!(card["copies"], by_id(&answer, "c:claude:s1")["copies"]);
    // A copies' store that does not read leaves the card without its copies, not without
    // the rest.
    let store = h.s.home.path().join("deliverables.db");
    std::fs::remove_file(&store).unwrap();
    std::fs::create_dir(&store).unwrap();
    let listed = super::listed(h.s.home.path(), Range::All, h.t, &tag).unwrap();
    let card = by_id(&listed, "c:claude:s1");
    assert_eq!(card["deliverable_count"], 3, "{card}");
    assert!(
        (card["deliverables"].as_array().unwrap().iter()).all(|p| p.get("file").is_none()),
        "{card}"
    );
}

/// #840: a copy hides the record of its own path only, compared with the path as recorded,
/// before the page's gate: a card's file as the card stored it, a work item's where the gate
/// left its field as it was. A copy whose own path is the very text the gate shows of a
/// recorded path hides nothing, nor does a copy of a path recorded with the mask in it (a card
/// summarized from masked text), nor one of the path the gate shows of a card's file it changed
/// without a mask (a space it trims), nor one of a path in a work item's field the gate changed
/// (read as gated, its path is not known as recorded); a copy of a card's file the gate masks
/// hides its record. The model's
/// review input (`products_read`) has neither a copy nor an ungated path.
#[test]
fn a_copy_hides_only_the_record_of_its_own_path() {
    let mut h = Home::new();
    let p = h.said("s1", -60_000, "Write the plan.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    h.wrote(
        "s1",
        "plan",
        json!({"task": "Plan", "status": "doing", "work_product": "/gone/secret-plan.pdf"}),
    );
    // A field the gate changes elsewhere than its path: the path is not known as recorded.
    let notes = r#"[{"title": "secret notes", "path": "/gone/notes.pdf"}]"#;
    h.wrote(
        "s1",
        "notes",
        json!({"task": "Notes", "status": "doing", "work_products": notes}),
    );
    let r = h.reply("s1", -50_000);
    let files = [
        "/gone/secret-deck.pptx",
        "/gone/[REDACTED]-old.pdf",
        "/gone/trail.pdf ",
        "/gone/same.pdf",
        "/gone/secret-sheet.csv",
    ];
    h.s.cards(
        p,
        r,
        json!([{"type": "feature", "title": "Deck", "narrative": "Made it.",
            "files_modified": files}]),
        false,
    );
    h.run_read();
    std::fs::write(
        h.s.home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = 'secret', regex = 'secret' }]\n",
    )
    .unwrap();
    for source in [
        "/gone/[REDACTED]-plan.pdf",
        "/gone/secret-plan.pdf",
        "/gone/[REDACTED]-deck.pptx",
        "/gone/[REDACTED]-old.pdf",
        "/gone/trail.pdf",
        "/gone/same.pdf",
        "/gone/secret-sheet.csv",
        "/gone/notes.pdf",
    ] {
        copy_row(&h, "s1", call, -59_000, source);
    }
    let (answer, held) = super::reviewing(h.s.home.path(), h.t).unwrap();
    let i = (ids(&answer).iter())
        .position(|id| id == "c:claude:s1")
        .unwrap();
    let tag = |r: &crate::deliverables::Row| Ok(format!("tag{}", r.id));
    let (page, _) = super::products(h.s.home.path(), &held[i], &tag).unwrap();
    // Records and copies shown at each path.
    let listed = |path: &str| -> (usize, usize) {
        let at: Vec<&Value> = page.iter().filter(|p| p["path"] == path).collect();
        let copies = at.iter().filter(|p| p.get("file").is_some()).count();
        (at.len() - copies, copies)
    };
    assert_eq!(listed("/gone/[REDACTED]-plan.pdf"), (1, 2), "{page:?}");
    assert_eq!(listed("/gone/[REDACTED]-deck.pptx"), (1, 1), "{page:?}");
    assert_eq!(listed("/gone/[REDACTED]-old.pdf"), (1, 1), "{page:?}");
    assert_eq!(listed("/gone/trail.pdf"), (1, 1), "{page:?}");
    assert_eq!(listed("/gone/same.pdf"), (0, 1), "{page:?}");
    assert_eq!(listed("/gone/[REDACTED]-sheet.csv"), (0, 1), "{page:?}");
    assert_eq!(listed("/gone/notes.pdf"), (1, 1), "{page:?}");
    assert!(!serde_json::to_string(&page).unwrap().contains("secret"));
    let (raw, k) = crate::search::b::stores(h.s.home.path()).unwrap().unwrap();
    let rules = Rules::load(h.s.home.path()).unwrap();
    let read = products_read(&raw, &k, &rules, &held[i].sources).unwrap();
    let read = serde_json::to_string(&read.iter().map(|(p, _)| p).collect::<Vec<_>>()).unwrap();
    assert!(
        !read.contains("secret") && !read.contains("\"file\"") && !read.contains("trail.pdf "),
        "{read}"
    );
}

/// Slice 2b: the list's cards are built in the readers' hold, as a request's page is (#439),
/// so a forget's registration cannot come while their copies are read.
#[test]
fn the_cards_are_built_in_the_readers_hold() {
    let mut h = Home::new();
    h.said("s1", -60_000, "Write the report.");
    let call = bash(&mut h, "s1", -59_000, "ls", "");
    h.run_read();
    copy_row(&h, "s1", call, -59_000, "/gone/a.pdf");
    let home = h.s.home.path().to_owned();
    let (inside, building) = std::sync::mpsc::channel();
    let (go, paused) = std::sync::mpsc::channel::<()>();
    let reader = std::thread::spawn({
        let home = home.clone();
        let now = h.t;
        move || {
            let tag = |r: &crate::deliverables::Row| {
                inside.send(()).unwrap();
                paused.recv().unwrap();
                Ok(format!("tag{}", r.id))
            };
            super::listed(&home, Range::All, now, &tag)
                .map(|v| v["requests"][0]["deliverable_count"].clone())
        }
    });
    building.recv().unwrap();
    let wait = std::time::Duration::from_millis(100);
    assert!(
        crate::dispatch::exclusive_within(&home, wait).is_err(),
        "a registration waits for the cards"
    );
    go.send(()).unwrap();
    assert_eq!(reader.join().unwrap().unwrap(), 1);
    assert!(crate::dispatch::exclusive_within(&home, wait).is_ok());
}

/// Slice 2b's measurement (docs/deliverables.md): on a copy of a store, never the live one,
/// how long the list's answer takes with its cards' deliverables (`listed`) and without them
/// (`read`), seven of each taken in turn, and what the cards show. By hand:
/// `OBOETE_MEASURE_HOME=<copy> cargo test --release --bin oboete -- --ignored --nocapture
/// measure_the_cards`.
#[test]
#[ignore]
fn measure_the_cards_on_a_copy_of_a_store() {
    let home = std::env::var("OBOETE_MEASURE_HOME").expect("a copy of a store");
    let home = Path::new(&home);
    let now = crate::db::now_ms();
    let tag = |r: &crate::deliverables::Row| Ok(format!("{:032x}", r.id));
    let (mut plain, mut cards) = (Vec::new(), Vec::new());
    for _ in 0..7 {
        let started = std::time::Instant::now();
        std::hint::black_box(super::read(home, Range::All, now).unwrap());
        plain.push(started.elapsed().as_secs_f64());
        let started = std::time::Instant::now();
        std::hint::black_box(super::listed(home, Range::All, now, &tag).unwrap());
        cards.push(started.elapsed().as_secs_f64());
    }
    plain.sort_by(f64::total_cmp);
    cards.sort_by(f64::total_cmp);
    let v = super::listed(home, Range::All, now, &tag).unwrap();
    let requests = v["requests"].as_array().unwrap();
    let shown: Vec<&Value> = (requests.iter())
        .filter_map(|r| r["deliverables"].as_array())
        .flatten()
        .collect();
    eprintln!(
        "requests {}, cards {}, deliverables shown {} (copies {}); without the cards \
             {:.3} s fastest, {:.3} s median; with them {:.3} s, {:.3} s",
        requests.len(),
        requests
            .iter()
            .filter(|r| r.get("deliverables").is_some())
            .count(),
        shown.len(),
        shown.iter().filter(|d| d.get("file").is_some()).count(),
        plain[0],
        plain[3],
        cards[0],
        cards[3]
    );
}
