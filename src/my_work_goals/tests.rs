use super::*;
use crate::config::Summary;
use crate::curate::{Phase, Reading, Reads};
use crate::my_work_ops::MyWorkOp;
use crate::my_work_run::Runner;
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
use crate::raw::OpKind;
use crate::search::b::fixture::Store;

const R: &str = "github.com/o/r";
const B: &str = "github.com/o/billing";
const MINUTE: i64 = 60_000;

/// A home whose records start two hours ago.
struct Home {
    s: Store,
    t: i64,
    summary: Summary,
}

impl Home {
    fn new() -> Self {
        Self {
            s: Store::new(),
            t: crate::db::now_ms() - 120 * MINUTE,
            summary: Summary::default(),
        }
    }

    /// A prompt the owner typed in `session` of `repo`, `at` minutes from the start.
    fn said(&mut self, session: &str, repo: &str, at: i64, text: &str) -> i64 {
        self.s.said(session, repo, self.t + at * MINUTE, text)
    }

    /// A prompt typed after the goals op written now: a second from now.
    fn later(&mut self, session: &str, repo: &str, text: &str) -> i64 {
        self.s
            .said(session, repo, crate::db::now_ms() + 1_000, text)
    }

    /// A prompt another agent sent `session`, marked so at capture (O2).
    fn sent(&mut self, session: &str, at: i64, text: &str) -> i64 {
        let body = json!({"prompt": text, "agent_sent": true});
        let ts = self.t + at * MINUTE;
        self.s.event("prompt", session, (R, "main"), ts, body)
    }

    /// The goals ops, a day older (M2's wait): those not yet applied too.
    fn age(&self) {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        (k.execute(
            "UPDATE my_work SET ts = ts - ?1 WHERE job = ?2",
            rusqlite::params![DAY_MS + MINUTE, NAME],
        ))
        .unwrap();
    }

    fn with<T>(&self, f: impl FnOnce(&Stores) -> T) -> T {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let rules = Rules::load(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        f(&Stores {
            raw: &self.s.raw,
            k: &k,
            rules: &rules,
            summary: &self.summary,
            reading: &reading,
            held: &crate::my_work_run::NOT_HELD,
        })
    }

    fn due(&self) -> Vec<(String, i64)> {
        let due = self.with(|s| OwnerGoals::default().due(s, crate::db::now_ms()).unwrap());
        due.into_iter().map(|d| (d.about, d.since)).collect()
    }

    /// One pass of a runner of the goals job alone, the chain answering `answer` and refusing
    /// what the check refuses: what it did, and the prompts sent.
    fn pass(&mut self, answer: &dyn Fn(&str) -> Value) -> (Phase, Vec<String>) {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let db = crate::providers_db::open(self.s.home.path()).unwrap();
        let rules = Rules::load(self.s.home.path()).unwrap();
        let summary = self.summary.clone();
        let sent = RefCell::new(Vec::new());
        let call = |_: &str,
                    prompt: &str,
                    _: &Value,
                    check: &AnswerCheck,
                    _: &Gate|
         -> Result<ChainResult> {
            sent.borrow_mut().push(prompt.to_owned());
            let output = answer(prompt);
            if let Some(refused) = check(&output) {
                return Err(ChainFailed(vec![Fallback {
                    provider: "fake".into(),
                    reason: refused.into(),
                    skip: Skip::Refused,
                    answer: None,
                }])
                .into());
            }
            Ok(ChainResult {
                provider: "fake".into(),
                output,
                tier: 1,
            })
        };
        let mut runner = Runner::new(
            (true, 100),
            vec![Box::new(OwnerGoals::default()) as Box<dyn Job>],
            Box::new(call),
            (&rules, &summary, "chain"),
        );
        let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
        drop(runner);
        self.s.run();
        (phase, sent.into_inner())
    }

    /// The goals ops, the newest first.
    fn goals(&self) -> Vec<Kept> {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let none = HashSet::new();
        crate::my_work_ops::shown(&k, (NAME, None), None, &none, false, usize::MAX).unwrap()
    }

    fn hidden(&self) -> HashSet<(String, i64)> {
        self.with(|s| crate::my_work_ops::hidden(s.raw, s.k, s.reading).unwrap())
    }
}

fn item(kind: &str, title: &str, quotes: &[(&str, &str)]) -> Value {
    let quotes: Vec<Value> = (quotes.iter())
        .map(|(p, t)| json!({"prompt": p, "text": t}))
        .collect();
    json!({"kind": kind, "title": title, "product": "", "asked": 1, "quotes": quotes})
}

fn answer(new: Value, changed: Value, order: Value) -> Value {
    json!({"order": order, "dropped": [], "new": new, "changed": changed,
            "summary": "Worked on the parser."})
}

fn nothing() -> Value {
    answer(json!([]), json!([]), json!([]))
}

fn titles(k: &Kept) -> Vec<String> {
    items(k).into_iter().map(|i| i.title).collect()
}

/// M1: the owner's typed prompts of the last 31 days alone: not one another agent sent, nor
/// one older than the window.
#[test]
fn goals_read_only_typed_prompts_of_31_days() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.sent("s2", 1, "Review the parser and report each finding.");
    let old = h.t - 32 * DAY_MS;
    h.s.said("s3", R, old, "Rename the old lexer module.");
    let (_, sent) = h.pass(&|_| nothing());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("Make the parser fail fast"), "{}", sent[0]);
    assert!(
        !sent[0].contains("Review the parser") && !sent[0].contains("old lexer"),
        "{}",
        sent[0]
    );
    assert!(
        sent[0].contains("P1 ") && sent[0].contains(R),
        "{}",
        sent[0]
    );
}

/// A5: what the job read is kept for the round, and `Job::reread` drops it. Here
/// a prompt typed since is in its next read, and not while the first is
/// kept. The runner tells every job so when the list changed between a round's two passes
/// (`my_work_run`).
#[test]
fn the_job_keeps_its_read_until_it_is_told_to_read_again() {
    let mut h = Home::new();
    let job = OwnerGoals::default();
    let due = |h: &Home| h.with(|s| job.due(s, crate::db::now_ms()).unwrap().len());
    assert_eq!(due(&h), 0);
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    assert_eq!(due(&h), 0, "kept for the round");
    job.reread();
    assert_eq!(due(&h), 1, "read again");
}

/// M2: with no typed prompt after the last goals, nothing is due and no call goes; an agent's
/// marked prompt is none.
#[test]
fn no_typed_prompt_since_the_last_goals_makes_no_call() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    let (_, sent) = h.pass(&|_| nothing());
    assert_eq!(sent.len(), 1);
    assert!(h.due().is_empty(), "{:?}", h.due());
    h.sent("s2", 5, "Review the parser and report each finding.");
    assert!(h.due().is_empty(), "{:?}", h.due());
    let (_, sent) = h.pass(&|_| panic!("no call"));
    assert!(sent.is_empty());
}

/// M2, A4: a first run reads the window in chunks, oldest first, each within the budget, and
/// each chunk is due at once after the one before; every prompt is shown once.
#[test]
fn the_first_read_goes_in_chunks_that_each_fit_the_budget() {
    let mut h = Home::new();
    let long = |i: usize| {
        let filler = "make the parser handle every header and report each bad byte. ";
        format!("Prompt {i}: {}", filler.repeat(40))
    };
    for i in 0..12 {
        h.said(&format!("s{i}"), R, i as i64, &long(i));
    }
    let budget = Summary::default().window_tokens + 1_500;
    let mut prompts: Vec<String> = Vec::new();
    while let Some((_, since)) = h.due().first().cloned() {
        assert!(since <= crate::db::now_ms(), "a first run goes on at once");
        let (_, sent) = h.pass(&|_| nothing());
        assert_eq!(sent.len(), 1);
        assert!(crate::budget::estimate(&sent[0]) <= budget);
        prompts.push(sent[0].clone());
        assert!(prompts.len() < 20, "{}", prompts.len());
    }
    assert!(prompts.len() >= 2, "{}", prompts.len());
    for i in 0..12 {
        let shown = (prompts.iter())
            .filter(|p| p.contains(&format!("Prompt {i}:")))
            .count();
        assert_eq!(shown, 1, "prompt {i}");
    }
}

/// M2: a prompt typed after the goals waits until they are a day old.
#[test]
fn an_update_waits_a_day_after_the_last_goals() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.pass(&|_| nothing());
    let written = h.goals()[0].op.ts;
    h.later("s2", R, "Add a test for the header check.");
    let due = h.due();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].1, written + DAY_MS);
}

/// M2 (#716): a prompt typed while the goals call is out waits a day after the op, as one
/// typed after it does; only a read that stopped at the budget goes on at once.
#[test]
fn a_prompt_typed_while_the_call_is_out_waits_a_day() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    let home = h.s.home.path().to_owned();
    h.pass(&|_| {
        // Recorded after the job read and before its op is written.
        let body = json!({"prompt": "Add a test for the header check."}).to_string();
        let typed = crate::raw::Event {
            kind: "prompt".into(),
            session: "s2".into(),
            repo: Some(R.into()),
            ts: crate::db::now_ms() - 1,
            ..crate::raw::test_event(&body)
        };
        crate::raw::open(&home).unwrap().append(&typed).unwrap();
        nothing()
    });
    let written = h.goals()[0].op.ts;
    let due = h.due();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].1, written + DAY_MS);
}

/// A goals op written at `ts` that read the prompts `spans`, keeping `items` as stored.
fn stored(h: &mut Home, ts: i64, spans: &[i64], items: Value) {
    let op = MyWorkOp {
        job: NAME.into(),
        about: day(ts, &chrono::Utc),
        entry: "fake".into(),
        ts,
        read: Read {
            spans: spans.iter().map(|&s| (s, s)).collect(),
            ops: vec![],
        },
        removed: vec![],
        fields: BTreeMap::from([
            ("items".to_owned(), items),
            ("summary".to_owned(), json!("Worked on the parser.")),
        ]),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    h.s.run();
}

/// A goal as an op keeps it, its quotes (seq, time, text) the newest first.
fn goal(id: &str, kind: &str, rank: u32, title: &str, quotes: &[(i64, i64, &str)]) -> Value {
    let quotes: Vec<Value> = (quotes.iter())
        .map(|(seq, ts, text)| json!({"seq": seq, "ts": ts, "text": text}))
        .collect();
    json!({"id": id, "kind": kind, "rank": rank, "title": title, "product": "",
            "asked": quotes.len(), "quotes": quotes})
}

/// V2, M5: the page shows each goal's quotes of the last 31 days, leaves out one with none
/// left, counts the ranks again within each kind, and counts the prompts read in the window.
#[test]
fn the_page_shows_the_quotes_of_31_days_and_counts_the_ranks_again() {
    let mut h = Home::new();
    let now = crate::db::now_ms();
    let (then, lately) = (now - 40 * DAY_MS, now - DAY_MS);
    let old = h.s.said("s1", R, then, "Rename the lexer.");
    let new = h.s.said("s2", R, lately, "Make the parser fail fast.");
    let items = json!([
        goal(
            "g1",
            "priority",
            1,
            "Rename the lexer",
            &[(old, then, "the lexer")]
        ),
        goal(
            "g2",
            "priority",
            2,
            "Fail fast",
            &[(new, lately, "fail fast"), (old, then, "lexer")]
        ),
        goal(
            "g3",
            "ask",
            1,
            "A header test",
            &[(new, lately, "the parser")]
        ),
    ]);
    stored(&mut h, lately + 1, &[old, new], items);
    let page = page_at(h.s.home.path(), now, &chrono::Utc).unwrap();
    let shown: Vec<(&str, &str, u64, usize)> = (page["current"]["items"].as_array().unwrap())
        .iter()
        .map(|i| {
            let quotes = i["quotes"].as_array().unwrap().len();
            let (id, kind) = (i["id"].as_str().unwrap(), i["kind"].as_str().unwrap());
            (id, kind, i["rank"].as_u64().unwrap(), quotes)
        })
        .collect();
    assert_eq!(shown, [("g2", "priority", 1, 1), ("g3", "ask", 1, 1)]);
    assert_eq!(page["current"]["items"][0]["last"], lately);
    assert_eq!(
        page["current"]["items"][0]["quotes"][0]["text"],
        "fail fast"
    );
    assert_eq!(page["window"], json!({"days": 31, "prompts": 1}));
}

/// V2, M5: each finished month's goals, the newest month first, with the prompts the goals
/// read in it; this month's count; the current goals.
#[test]
fn the_page_lists_each_finished_month_with_the_prompts_read_in_it() {
    use chrono::TimeZone;
    let mut h = Home::new();
    let at = |m: u32, d: u32| {
        (chrono::Utc.with_ymd_and_hms(2026, m, d, 12, 0, 0))
            .unwrap()
            .timestamp_millis()
    };
    let mut said = |session: &str, ts: i64| h.s.said(session, R, ts, "Work on the parser.");
    let (aug, sep1, sep2, oct) = (
        said("s1", at(8, 20)),
        said("s2", at(9, 3)),
        said("s3", at(9, 28)),
        said("s4", at(10, 2)),
    );
    for (written, seq, ts, title) in [
        (at(8, 21), aug, at(8, 20), "Export plan"),
        (at(9, 4), sep1, at(9, 3), "Lexer"),
        (at(9, 29), sep2, at(9, 28), "Export"),
        (at(10, 3), oct, at(10, 2), "Fail fast"),
    ] {
        let items = json!([goal("g1", "priority", 1, title, &[(seq, ts, "the parser")])]);
        stored(&mut h, written, &[seq], items);
    }
    let page = page_at(h.s.home.path(), at(10, 15), &chrono::Utc).unwrap();
    let rows: Vec<(&str, u64, &str)> = (page["months"].as_array().unwrap().iter())
        .map(|m| {
            let top = m["priorities"][0]["title"].as_str().unwrap();
            (
                m["month"].as_str().unwrap(),
                m["prompts"].as_u64().unwrap(),
                top,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [("2026-09", 2, "Export"), ("2026-08", 1, "Export plan")]
    );
    assert_eq!(
        page["months"][0]["priorities"][0]["quote"]["text"],
        "the parser"
    );
    assert_eq!(page["months"][0]["summary"], "Worked on the parser.");
    assert_eq!(page["month"], json!({"month": "2026-10", "prompts": 1}));
    assert_eq!(page["current"]["items"][0]["title"], "Fail fast");
}

/// S6: goals that read an excluded repository's prompt are not on the page.
#[test]
fn goals_that_read_an_excluded_repository_are_not_on_the_page() {
    let mut h = Home::new();
    let now = crate::db::now_ms();
    let seq = h.s.said("s1", B, now - DAY_MS, "Ship the billing export.");
    let items = json!([goal(
        "g1",
        "priority",
        1,
        "Billing export",
        &[(seq, now - DAY_MS, "billing")]
    )]);
    stored(&mut h, now - DAY_MS + 1, &[seq], items);
    let page = page_at(h.s.home.path(), now, &chrono::Utc).unwrap();
    assert_eq!(page["current"]["items"][0]["title"], "Billing export");
    h.s.exclude(B);
    h.s.run();
    let page = page_at(h.s.home.path(), now, &chrono::Utc).unwrap();
    assert!(page["current"].is_null(), "{page}");
    assert_eq!(page["window"]["prompts"], 0);
}

/// S6: a rule added after the goals were written masks their text on the page.
#[test]
fn a_rule_added_later_masks_the_goals_on_the_page() {
    let mut h = Home::new();
    let now = crate::db::now_ms();
    let seq =
        h.s.said("s1", R, now - DAY_MS, "Sign in with code=123456 first.");
    let quote = [(seq, now - DAY_MS, "code=123456 first")];
    let items = json!([goal(
        "g1",
        "priority",
        1,
        "Sign in with code=123456",
        &quote
    )]);
    stored(&mut h, now - DAY_MS + 1, &[seq], items);
    assert!(
        page_at(h.s.home.path(), now, &chrono::Utc)
            .unwrap()
            .to_string()
            .contains("123456")
    );
    let rule = "[redaction]\nextra_rules = [{ id = \"r\", regex = 'code=([0-9]{6})', secret_group = 1 }]\n";
    std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
    let page = page_at(h.s.home.path(), now, &chrono::Utc)
        .unwrap()
        .to_string();
    assert!(
        page.contains("Sign in") && !page.contains("123456"),
        "{page}"
    );
}

/// M4: a quote must be found in the prompt it names, else it goes; an item with none goes.
#[test]
fn a_quote_must_be_found_in_the_prompt_it_names() {
    let mut h = Home::new();
    let first = h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.said("s2", R, 1, "Keep the logs in UTC everywhere.");
    h.pass(&|_| {
        answer(
            json!([
                item(
                    "priority",
                    "Fail fast",
                    &[("P1", "fail fast on a bad header")]
                ),
                item("ask", "UTC logs", &[("P1", "logs in UTC")]),
                item("ask", "Made up", &[("P2", "a quote no prompt holds")]),
            ]),
            json!([]),
            json!([]),
        )
    });
    let kept = items(&h.goals()[0]);
    assert_eq!(
        kept.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["Fail fast"]
    );
    assert_eq!(kept[0].quotes[0].seq, first);
    assert_eq!((kept[0].id.as_str(), kept[0].rank), ("g1", 1));
}

/// M3, M4: an item the answer changes keeps its id; a new one takes the next.
#[test]
fn a_kept_item_keeps_its_id() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.pass(&|_| {
        let new = item("priority", "Fail fast", &[("P1", "fail fast")]);
        answer(json!([new]), json!([]), json!([]))
    });
    h.later("s2", R, "The parser should also fail fast on a bad footer.");
    h.age();
    let (_, sent) = h.pass(&|_| {
        let changed = json!({"id": "g1", "title": "Fail fast everywhere", "product": "",
                "asked": 2, "quote": {"prompt": "P1", "text": "fail fast on a bad footer"}});
        let new = item("ask", "Footer check", &[("P1", "a bad footer")]);
        answer(json!([new]), json!([changed]), json!(["g1"]))
    });
    assert!(
        sent[0].contains("g1 priority rank 1: Fail fast"),
        "{}",
        sent[0]
    );
    let kept = items(&h.goals()[0]);
    let by_id: BTreeMap<&str, &Item> = kept.iter().map(|i| (i.id.as_str(), i)).collect();
    assert_eq!(by_id["g1"].title, "Fail fast everywhere");
    assert_eq!(by_id["g1"].quotes.len(), 2);
    assert!(by_id["g1"].asked >= 2);
    assert_eq!(by_id["g2"].title, "Footer check");
}

/// F5: a new priority of no area makes a suggested area, sourced owner-goals, of its title,
/// product and rank; a match to a suggested area no owner decided moves its rank and fills an
/// empty goal; an area the owner decided is left as it is; one the system withdrew is not shown.
#[test]
fn a_priority_makes_or_matches_its_focus_area() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.pass(&|_| {
        let mut new = item("priority", "Parser", &[("P1", "fail fast")]);
        new["product"] = json!("A parser that fails fast");
        answer(json!([new]), json!([]), json!([]))
    });
    let focus =
        |h: &Home| h.with(|s| crate::my_work_focus::Focus::read(s.raw, s.k, s.reading).unwrap());
    let parser = items(&h.goals()[0])[0].area.clone().expect("an area");
    assert!(crate::my_work_focus::is_id(&json!(parser)), "{parser}");
    let made = focus(&h).areas[&parser].clone();
    assert_eq!(
        (
            made.name(),
            made.status(),
            made.text("source"),
            made.text("goal")
        ),
        (
            "Parser",
            "suggested",
            Some("owner-goals"),
            Some("A parser that fails fast")
        )
    );
    assert_eq!(made.fields["priority_rank"], json!(1));
    let area = |name: &str, status: &str, by: &str| {
        json!({"kind": "area", "name": name, "status": status, "source": "clustering",
                "decided_by": by})
    };
    let mut docs = area("Docs", "suggested", "system");
    docs["goal"] = json!("Keep the docs current");
    let ops = vec![
        (
            "fa_00000000000a".to_owned(),
            area("Lexer", "suggested", "system"),
        ),
        ("fa_00000000000b".to_owned(), area("Tests", "kept", "owner")),
        (
            "fa_00000000000c".to_owned(),
            area("Gone", "dismissed", "system"),
        ),
        ("fa_00000000000d".to_owned(), docs),
        (
            "fa_00000000000e".to_owned(),
            area("Release", "done", "system"),
        ),
        (
            "fa_000000000010".to_owned(),
            area("Owned", "suggested", "owner"),
        ),
    ];
    crate::my_work_ops::record(h.s.home.path(), crate::my_work_focus::NAME, ops).unwrap();
    h.later("s2", R, "Write a lexer, test the parser, ship it, own it.");
    h.age();
    let (_, sent) = h.pass(&|_| {
        let mut new = vec![];
        for (title, quote, area) in [
            ("Lexer", "Write a lexer", "fa_00000000000a"),
            ("Tests", "test the parser", "fa_00000000000b"),
            ("Release", "ship it", "fa_00000000000e"),
            ("Owned", "own it", "fa_000000000010"),
        ] {
            let mut n = item("priority", title, &[("P1", quote)]);
            n["product"] = json!(format!("The {title}"));
            n["area"] = json!(area);
            new.push(n);
        }
        let moved = json!({"id": "g1", "title": "", "product": "Docs of the parser",
                "asked": 1, "quote": {"prompt": "", "text": ""}, "area": "fa_00000000000d"});
        answer(json!(new), json!([moved]), json!(["g1"]))
    });
    assert!(
        sent[0].contains(&format!("area {parser}"))
            && sent[0].contains("- fa_00000000000a suggested: Lexer")
            && !sent[0].contains("Gone"),
        "{}",
        sent[0]
    );
    assert_eq!(
        items(&h.goals()[0])[0].area.as_deref(),
        Some("fa_00000000000d")
    );
    let now = focus(&h);
    // S2: it read the shown areas' ops; an area the owner decided got none of its rows.
    let newest = &h.goals()[0];
    let lexer = now.sources("fa_00000000000a");
    let before: Vec<i64> = (lexer.iter())
        .map(|(_, seq)| *seq)
        .filter(|seq| *seq != newest.op_seq)
        .collect();
    assert!(
        before.len() == 1 && before.iter().all(|seq| newest.op.read.ops.contains(seq)),
        "{:?} {lexer:?}",
        newest.op.read.ops
    );
    for owners in ["fa_00000000000b", "fa_000000000010"] {
        assert_eq!(now.sources(owners).len(), 1, "only the owner's own op");
    }
    let rank_goal = |id: &str| {
        let a = &now.areas[id];
        (
            a.fields.get("priority_rank").cloned(),
            a.text("goal").map(str::to_owned),
        )
    };
    assert_eq!(
        rank_goal("fa_00000000000a"),
        (Some(json!(2)), Some("The Lexer".into()))
    );
    assert_eq!(rank_goal("fa_00000000000b"), (None, None), "the owner's");
    assert_eq!(rank_goal("fa_00000000000e"), (None, None), "done");
    assert_eq!(
        rank_goal("fa_00000000000d"),
        (Some(json!(1)), Some("Keep the docs current".into())),
        "its goal kept"
    );
    // A priority named with a page word makes no area.
    let goals = json!({"items": [{"id": "g9", "kind": "priority", "rank": 1, "title": "My Work",
            "product": "", "asked": 1, "quotes": [], "area": "fa_00000000000f"}]});
    let day = vec![("2026-10-29".to_owned(), goals)];
    crate::my_work_ops::record(h.s.home.path(), NAME, day).unwrap();
    assert!(!focus(&h).areas.contains_key("fa_00000000000f"));
}

/// F5 (Codex on #754): the areas a goals call shows fit a quarter of its window, those that
/// take requests first, and the prompts keep the rest.
#[test]
fn the_areas_shown_fit_a_quarter_of_the_window() {
    let mut h = Home::new();
    h.summary.window_tokens = 2_000;
    let area = |name: String, status: &str| {
        json!({"kind": "area", "name": name, "status": status, "source": "owner-goals",
                "decided_by": "system"})
    };
    let mut ops: Vec<(String, Value)> = (0..30)
        .map(|i| {
            let name = format!("Area {i} {}", "x".repeat(200));
            (format!("fa_{i:012x}"), area(name, "done"))
        })
        .collect();
    ops.push(("fa_0000000000ff".into(), area("Parser".into(), "suggested")));
    crate::my_work_ops::record(h.s.home.path(), crate::my_work_focus::NAME, ops).unwrap();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    let (_, sent) = h.pass(&|_| nothing());
    let block = sent[0].split("## Focus areas\n").nth(1).unwrap();
    let block = block.split("## Prompts").next().unwrap();
    assert!(
        block.starts_with("- fa_0000000000ff suggested: Parser"),
        "{block}"
    );
    assert!(block.lines().count() < 31, "{}", block.lines().count());
    assert!(crate::budget::estimate(block) <= 500, "{block}");
    assert!(sent[0].contains("Make the parser fail fast on a bad header."));
}

/// F5's refusals: an area not shown, two priorities of one area (a dropped one frees its
/// area), and a new priority of no area named with a page word.
#[test]
fn an_area_unknown_or_taken_twice_refuses_the_answer() {
    let shown: HashSet<String> = ["fa_00000000000a".to_owned()].into();
    let now = crate::db::now_ms();
    let quote = |age: i64| Quote {
        seq: 1,
        ts: now - age,
        text: "x".into(),
    };
    let mut current = Item {
        id: "g1".into(),
        kind: "priority".into(),
        rank: 1,
        title: "Parser".into(),
        product: String::new(),
        asked: 1,
        quotes: vec![quote(DAY_MS)],
        area: Some("fa_00000000000a".into()),
    };
    let new = |title: &str, area: &str| {
        let mut n = item("priority", title, &[("P1", "x")]);
        n["area"] = json!(area);
        n
    };
    let run = |answer: &Value, current: &Item| {
        areas(
            answer,
            &shown,
            (std::slice::from_ref(current), now - WINDOW_MS),
        )
    };
    let fine = answer(json!([new("Lexer", "")]), json!([]), json!([]));
    assert_eq!(run(&fine, &current), None);
    let unknown = answer(
        json!([new("Lexer", "fa_00000000000f")]),
        json!([]),
        json!([]),
    );
    assert_eq!(run(&unknown, &current), Some("unanchored"));
    let twice = answer(
        json!([new("Lexer", "fa_00000000000a")]),
        json!([]),
        json!([]),
    );
    assert_eq!(run(&twice, &current), Some("shape"));
    let mut freed = twice.clone();
    freed["dropped"] = json!(["g1"]);
    assert_eq!(run(&freed, &current), None);
    // One whose quotes the window left goes, and takes no area; changed, it stays.
    let mut expired = current.clone();
    expired.quotes = vec![quote(WINDOW_MS + DAY_MS)];
    assert_eq!(run(&twice, &expired), None);
    let mut kept = twice.clone();
    kept["changed"] = json!([{"id": "g1", "title": "", "product": "", "asked": 1,
            "quote": {"prompt": "P1", "text": "x"}, "area": ""}]);
    assert_eq!(run(&kept, &expired), Some("shape"));
    let word = answer(json!([new("My Work", "")]), json!([]), json!([]));
    assert_eq!(run(&word, &current), Some("page_word"));
    current.kind = "ask".into();
    assert_eq!(
        run(&twice, &current),
        None,
        "an ask's area is no priority's"
    );
}

fn context(now: i64, current: Vec<Item>) -> Context {
    Context {
        current,
        shown: vec![ShownPrompt {
            n: 1,
            seq: 9,
            ts: now,
            text: "日本語のタイトルを試す".into(),
        }],
        through: 9,
        more: false,
        next_id: 1,
        now,
        rules: Rules::default(),
    }
}

/// M5: a quote older than the window goes, and an item left with none.
#[test]
fn quotes_older_than_31_days_drop_and_an_item_with_none_goes() {
    let now = crate::db::now_ms();
    let quote = |seq: i64, age: i64| Quote {
        seq,
        ts: now - age,
        text: format!("q{seq}"),
    };
    let goal = |id: &str, quotes: Vec<Quote>| Item {
        id: id.into(),
        kind: "priority".into(),
        rank: 1,
        title: id.into(),
        product: String::new(),
        asked: 1,
        quotes,
        area: None,
    };
    let current = vec![
        goal("g1", vec![quote(1, 40 * DAY_MS)]),
        goal("g2", vec![quote(2, 40 * DAY_MS), quote(3, DAY_MS)]),
    ];
    let kept = keep(&nothing(), &context(now, current));
    let kept: Vec<Item> = serde_json::from_value(kept["items"].clone()).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, "g2");
    assert_eq!(
        kept[0].quotes.iter().map(|q| q.seq).collect::<Vec<_>>(),
        [3]
    );
}

/// M4: lengths are characters: an 80-character Japanese title is kept, one of 81 is not.
#[test]
fn lengths_are_counted_in_characters() {
    let now = crate::db::now_ms();
    let title = |n: usize| "目".repeat(n);
    let new = json!([
        item("priority", &title(TITLE), &[("P1", "タイトル")]),
        item("ask", &title(TITLE + 1), &[("P1", "日本語")]),
    ]);
    let kept = keep(
        &answer(new, json!([]), json!([])),
        &context(now, Vec::new()),
    );
    let kept: Vec<Item> = serde_json::from_value(kept["items"].clone()).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title.chars().count(), TITLE);
}

/// A4: the largest answer, written in Japanese (0.8 tokens a character), fits 4,000 tokens.
#[test]
fn the_largest_japanese_answer_fits_the_answer_limit() {
    let jp = |n: usize| "目".repeat(n);
    let quote = json!({"prompt": "P10", "text": jp(QUOTE)});
    let new: Vec<Value> = (0..NEW)
        .map(|_| {
            json!({"kind": "priority", "title": jp(TITLE), "product": jp(PRODUCT),
                "asked": 10, "quotes": vec![quote.clone(); NEW_QUOTES]})
        })
        .collect();
    let changed: Vec<Value> = (0..CHANGED)
        .map(|i| {
            json!({"id": format!("g{i}"), "title": jp(TITLE), "product": jp(PRODUCT),
                "asked": 10, "quote": quote})
        })
        .collect();
    let ids: Vec<String> = (0..2 * EACH).map(|i| format!("g{i}")).collect();
    let largest = json!({"order": ids, "dropped": ids, "new": new, "changed": changed,
            "summary": jp(SUMMARY)});
    assert!(shape(&largest).is_none());
    let tokens = crate::budget::estimate(&largest.to_string());
    assert!(tokens <= 4_000, "{tokens}");
}

/// M5: a finished month keeps the last goals op written in it, read with no call; the month
/// under way is not one.
#[test]
fn a_finished_month_is_kept_with_no_call() {
    use chrono::TimeZone;
    let s = Store::new();
    let mut raw = crate::raw::open(s.home.path()).unwrap();
    let at = |y: i32, m: u32, d: u32| {
        chrono::Utc
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp_millis()
    };
    for (ts, title) in [
        (at(2026, 9, 3), "early"),
        (at(2026, 9, 28), "late"),
        (at(2026, 10, 2), "now"),
    ] {
        let goal = json!([{"id": "g1", "kind": "priority", "rank": 1, "title": title,
                "product": "", "asked": 1, "quotes": [{"seq": 1, "ts": ts, "text": "q"}]}]);
        let op = MyWorkOp {
            job: NAME.into(),
            about: day(ts, &chrono::Utc),
            entry: "fake".into(),
            ts,
            read: Read {
                spans: vec![],
                ops: vec![],
            },
            removed: vec![],
            fields: BTreeMap::from([("items".to_owned(), goal)]),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let none = HashSet::new();
    let goals = (NAME, None);
    let ops = crate::my_work_ops::shown(&k, goals, Some(raw.device()), &none, true, 64).unwrap();
    let months = months(&ops, at(2026, 10, 15), &chrono::Utc);
    let got: Vec<(String, Vec<String>)> = (months.iter())
        .map(|(m, k)| (m.clone(), titles(k)))
        .collect();
    assert_eq!(got, [("2026-09".to_owned(), vec!["late".to_owned()])]);
}

/// M6: goals are in no session start and no search: their text is found nowhere an agent is
/// given memory.
#[test]
fn goals_are_not_in_session_start_search_or_what_agents_are_given() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    // A decision after it, so that session start has something to show.
    h.s.decided(R, h.t + 5 * MINUTE, "Parser errors go to stderr.", &[]);
    h.pass(&|_| {
        let new = item("priority", "Zephyrine parser goal", &[("P1", "fail fast")]);
        answer(json!([new]), json!([]), json!([]))
    });
    assert_eq!(titles(&h.goals()[0]), ["Zephyrine parser goal"]);
    let found = h.s.query(&crate::search::b::Query {
        text: "Zephyrine".into(),
        caller: Some(R.into()),
        limit: 20,
        ..Default::default()
    });
    assert!(found.hits.is_empty(), "{:?}", found.hits);
    let rules = Rules::load(h.s.home.path()).unwrap();
    let start = crate::consumer::manifest::text(
        h.s.home.path(),
        &h.s.raw,
        R,
        "main",
        "other",
        &rules,
        crate::consumer::manifest::CAP,
        crate::consumer::manifest::INDEX,
        crate::db::now_ms(),
    )
    .unwrap()
    .expect("session start shows the decision");
    assert!(start.text.contains("stderr"), "{}", start.text);
    assert!(!start.text.contains("Zephyrine"), "{}", start.text);
}

/// S6: goals that read an excluded repository's prompts are hidden, and the next call is a first
/// run that sends none of them.
#[test]
fn goals_that_read_an_excluded_repository_are_hidden_and_sent_nowhere_until_rewritten() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.said("s2", B, 1, "Ship the billing export on Friday.");
    let (_, sent) = h.pass(&|_| {
        let new = item("priority", "Billing export", &[("P2", "billing export")]);
        answer(json!([new]), json!([]), json!([]))
    });
    assert!(sent[0].contains("billing export"), "{}", sent[0]);
    h.s.exclude(B);
    assert_eq!(h.hidden().len(), 1);
    assert_eq!(h.due().len(), 1, "a first run of what is left");
    let (_, sent) = h.pass(&|_| {
        let new = item("priority", "Fail fast", &[("P1", "fail fast")]);
        answer(json!([new]), json!([]), json!([]))
    });
    assert!(
        sent[0].contains("fail fast") && !sent[0].contains("billing"),
        "{}",
        sent[0]
    );
    assert!(sent[0].contains("No goals yet"), "{}", sent[0]);
    // The rewrite's item takes an id the hidden op never gave, so an id names one goal.
    assert_eq!(items(&h.goals()[0])[0].id, "g2");
}

/// M2: the current goals are the newest op with items, however many skips came after it.
#[test]
fn the_current_goals_survive_any_number_of_skips() {
    let mut h = Home::new();
    let seq = h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.pass(&|_| {
        let new = item("priority", "Fail fast", &[("P1", "fail fast")]);
        answer(json!([new]), json!([]), json!([]))
    });
    let now = crate::db::now_ms();
    let skips: Vec<(OpKind, Value)> = (0..70)
        .map(|i| {
            let op = MyWorkOp {
                job: NAME.into(),
                about: day(now, &chrono::Local),
                entry: String::new(),
                ts: now + i,
                read: Read {
                    spans: vec![(seq, seq)],
                    ops: vec![],
                },
                removed: vec![],
                fields: BTreeMap::new(),
                skipped: true,
                forgotten: None,
            };
            (OpKind::MyWork, serde_json::to_value(op).unwrap())
        })
        .collect();
    h.s.raw.append_ops(&skips).unwrap();
    h.later("s2", R, "The parser should also fail fast on a bad footer.");
    h.age();
    let (_, sent) = h.pass(&|_| nothing());
    assert!(
        sent[0].contains("g1 priority rank 1: Fail fast"),
        "{}",
        sent[0]
    );
}

/// A4: current goals that take all the room leave the next prompt cut short but read, so the
/// goals still update (Codex on #714).
#[test]
fn a_call_reads_a_prompt_however_much_room_the_current_goals_take() {
    let mut h = Home::new();
    h.summary.window_tokens = 1_000;
    let seq = h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    let now = crate::db::now_ms();
    let long = "目".repeat(TITLE);
    let goals: Vec<Item> = (0..2 * EACH)
        .map(|i| Item {
            id: format!("g{}", i + 1),
            kind: KINDS[i / EACH].into(),
            rank: u32::try_from(i % EACH).unwrap() + 1,
            title: long.clone(),
            product: long.clone(),
            asked: 1,
            quotes: vec![Quote {
                seq,
                ts: now,
                text: "fail fast".into(),
            }],
            area: None,
        })
        .collect();
    let op = MyWorkOp {
        job: NAME.into(),
        about: day(now, &chrono::Local),
        entry: String::new(),
        ts: now,
        read: Read {
            spans: vec![(seq, seq)],
            ops: vec![],
        },
        removed: vec![],
        fields: BTreeMap::from([
            ("items".to_owned(), serde_json::to_value(&goals).unwrap()),
            ("next_id".to_owned(), json!(2 * EACH + 1)),
        ]),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    h.s.run();
    h.later("s2", R, "The parser should also fail fast on a bad footer.");
    h.age();
    let (_, sent) = h.pass(&|_| nothing());
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].contains("P1 ") && sent[0].contains("The parser"),
        "{}",
        sent[0]
    );
    assert!(h.due().is_empty(), "{:?}", h.due());
}

/// R14: prompts recorded one after another but stamped out of order, the clock having stepped
/// back, are each read once when the budget cuts a call short: the prompts go in seq order, so
/// the last seq read covers every one before it.
#[test]
fn a_clock_step_neither_loses_nor_repeats_a_prompt() {
    let mut h = Home::new();
    // Each fills about half a call: two fit, the third waits.
    let long = |mark: &str| format!("{mark} {}", "目".repeat(3_000));
    for (mark, at) in [("Alpha", 10), ("Bravo", 0), ("Charlie", 5)] {
        h.said(mark, R, at, &long(mark));
    }
    let mut sent_all = Vec::new();
    while !h.due().is_empty() {
        let (_, sent) = h.pass(&|_| nothing());
        sent_all.extend(sent);
        assert!(sent_all.len() < 6, "{}", sent_all.len());
    }
    assert!(sent_all.len() >= 2, "{}", sent_all.len());
    for mark in ["Alpha", "Bravo", "Charlie"] {
        let n = sent_all.iter().filter(|p| p.contains(mark)).count();
        assert_eq!(n, 1, "{mark}");
    }
}

/// S6: an op built on a hidden goals op is hidden with it, and taking the exclusion back shows
/// both, the newer the current goals again.
#[test]
fn an_op_built_on_a_hidden_op_is_hidden_and_the_exclusion_taken_back_shows_both() {
    let mut h = Home::new();
    h.said("s2", B, 0, "Ship the billing export on Friday.");
    h.pass(&|_| {
        let new = item("priority", "Billing export", &[("P1", "billing export")]);
        answer(json!([new]), json!([]), json!([]))
    });
    h.later("s1", R, "Make the parser fail fast on a bad header.");
    h.age();
    h.pass(&|_| nothing());
    let ops = h.goals();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].op.read.ops, [ops[1].op_seq]);
    h.s.exclude(B);
    assert_eq!(h.hidden().len(), 2);
    h.s.raw.exclude(B, true).unwrap();
    assert!(h.hidden().is_empty());
    assert_eq!(titles(&h.goals()[0]), ["Billing export"]);
}

/// M2, O3: a call whose prompts the text tests all find an agent wrote sends nothing: what it
/// read is kept as a skip, and the job is not due again for them.
#[test]
fn a_read_of_agents_prompts_alone_is_kept_as_a_skip_with_no_call() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Make the parser fail fast on a bad header.");
    h.pass(&|_| nothing());
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    let input = json!({"command": format!("codex exec '{delegated}'")}).to_string();
    let call = json!({"tool": "Bash", "input": input, "output": "ok", "failed": false});
    let now = crate::db::now_ms();
    h.s.event("tool", "s3", (R, "main"), now + 1_000, call);
    h.s.said("s4", R, now + 2_000, &delegated);
    h.age();
    assert_eq!(h.due().len(), 1);
    let (_, sent) = h.pass(&|_| panic!("no call"));
    assert!(sent.is_empty());
    assert!(h.goals()[0].op.skipped);
    assert!(h.due().is_empty(), "{:?}", h.due());
}
