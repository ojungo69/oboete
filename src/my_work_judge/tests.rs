use super::*;
use std::cell::RefCell;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use rusqlite::Connection;

use crate::config::{self, Summary};
use crate::curate::{Phase, Reading, Reads};
use crate::my_work_group::Grouping;
use crate::my_work_run::{Job, Runner};
use crate::provider::tests::{request_body, serve_each};
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
use crate::raw::OpKind;
use crate::redact::Rules;
use crate::search::b::fixture::Store;

const R: &str = "github.com/o/r";
const MINUTE: i64 = 60_000;

/// A home whose records start an hour ago, with a judge whose key is set.
struct Home {
    s: Store,
    t: i64,
    judge: config::Judge,
}

impl Home {
    fn new() -> Self {
        let s = Store::new();
        let key_file = s.home.path().join("TYPESAFE_KEY.md");
        std::fs::write(&key_file, "# a test key\nTESTKEY_for_the_stub\n").unwrap();
        // Nothing a test does not serve answers: no test reaches Typesafe.
        crate::judge::AT.set(Some("http://127.0.0.1:9".into()));
        let h = Self {
            s,
            t: crate::db::now_ms() - 60 * MINUTE,
            judge: config::Judge {
                enabled: true,
                destination: config::Destination::Jev,
                key_file: Some(key_file),
                cloudflare_key_file: None,
                cloudflare_account_id: None,
                liquid_key_file: None,
                model: "jev-latest".into(),
                timeout_s: 5,
                daily_budget: 300,
            },
        };
        h.config("");
        h
    }

    /// The home's config.toml: `[judge]` as the runner is given it, then `extra`.
    fn config(&self, extra: &str) {
        let key_file = self.judge.key_file.as_ref().unwrap();
        let table = format!("[judge]\nkey_file = '{}'\n", key_file.display());
        let path = self.s.home.path().join("config.toml");
        std::fs::write(path, format!("{table}{extra}")).unwrap();
    }

    fn said(&mut self, session: &str, at: i64, text: &str) -> i64 {
        self.s.said(session, R, self.t + at * MINUTE, text)
    }

    /// A grouping op that makes `members` one request titled `title`, as the model's.
    fn grouped(&mut self, members: &[&str], title: &str) {
        let keys: Vec<Value> = members.iter().map(|m| json!(["claude", m])).collect();
        let op = MyWorkOp {
            job: "group".into(),
            about: R.into(),
            entry: "fake".into(),
            ts: crate::db::now_ms(),
            read: Read::default(),
            removed: vec![],
            fields: BTreeMap::from([
                (
                    "groups".to_owned(),
                    json!([{"conversations": keys, "title": title, "links": []}]),
                ),
                ("ignored".to_owned(), json!([])),
            ]),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }

    /// Grouping's ops as long ago as `minutes` minutes, as C5 counts them.
    fn age(&self, minutes: i64) {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        k.execute(
            "UPDATE my_work SET ts = ts - ?1 WHERE job = 'group'",
            [minutes * MINUTE],
        )
        .unwrap();
    }

    /// One pass of grouping with the judge `on` (its stub at `judge::AT`), the chain answering
    /// `answer`: what it did, and the prompts the chain was sent.
    fn pass(&mut self, on: bool, answer: &dyn Fn(&str) -> Value) -> (Phase, Vec<String>) {
        self.pass_with(on, Box::new(Grouping::default()), answer)
    }

    fn pass_with(
        &mut self,
        on: bool,
        job: Box<dyn Job>,
        answer: &dyn Fn(&str) -> Value,
    ) -> (Phase, Vec<String>) {
        self.s.run();
        self.run_pass(on, job, answer)
    }

    /// A pass with no run of the consumers first: what they last applied, as it stands.
    fn run_pass(
        &mut self,
        on: bool,
        job: Box<dyn Job>,
        answer: &dyn Fn(&str) -> Value,
    ) -> (Phase, Vec<String>) {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let db = crate::providers_db::open(self.s.home.path()).unwrap();
        let (rules, summary) = (Rules::load(self.s.home.path()).unwrap(), Summary::default());
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
            vec![job],
            Box::new(call),
            (&rules, &summary, "chain"),
        )
        .judge(on.then_some((&self.judge, 5.0)));
        let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
        drop(runner);
        self.s.run();
        (phase, sent.into_inner())
    }

    fn groups(&self) -> Groups {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        crate::my_work_group::groups(&self.s.raw, &k, &reading).unwrap()
    }

    /// The judge's ops, the oldest first.
    fn judged(&self) -> Vec<MyWorkOp> {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        if !crate::consumer::manifest::exists(&k, "table", "my_work").unwrap() {
            return Vec::new();
        }
        let mut st = k
            .prepare(
                "SELECT about, entry, read, fields FROM my_work WHERE job = 'judge'
                     ORDER BY ts, op_seq",
            )
            .unwrap();
        st.query_map([], |r| {
            Ok(MyWorkOp {
                job: NAME.into(),
                about: r.get(0)?,
                entry: r.get(1)?,
                ts: 0,
                read: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
                removed: vec![],
                fields: serde_json::from_str(&r.get::<_, String>(3)?).unwrap(),
                skipped: false,
                forgotten: None,
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    fn ledger(&self) -> Vec<(String, String, String)> {
        let db = Connection::open(self.s.home.path().join("providers.db")).unwrap();
        let mut st = db
            .prepare("SELECT provider, role, outcome FROM provider_calls ORDER BY id")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

/// A well-formed binding, for an answer refused before its binding is compared (W3b-2).
const ANY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn key(session: &str) -> Key {
    ("claude".to_owned(), session.to_owned())
}

/// The stub answering `answers` in the endpoint's place.
fn stub(answers: Vec<(&'static str, Vec<u8>)>) -> Receiver<String> {
    let (url, rx) = serve_each(answers, "");
    crate::judge::AT.set(Some(url));
    rx
}

/// The bodies of the requests the stub received.
fn bodies(rx: &Receiver<String>) -> Vec<Value> {
    std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(300)).ok())
        .map(|r| request_body(&r))
        .collect()
}

fn picked(choice: &str, p: f64) -> (&'static str, Vec<u8>) {
    let body = json!({"model": "jev-1.14.0",
            "answers": {"pick": {"type": "choice", "choice": choice, "confidence": p,
                "probabilities": {choice: p}}},
            "usage": {"input_tokens": 9_000, "output_tokens": 12}});
    ("200 OK", body.to_string().into_bytes())
}

fn checked(score: f64) -> (&'static str, Vec<u8>) {
    let body = json!({"model": "jev-1.14.0",
            "answers": {"same": {"type": "noul", "noul": score}},
            "usage": {"input_tokens": 1_100, "output_tokens": 4}});
    ("200 OK", body.to_string().into_bytes())
}

fn grouping(groups: Value) -> Value {
    json!({"groups": groups, "ignored": []})
}

/// J3: with no judge, nothing is judged: grouping asks for each conversation as before.
#[test]
fn with_the_judge_off_grouping_decides_as_before() {
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    let answer =
        |_: &str| grouping(json!([{"conversations": ["C1"], "title": "Parser", "links": []}]));
    let (phase, prompts) = h.pass(false, &answer);
    assert_eq!((phase, prompts.len()), (Phase::Covered, 1));
    assert!(h.judged().is_empty());
    assert_eq!(h.groups().of.get(&key("s1")), Some(&key("s1")));
    // Off by its switch, or with no key on line 2, the same.
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.judge.enabled = false;
    assert_eq!(h.pass(true, &answer).1.len(), 1);
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    std::fs::write(h.judge.key_file.as_ref().unwrap(), "# no key\n").unwrap();
    assert_eq!(h.pass(true, &answer).1.len(), 1);
    assert!(h.judged().is_empty() && h.ledger().is_empty());
}

/// J3 (claude-mem's rules judge): a conversation one proven link gives to a single open request
/// joins it with no call, Jev's or the chain's; the tool record that proves it is read.
#[test]
fn a_single_proven_link_joins_with_no_call() {
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    let input = json!({"command": format!("codex exec '{delegated}'")}).to_string();
    let call = json!({"tool": "Bash", "input": input, "output": "ok", "failed": false});
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    let record = h.s.event("tool", "s1", (R, "main"), h.t + 2 * MINUTE, call);
    let opening = h.said("s2", 3, &delegated);
    let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!((phase, prompts.len()), (Phase::Covered, 0));
    assert!(h.ledger().is_empty(), "{:?}", h.ledger());
    let ops = h.judged();
    assert_eq!(ops.len(), 1);
    let op = &ops[0];
    assert_eq!((op.about.as_str(), op.entry.as_str()), (R, RULES));
    assert_eq!(op.fields["outcome"], "rules");
    assert_eq!(op.fields["conversation"], json!(["claude", "s2"]));
    let link = &op.fields["groups"][0]["links"][0];
    assert_eq!(link["kind"], "rules");
    assert_eq!(
        link["conversations"],
        json!([["claude", "s1"], ["claude", "s2"]])
    );
    assert!(op.read.spans.contains(&(opening, opening)), "{:?}", op.read);
    assert!(op.read.spans.contains(&(record, record)), "{:?}", op.read);
    let groups = h.groups();
    assert_eq!(groups.of.get(&key("s2")), Some(&key("s1")));
    assert!(groups.asked.contains(&key("s2")));
}

/// J3: with no open request in its repository, a conversation is a request of its own, with no
/// call; one judged is not judged again, and grouping does not ask for it.
#[test]
fn with_no_open_request_a_conversation_is_its_own_with_no_call() {
    let mut h = Home::new();
    let opening = h.said("s1", 0, "Build the parser.");
    let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!((phase, prompts.len()), (Phase::Covered, 0));
    let ops = h.judged();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].fields["outcome"], "alone");
    assert_eq!(ops[0].entry, crate::judge::NAME);
    assert_eq!(ops[0].read.spans, vec![(opening, opening)]);
    assert_eq!(
        ops[0].fields["groups"],
        json!([{"conversations": [["claude", "s1"]], "title": null, "links": []}])
    );
    assert_eq!(h.groups().of.get(&key("s1")), Some(&key("s1")));
    let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!((phase, prompts.len()), (Phase::Idle, 0));
    assert_eq!(h.judged().len(), 1);
    assert!(h.ledger().is_empty());
}

/// J7: the judge sends where `[judge]` says: a decision made through d1 on this PC is its op's
/// entry, its evidence names it, and its calls are its own rows in the ledger.
#[test]
fn a_decision_names_the_destination_that_made_it() {
    let mut h = Home::new();
    h.judge.destination = config::Destination::D1Local;
    h.config("destination = \"d1-local\"\n");
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "The parser drops a trailing comma.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!(bodies(&rx).len(), 2);
    let op = &h.judged()[0];
    assert_eq!(op.entry, "judge:d1-local");
    // In Japanese, as the summaries' language is here (decision 81).
    assert_eq!(
        op.fields["groups"][0]["links"][0]["evidence"],
        "d1 on this PC: 開いている依頼 1 件の中から選びました (確率 0.94)。「その依頼の続き」の確認は 0.90 でした。"
    );
    let row = ("judge:d1-local".into(), "judge".into(), "ok".into());
    assert_eq!(h.ledger(), vec![row.clone(), row]);
}

/// J3: Jev picks among the open requests, then checks its pick; the check's score is the band:
/// from 0.8 it joins, a link of kind `judge` with the scores as its evidence, and grouping is not
/// asked; from 0.5 its own request with a suggestion to join the pick; below, or `none`, its own.
#[test]
fn the_check_s_score_joins_suggests_or_leaves_it_alone() {
    for (score, outcome) in [(0.9, "joined"), (0.6, "suggested"), (0.3, "own")] {
        let mut h = Home::new();
        h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], "Parser");
        h.said("s2", 5, "The parser drops a trailing comma.");
        let rx = stub(vec![picked("r1", 0.94), checked(score)]);
        let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked"));
        assert_eq!((phase, prompts.len()), (Phase::Covered, 0), "{score}");
        assert_eq!(bodies(&rx).len(), 2, "{score}");
        let ops = h.judged();
        let op = &ops[0];
        assert_eq!(op.fields["outcome"], outcome);
        let groups = h.groups();
        match outcome {
            "joined" => {
                assert_eq!(groups.of.get(&key("s2")), Some(&key("s1")));
                let link = &op.fields["groups"][0]["links"][0];
                assert_eq!(link["kind"], "judge");
                assert_eq!(
                    link["evidence"],
                    "Jev: 開いている依頼 1 件の中から選びました (確率 0.94)。「その依頼の続き」の確認は 0.90 でした。"
                );
                assert!(!op.fields.contains_key("suggestion"));
            }
            "suggested" => {
                assert_eq!(groups.of.get(&key("s2")), Some(&key("s2")));
                assert_eq!(
                    op.fields["suggestion"],
                    json!({"request": ["claude", "s1"], "score": 0.6})
                );
            }
            _ => {
                assert_eq!(groups.of.get(&key("s2")), Some(&key("s2")));
                assert!(!op.fields.contains_key("suggestion"));
            }
        }
        assert_eq!(
            h.ledger(),
            vec![
                ("judge:jev".into(), "judge".into(), "ok".into()),
                ("judge:jev".into(), "judge".into(), "ok".into())
            ]
        );
        // The judge's calls are its own: My Work's daily bound does not count them.
        let db = crate::providers_db::open(h.s.home.path()).unwrap();
        assert_eq!(
            crate::providers_db::my_work_calls_in_a_day(&db).unwrap().0,
            0
        );
    }
    // `none`: its own request, and no check.
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "Write the release notes.");
    let rx = stub(vec![picked("none", 0.8)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!(bodies(&rx).len(), 1);
    assert_eq!(h.judged()[0].fields["outcome"], "none");
    assert_eq!(h.groups().of.get(&key("s2")), Some(&key("s2")));
}

/// J2, J3: what Jev is shown, as claude-mem shows it: the conversation's repository, its start
/// and its first asks and card titles, cut and gated; each request's the same and its title, by
/// a label that is a key and never recorded text; claude-mem's questions word for word.
#[test]
fn jev_is_shown_what_claude_mem_shows_it_gated() {
    let mut h = Home::new();
    h.config("[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n");
    let first = h.said("s1", 0, "Build the parser.");
    h.s.cards(
        first,
        first,
        json!([{"type": "feature", "title": "Parser built", "narrative": "Done."}]),
        false,
    );
    h.grouped(&["s1"], "Parser");
    let long = format!("{} tail", "x".repeat(300));
    h.said(
        "s2",
        5,
        "Fix the   parser's\n comma for codename-blue.\n# Attached files\nsecret.txt",
    );
    h.said("s2", 6, &long);
    h.said("s2", 7, "Then add a test for it.");
    let last = h.said("s2", 8, "And a fourth ask, not shown.");
    let cards: Vec<Value> = (0..9)
        .map(|i| {
            let title = if i == 0 {
                "y".repeat(130)
            } else {
                format!("Card {i}")
            };
            json!({"type": "change", "title": title, "narrative": "Done."})
        })
        .collect();
    h.s.cards(last, last, json!(cards), false);
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    let sent = bodies(&rx);
    let (pick, check) = (&sent[0], &sent[1]);
    let conversation = &pick["state"]["conversation"];
    assert_eq!(conversation["worktree"], R);
    assert_eq!(
        conversation["started"],
        crate::db::utc(h.t + 5 * MINUTE)[..16].replacen(' ', "T", 1)
    );
    let asks = conversation["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 3, "{asks:?}");
    assert!(
        asks[0]
            .as_str()
            .unwrap()
            .starts_with("Fix the parser's comma for "),
        "{asks:?}"
    );
    assert!(!pick.to_string().contains("codename-blue"), "{pick}");
    assert!(!pick.to_string().contains("secret.txt"), "{pick}");
    assert_eq!(asks[1].as_str().unwrap().chars().count(), 250);
    let recorded = conversation["agent_recorded"].as_array().unwrap();
    assert_eq!(recorded.len(), 8, "{recorded:?}");
    assert_eq!(recorded[0].as_str().unwrap().chars().count(), 120);
    assert_eq!(recorded[1], "Card 1");
    assert_eq!(recorded[7], "Card 7");
    assert_eq!(pick["state"]["requests"]["r1"]["title"], "Parser");
    let fields: Vec<&String> = pick["state"]["requests"]["r1"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        fields,
        ["title", "worktree", "started", "asks", "agent_recorded"]
    );
    assert_eq!(
        pick["state"]["requests"]["r1"]["agent_recorded"],
        json!(["Parser built"])
    );
    assert_eq!(
        pick["state"]["requests"]["r1"]["asks"],
        json!(["Build the parser."])
    );
    assert_eq!(pick["questions"]["pick"]["instructions"], PICK);
    assert_eq!(
        pick["questions"]["pick"]["criteria"],
        json!({"r1": "`requests.r1`: Parser", "none": NONE})
    );
    assert_eq!(pick["model"], "jev-latest");
    assert_eq!(check["state"]["request"], pick["state"]["requests"]["r1"]);
    assert_eq!(check["state"]["conversation"], *conversation);
    assert_eq!(
        check["questions"],
        json!({"same": {"type": "noul", "instructions": CHECK,
                "criteria": {"true": "Same work", "false": "Different work"}}})
    );
}

/// J3: the pick offers the 19 most recently active open requests, the newest first, and says how
/// many it left out.
#[test]
fn the_pick_offers_the_19_most_recently_active_requests() {
    let mut h = Home::new();
    for i in 0..21 {
        h.said(&format!("o{i}"), i, &format!("Build part {i} of the site."));
        h.grouped(&[&format!("o{i}")], &format!("Part {i}"));
    }
    h.said("s2", 30, "Fix the footer.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    let pick = &bodies(&rx)[0];
    let requests = pick["state"]["requests"].as_object().unwrap();
    assert_eq!(requests.len(), 19);
    assert_eq!(requests["r1"]["title"], "Part 20");
    assert_eq!(requests["r19"]["title"], "Part 2");
    let link = &h.judged()[0].fields["groups"][0]["links"][0];
    assert!(
        (link["evidence"].as_str().unwrap())
            .contains("開いている依頼 21 件のうち最近動いた 19 件の中から選びました"),
        "{link}"
    );
    assert_eq!(h.groups().of.get(&key("s2")), Some(&key("o20")));
}

/// J1, J3: a call that fails is not made again: the conversation is left to grouping, whose next
/// pass asks for it, and the judge never asks for it again.
#[test]
fn a_failed_call_leaves_the_conversation_to_grouping() {
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.s.run();
    h.age(31);
    h.said("s2", 5, "The parser drops a trailing comma.");
    let rx = stub(vec![("500 Internal Server Error", b"{}".to_vec())]);
    let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked yet"));
    assert_eq!((phase, prompts.len()), (Phase::Covered, 0));
    assert_eq!(bodies(&rx).len(), 1);
    let ops = h.judged();
    assert_eq!(ops[0].fields["outcome"], "error");
    assert!(!ops[0].fields.contains_key("groups"));
    let groups = h.groups();
    assert!(groups.asked.contains(&key("s2")));
    assert!(!groups.decided(&key("s2")));
    let answer = |_: &str| {
        grouping(json!([{"conversations": ["R1", "C1"], "title": "Parser",
                "links": [{"conversations": ["R1", "C1"], "evidence": "The same parser."}]}]))
    };
    let (phase, prompts) = h.pass(true, &answer);
    assert_eq!((phase, prompts.len()), (Phase::Covered, 1));
    assert!(prompts[0].contains("The parser drops a trailing comma."));
    assert_eq!(h.groups().of.get(&key("s2")), Some(&key("s1")));
    assert_eq!(h.judged().len(), 1);
    assert_eq!(
        h.ledger(),
        vec![("judge:jev".into(), "judge".into(), "error".into())]
    );
}

/// J3: a decision too big for an op's cap would be kept as a skip, naming no conversation, and
/// asked for again each round: before anything is paid, the conversation is left to grouping.
#[test]
fn a_decision_too_big_to_keep_is_left_to_grouping_unpaid() {
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    let opening = h.said("s2", 5, "The parser drops a trailing comma.");
    CAP.set(Some(1_000));
    let (phase, prompts) = h.pass(true, &|_| panic!("the chain is not asked yet"));
    CAP.set(None);
    assert_eq!((phase, prompts.len()), (Phase::Covered, 0));
    assert!(h.ledger().is_empty(), "{:?}", h.ledger());
    let ops = h.judged();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].fields["outcome"], "too_big");
    assert_eq!(ops[0].read.spans, vec![(opening, opening)]);
    let groups = h.groups();
    assert!(groups.asked.contains(&key("s2")) && !groups.decided(&key("s2")));
}

/// J3: one conversation a round, the earliest first: the next round reads the request the first
/// became, which the second is offered.
#[test]
fn one_conversation_a_round_and_the_next_is_offered_the_first() {
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.said("s2", 5, "The parser drops a trailing comma.");
    let (phase, _) = h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!(phase, Phase::Covered);
    let ops = h.judged();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].fields["conversation"], json!(["claude", "s1"]));
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    let pick = &bodies(&rx)[0];
    assert_eq!(
        pick["state"]["requests"]["r1"]["asks"],
        json!(["Build the parser."])
    );
    assert_eq!(h.groups().of.get(&key("s2")), Some(&key("s1")));
}

/// J1 (A5): an exclusion written after the pass read the stores, before the judge's request,
/// keeps the request from going and the pass from keeping anything; the reservation goes.
#[test]
fn an_exclusion_written_before_the_request_keeps_it_from_going() {
    struct Excluding(std::path::PathBuf);
    impl Job for Excluding {
        fn name(&self) -> &'static str {
            "group"
        }
        fn reread(&self) {}
        fn due(&self, _: &Stores, _: i64) -> Result<Vec<crate::my_work_run::Due>> {
            Ok(Vec::new())
        }
        fn ask(&self, _: &Stores, _: &str, _: i64) -> Result<Option<crate::my_work_run::Ask>> {
            Ok(None)
        }
        fn judged(&self, _: &Stores, judge: &mut Judging, _: i64) -> Result<Vec<MyWorkOp>> {
            crate::raw::open(&self.0)
                .unwrap()
                .exclude("elsewhere", false)
                .unwrap();
            (judge.ask)(Ask {
                span: "judge r",
                state: json!({"conversation": {"asks": ["Build it."]}}),
                questions: json!({"same": {"type": "noul", "instructions": CHECK,
                        "criteria": {"true": "Same work", "false": "Different work"}}}),
                repos: &[],
            })?;
            panic!("the gate lets nothing go")
        }
    }
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    let rx = stub(vec![checked(0.9)]);
    let job = Excluding(h.s.home.path().to_owned());
    let (phase, _) = h.pass_with(true, Box::new(job), &|_| panic!("no call"));
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert!(bodies(&rx).is_empty());
    assert!(h.judged().is_empty());
    assert!(h.ledger().is_empty(), "{:?}", h.ledger());
}

/// S2, S6: the judge's op reads what it showed, this device's prompt records and the card and
/// grouping ops behind them, so an exclusion of the repository leaves it out with the decision
/// it made, as a grouping op's.
#[test]
fn the_op_reads_what_it_showed_and_an_exclusion_leaves_it_out() {
    let mut h = Home::new();
    let first = h.said("s1", 0, "Build the parser.");
    let window = h.s.cards(
        first,
        first,
        json!([{"type": "feature", "title": "Parser built", "narrative": "Done."}]),
        false,
    );
    h.grouped(&["s1"], "Parser");
    let opening = h.said("s2", 5, "The parser drops a trailing comma.");
    let later = h.said("s2", 6, "Add a test for it too.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!(bodies(&rx).len(), 2);
    let op = &h.judged()[0];
    for seq in [first, opening, later] {
        assert!(op.read.spans.contains(&(seq, seq)), "{seq} {:?}", op.read);
    }
    let card_op: i64 = window[0].split('.').next().unwrap().parse().unwrap();
    assert!(op.read.ops.contains(&card_op), "{:?}", op.read);
    let group_op: i64 = (crate::knowledge::open(h.s.home.path()).unwrap())
        .query_row("SELECT op_seq FROM my_work WHERE job = 'group'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(op.read.ops.contains(&group_op), "{:?}", op.read);
    assert_eq!(h.groups().of.get(&key("s2")), Some(&key("s1")));
    h.s.raw.exclude(R, false).unwrap();
    h.s.run();
    let groups = h.groups();
    assert!(!groups.decided(&key("s2")) && !groups.asked.contains(&key("s2")));
}

/// J3: a repository excluded after the round read it, before the judge is asked: nothing goes
/// and nothing is kept of the judge, so the next round's read leaves the conversation out.
#[test]
fn an_exclusion_found_on_the_way_keeps_nothing_of_the_judge() {
    struct Racing(Grouping, std::path::PathBuf);
    impl Job for Racing {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn reread(&self) {
            self.0.reread()
        }
        fn due(&self, s: &Stores, now: i64) -> Result<Vec<crate::my_work_run::Due>> {
            self.0.due(s, now)
        }
        fn ask(
            &self,
            s: &Stores,
            about: &str,
            now: i64,
        ) -> Result<Option<crate::my_work_run::Ask>> {
            self.0.ask(s, about, now)
        }
        /// The round's read first, then the exclusion, then the judge.
        fn judged(&self, s: &Stores, judge: &mut Judging, now: i64) -> Result<Vec<MyWorkOp>> {
            self.0.due(s, now)?;
            crate::raw::open(&self.1)
                .unwrap()
                .exclude(R, false)
                .unwrap();
            self.0.judged(s, judge, now)
        }
    }
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "The parser drops a trailing comma.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    let job = Racing(Grouping::default(), h.s.home.path().to_owned());
    h.pass_with(true, Box::new(job), &|_| panic!("the chain is not asked"));
    assert!(bodies(&rx).is_empty());
    assert!(h.judged().is_empty());
    assert!(h.ledger().is_empty(), "{:?}", h.ledger());
}

/// A5: what Jev is shown is gated as it was recorded, before claude-mem's clean collapses its
/// lines: a rule over two lines, written after the record was read and before a rescan reached
/// it, still keeps its secret out.
#[test]
fn a_rule_over_lines_is_kept_before_the_text_is_cleaned() {
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "Fix the parser.\nowner\nACME-7731 is the account.");
    h.s.run();
    h.config(
        "[redaction]\nextra_rules = [{ id = \"owner\", regex = 'owner\\n(ACME-[0-9]+)', \
             secret_group = 1 }]\n",
    );
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    h.run_pass(true, Box::new(Grouping::default()), &|_| panic!("no call"));
    let sent = bodies(&rx);
    assert_eq!(sent.len(), 2);
    assert!(!sent[0].to_string().contains("ACME-7731"), "{}", sent[0]);
    assert!(
        sent[0].to_string().contains("Fix the parser. owner"),
        "{}",
        sent[0]
    );
}

/// J1: the switch turned off between the pick and the check: the check does not go, and nothing
/// is kept of the pick.
#[test]
fn the_switch_turned_off_on_the_way_sends_and_keeps_nothing_more() {
    struct Turning(Grouping, std::path::PathBuf);
    impl Job for Turning {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn reread(&self) {
            self.0.reread()
        }
        fn due(&self, s: &Stores, now: i64) -> Result<Vec<crate::my_work_run::Due>> {
            self.0.due(s, now)
        }
        fn ask(
            &self,
            s: &Stores,
            about: &str,
            now: i64,
        ) -> Result<Option<crate::my_work_run::Ask>> {
            self.0.ask(s, about, now)
        }
        /// The switch off once the first request is answered.
        fn judged(&self, s: &Stores, judge: &mut Judging, now: i64) -> Result<Vec<MyWorkOp>> {
            let (entry, label) = (judge.entry, judge.label);
            let mut turning = |ask: Ask<'_>| {
                let asked = (judge.ask)(ask);
                let config = self.1.join("config.toml");
                let text = std::fs::read_to_string(&config).unwrap();
                std::fs::write(&config, format!("{text}enabled = false\n")).unwrap();
                asked
            };
            let mut turning = Judging {
                entry,
                label,
                ask: &mut turning,
            };
            self.0.judged(s, &mut turning, now)
        }
    }
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "The parser drops a trailing comma.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.9)]);
    let job = Turning(Grouping::default(), h.s.home.path().to_owned());
    let (phase, _) = h.pass_with(true, Box::new(job), &|_| panic!("no call"));
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert_eq!(bodies(&rx).len(), 1);
    assert!(h.judged().is_empty());
    assert_eq!(
        h.ledger(),
        vec![("judge:jev".into(), "judge".into(), "ok".into())]
    );
}

/// J1: the switch turned off before a decision that needs no call is kept: the round keeps
/// nothing, as it keeps nothing of a pick.
#[test]
fn the_switch_turned_off_before_a_decision_is_kept_keeps_nothing() {
    struct Off(Grouping, std::path::PathBuf);
    impl Job for Off {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn reread(&self) {
            self.0.reread()
        }
        fn due(&self, s: &Stores, now: i64) -> Result<Vec<crate::my_work_run::Due>> {
            self.0.due(s, now)
        }
        fn ask(
            &self,
            s: &Stores,
            about: &str,
            now: i64,
        ) -> Result<Option<crate::my_work_run::Ask>> {
            self.0.ask(s, about, now)
        }
        fn judged(&self, s: &Stores, judge: &mut Judging, now: i64) -> Result<Vec<MyWorkOp>> {
            let config = self.1.join("config.toml");
            let text = std::fs::read_to_string(&config).unwrap();
            std::fs::write(&config, format!("{text}enabled = false\n")).unwrap();
            self.0.judged(s, judge, now)
        }
    }
    let mut h = Home::new();
    h.said("s1", 0, "Build the parser.");
    let job = Off(Grouping::default(), h.s.home.path().to_owned());
    let (phase, _) = h.pass_with(true, Box::new(job), &|_| panic!("no call"));
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert!(h.judged().is_empty());
    assert!(!h.groups().decided(&key("s1")));
}

/// J4: `s2` suggested to join `s1`'s request, Parser, at 0.6: the stub that served it, and
/// the seq of `s1`'s ask, which the suggestion was made on.
fn suggested(h: &mut Home) -> (Receiver<String>, i64) {
    let ask = h.said("s1", 0, "Build the parser.");
    h.grouped(&["s1"], "Parser");
    h.said("s2", 5, "The parser drops a trailing comma.");
    let rx = stub(vec![picked("r1", 0.94), checked(0.6)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    (rx, ask)
}

impl Home {
    /// My Work's requests as the page reads them, through the gate, the ops written so far
    /// applied first.
    fn page(&self) -> Vec<Value> {
        self.s.run();
        let path = self.s.home.path();
        let v = crate::my_work::read(path, crate::my_work::Range::All, crate::db::now_ms());
        let v = crate::view::answered("my-work", v.unwrap());
        v["requests"].as_array().unwrap().clone()
    }

    /// The request the page shows conversation `session` in.
    fn request_of(&self, session: &str) -> Value {
        let id = format!("c:claude:{session}");
        (self.page().into_iter())
            .find(|r| (r["sessions"].as_array().unwrap().iter()).any(|s| s["id"] == id))
            .unwrap()
    }

    /// The owner's `action` on the request the page shows `session` in, as the page posts it:
    /// with the binding of the suggestion it shows (W3b-2).
    fn answer_on(&self, session: &str, action: &str) -> std::result::Result<Value, (u16, &str)> {
        let shown = self.request_of(session);
        let body = json!({"request": shown["id"], "action": action,
            "binding": shown["suggestion"]["binding"]});
        self.post(body)
    }

    /// The card the page shows conversation `session`'s request as (W3b-2): its id and its
    /// suggestion's binding, which an answer read from it posts, whatever has changed since.
    fn card(&self, session: &str) -> Value {
        let shown = self.request_of(session);
        json!({"request": shown["id"], "binding": shown["suggestion"]["binding"]})
    }

    /// The owner's `action` on `card`, as the page posts it.
    fn answer(&self, card: &Value, action: &str) -> std::result::Result<Value, (u16, &str)> {
        let mut body = card.clone();
        body["action"] = json!(action);
        self.post(body)
    }

    fn post(&self, body: Value) -> std::result::Result<Value, (u16, &str)> {
        let body = serde_json::to_vec(&body).unwrap();
        act(self.s.home.path(), &Mutex::new(()), &body, &|| true).map_err(|r| (r.status, r.code))
    }

    /// A grouping op that leaves `session` out of every request, as the model's.
    fn ignored(&mut self, session: &str) {
        let op = MyWorkOp {
            job: "group".into(),
            about: R.into(),
            entry: "fake".into(),
            ts: crate::db::now_ms(),
            read: Read::default(),
            removed: vec![],
            fields: BTreeMap::from([
                ("groups".to_owned(), json!([])),
                (
                    "ignored".to_owned(),
                    json!([{"conversation": ["claude", session], "reason": "A test."}]),
                ),
            ]),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }

    /// A work state write in `session` naming request `id`, as the server appends it and the
    /// agent's call records it 20 ms after (W3b-1).
    fn named(&mut self, session: &str, id: &str) {
        let write = self.naming(id);
        self.called(session, write);
    }

    /// A work state write naming request `id`, as the server appends it: its time and fields.
    fn naming(&mut self, id: &str) -> (i64, serde_json::Map<String, Value>) {
        let fields = json!({"task": "notes", "request_id": id, "request": "Write the notes",
            "status": "doing"});
        let fields = fields.as_object().unwrap().clone();
        let seq = (self.s.raw.work_state(R, "notes", &fields, None)).unwrap();
        let ts: i64 = Connection::open(self.s.home.path().join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT ts FROM ops WHERE device = ?1 AND op_seq = ?2",
                rusqlite::params![self.s.raw.device(), seq],
                |r| r.get(0),
            )
            .unwrap();
        (ts, fields)
    }

    /// The agent's call in `session` that sent the write `naming` made, recorded 20 ms after it.
    fn called(&mut self, session: &str, (ts, fields): (i64, serde_json::Map<String, Value>)) {
        let input = json!({"list": "notes", "fields": fields}).to_string();
        let body = json!({"tool": "mcp__oboete__work_state_write", "input": input,
            "output": "Saved.", "failed": false});
        self.s.event("tool", session, (R, "main"), ts + 20, body);
    }

    /// The judge's ops raw.db holds, applied or not (W3b-1: what an answer wrote, if any).
    fn recorded(&self) -> i64 {
        Connection::open(self.s.home.path().join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM ops WHERE type = 'my_work'
                   AND json_extract(body, '$.job') = 'judge'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// The home after the record `seq` of this device was forgotten and purged.
    fn forgot(self, seq: i64) -> Self {
        let Home { s, t, judge } = self;
        let Store { home, raw } = s;
        let device = raw.device().to_owned();
        drop(raw);
        let target = crate::forget::Target::Record { device, seq };
        let preview = crate::forget::preview(home.path(), target).unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        crate::forget::purge(home.path()).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let h = Home {
            s: Store { home, raw },
            t,
            judge,
        };
        h.s.run();
        h
    }
}

/// J4: the request of a conversation the judge suggested names the request its pick is in,
/// with that request's id as the page has it and its title; no other request names one. #777:
/// and the conversation it is about, by the id the page has it, which a Join follows.
#[test]
fn a_suggestion_shows_on_its_conversations_request_and_names_the_pick() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let (parser, own) = (h.request_of("s1"), h.request_of("s2"));
    assert_ne!(own["id"], parser["id"]);
    // W3b-2: and its binding, a keyed hash the page posts back with its answer.
    let mut shown = own["suggestion"].clone();
    let binding = shown.as_object_mut().unwrap().remove("binding").unwrap();
    let binding = binding.as_str().unwrap();
    assert!(
        binding.len() == 64 && binding.chars().all(|c| c.is_ascii_hexdigit()),
        "{binding}"
    );
    assert_eq!(
        shown,
        json!({"id": parser["id"], "title": "Parser", "conversation": {"id": "c:claude:s2"}})
    );
    assert!(parser.get("suggestion").is_none(), "{parser}");
}

/// J4: Join moves the conversation, and the conversations of its own request (claude-mem's
/// `brought`), into the request it picked, in an op that reads nothing and names that
/// request's anchor first; the suggestion is gone, and a second answer is refused.
#[test]
fn join_moves_the_conversation_and_what_it_anchored_into_the_pick() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "");
    assert_eq!(h.request_of("s3")["id"], h.request_of("s2")["id"]);
    assert_eq!(h.answer_on("s2", "join").unwrap()["state"], "applied");
    let parser = h.request_of("s1");
    assert_eq!(parser["title"], "Parser");
    assert_eq!(parser["sessions"].as_array().unwrap().len(), 3, "{parser}");
    for s in ["s2", "s3"] {
        assert_eq!(h.request_of(s)["id"], parser["id"], "{s}");
    }
    assert!(h.page().iter().all(|r| r.get("suggestion").is_none()));
    let op = h.judged().pop().unwrap();
    assert_eq!(op.fields["action"], "join");
    assert_eq!(
        op.fields["groups"][0]["conversations"],
        json!([["claude", "s1"], ["claude", "s2"], ["claude", "s3"]])
    );
    assert!(op.read.spans.is_empty() && op.read.ops.is_empty());
    assert_eq!(
        h.post(json!({"request": parser["id"], "action": "join", "binding": ANY})),
        Err((404, "suggestion_not_found"))
    );
}

/// J4: Not this keeps it its own request, the suggestion goes, and the judge does not ask
/// about it again.
#[test]
fn not_this_keeps_it_its_own_request() {
    let mut h = Home::new();
    let (served, _) = suggested(&mut h);
    let own = h.request_of("s2")["id"].clone();
    assert_eq!(h.answer_on("s2", "not-this").unwrap()["state"], "applied");
    let after = h.request_of("s2");
    assert_eq!(after["id"], own);
    assert!(after.get("suggestion").is_none(), "{after}");
    assert_ne!(after["id"], h.request_of("s1")["id"]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    assert_eq!(bodies(&served).len(), 2, "no call after the answer");
    assert_eq!(
        h.post(json!({"request": own, "action": "join", "binding": ANY})),
        Err((404, "suggestion_not_found"))
    );
}

/// J4: the pick's anchor joined another request after the suggestion: the suggestion names
/// that request, and Join lands there without taking the anchor out of it.
#[test]
fn join_lands_in_the_request_the_pick_is_in_now() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s0", -10, "Ship the reader.");
    h.grouped(&["s0", "s1"], "Reader");
    assert_eq!(h.request_of("s2")["suggestion"]["title"], "Reader");
    h.answer_on("s2", "join").unwrap();
    let reader = h.request_of("s2");
    assert_eq!(reader["title"], "Reader");
    for s in ["s0", "s1"] {
        assert_eq!(h.request_of(s)["id"], reader["id"], "{s}");
    }
}

/// J4: a body of another shape, an id that names no request or none with a suggestion, and a
/// Join whose pick is in no request any more are refused, and nothing is written.
#[test]
fn an_answer_is_refused_before_it_writes() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let (own, parser) = (
        h.request_of("s2")["id"].clone(),
        h.request_of("s1")["id"].clone(),
    );
    let card = h.card("s2");
    let before = h.judged().len();
    for (body, refusal) in [
        // W3b-2: a page from before the binding posts none (its words for this: reload the page).
        (
            json!({"request": own, "action": "join"}),
            (400, "bad_request"),
        ),
        (
            json!({"request": own, "action": "join", "binding": "f00"}),
            (400, "bad_request"),
        ),
        (
            json!({"request": own, "action": "merge"}),
            (400, "bad_request"),
        ),
        (
            json!({"request": own, "action": "join", "more": 1}),
            (400, "bad_request"),
        ),
        (
            json!({"request": "", "action": "join"}),
            (400, "bad_request"),
        ),
        (
            json!({"request": "g:000000000000", "action": "join", "binding": ANY}),
            (404, "suggestion_not_found"),
        ),
        (
            json!({"request": parser, "action": "join", "binding": ANY}),
            (404, "suggestion_not_found"),
        ),
    ] {
        assert_eq!(h.post(body.clone()), Err(refusal), "{body}");
    }
    h.ignored("s1");
    assert!(h.request_of("s2").get("suggestion").is_none());
    assert_eq!(
        h.answer(&card, "join"),
        Err((409, "suggestion_target_gone"))
    );
    assert_eq!(h.judged().len(), before);
}

/// S5: a forget of a record the suggestion was made on takes it, and the conversation is
/// undecided again; after the owner's Join or Not this, the same forget leaves the answer.
#[test]
fn a_forget_takes_the_suggestion_and_leaves_the_owners_answer() {
    for answer in [None, Some("join"), Some("not-this")] {
        let mut h = Home::new();
        let (_served, ask) = suggested(&mut h);
        assert!(h.judged()[0].read.spans.iter().any(|s| s.0 == ask));
        if let Some(action) = answer {
            h.answer_on("s2", action).unwrap();
        }
        let h = h.forgot(ask);
        let groups = h.groups();
        assert!(!groups.suggested.contains_key(&key("s2")), "{answer:?}");
        let anchor = match answer {
            None => None,
            Some("join") => Some(key("s1")),
            Some(_) => Some(key("s2")),
        };
        assert_eq!(groups.of.get(&key("s2")), anchor.as_ref(), "{answer:?}");
    }
}

/// #775 (an_inflight_grouping_answer_cannot_undo_the_owners_join): a model's decision kept
/// after the owner's Join, asked before it, moves none of the conversations the owner placed.
/// One led by the conversation the owner moved continues the request it is in now, without
/// renaming it; one that lists it among others leaves it, and so does one that leaves it out.
#[test]
fn an_inflight_grouping_answer_cannot_undo_the_owners_join() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.answer_on("s2", "join").unwrap();
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "Stale");
    h.said("s4", 12, "Check the commas.");
    h.grouped(&["s4", "s2"], "Other");
    h.ignored("s2");
    let parser = h.request_of("s1");
    assert_eq!(parser["title"], "Parser");
    // The request the owner joined it to is no conversation the owner moved: continued, it
    // takes the model's title as before.
    h.said("s5", 14, "All of the parser.");
    h.grouped(&["s1", "s5"], "Parser, all of it");
    assert_eq!(h.request_of("s5")["title"], "Parser, all of it");
    let parser = h.request_of("s1");
    for s in ["s2", "s3"] {
        assert_eq!(h.request_of(s)["id"], parser["id"], "{s}");
    }
    assert_eq!(h.request_of("s4")["title"], "Other");
}

/// #775 (pending_owner_joins_preserve_the_current_destination): an answer posted while an
/// earlier one waits to be applied is resolved after it. s3 suggested to s2's request, and s2's
/// Join into s1 recorded but not yet applied: s3's card names s2's request, which its pick is in
/// no more once that Join is applied (W3b-2: 409, nothing recorded); read again, it names s1's,
/// and its Join lands there, and s2 stays there.
#[test]
fn pending_owner_joins_preserve_the_current_destination() {
    let mut h = Home::new();
    let _first = suggested(&mut h);
    h.said("s3", 10, "The trailing comma, once more.");
    let _second = stub(vec![picked("r1", 0.9), checked(0.6)]);
    h.pass(true, &|_| panic!("the chain is not asked"));
    let (s2, s3) = (h.request_of("s2"), h.request_of("s3"));
    assert_eq!(
        s3["suggestion"]["id"], s2["id"],
        "s3 suggested to s2's request"
    );
    let joined = json!({"conversation": ["claude", "s2"], "action": "join", "groups":
            [{"conversations": [["claude", "s1"], ["claude", "s2"]], "title": null, "links": []}]});
    let op = MyWorkOp {
        job: NAME.into(),
        about: R.into(),
        entry: String::new(),
        ts: crate::db::now_ms(),
        read: Read::default(),
        removed: vec![],
        fields: serde_json::from_value(joined).unwrap(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    let stale = json!({"request": s3["id"], "action": "join",
        "binding": s3["suggestion"]["binding"]});
    let before = h.recorded();
    assert_eq!(h.post(stale), Err((409, "suggestion_changed")));
    assert_eq!(h.recorded(), before);
    h.answer_on("s3", "join").unwrap();
    let parser = h.request_of("s1");
    for s in ["s2", "s3"] {
        assert_eq!(h.request_of(s)["id"], parser["id"], "{s}");
    }
}

/// #777 (a_judge_op_folded_after_the_owners_answer_opens_no_suggestion): an answer that folds
/// before the judge op it answered, its time the earlier (another device's clock behind this
/// one's), opens no suggestion again: a conversation the owner placed takes none.
#[test]
fn a_judge_op_folded_after_the_owners_answer_opens_no_suggestion() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    assert!(h.groups().suggested.contains_key(&key("s2")));
    let declined = json!({"conversation": ["claude", "s2"], "action": "not-this", "groups":
            [{"conversations": [["claude", "s2"]], "title": null, "links": []}]});
    let op = MyWorkOp {
        job: NAME.into(),
        about: R.into(),
        entry: String::new(),
        ts: h.t,
        read: Read::default(),
        removed: vec![],
        fields: serde_json::from_value(declined).unwrap(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    h.s.run();
    let groups = h.groups();
    assert!(!groups.suggested.contains_key(&key("s2")));
    assert_eq!(groups.of.get(&key("s2")), Some(&key("s2")));
    assert!(h.request_of("s2")["suggestion"].is_null());
}

/// #777 (catch_up_does_not_wait_for_unrelated_work_after_its_checkpoint_is_applied): with this
/// device's ops applied, the catch-up runs no worker, so a worker lock held elsewhere costs it
/// nothing.
#[test]
fn catch_up_does_not_wait_for_unrelated_work_after_its_checkpoint_is_applied() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let _held = crate::worker::lock(h.s.home.path()).unwrap().unwrap();
    let started = std::time::Instant::now();
    assert!(caught_up(h.s.home.path()).unwrap());
    let took = started.elapsed();
    assert!(took < Duration::from_secs(1), "{took:?}");
}

/// #777 (a_failed_catch_up_is_unavailable_not_pending): a catch-up whose worker cannot run is
/// the store unavailable, 503, not an answer still being saved; one whose wait only runs out,
/// its pass among it, is 409 `my_work_pending`. Neither writes the answer.
#[test]
fn a_failed_catch_up_is_unavailable_not_pending() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let mut answer = h.card("s2");
    answer["action"] = json!("not-this");
    // An op of this device the consumer has not applied yet.
    h.grouped(&["s9"], "Unrelated");
    let lock = h.s.home.path().join("state").join("worker.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    assert_eq!(h.post(answer.clone()), Err((503, "my_work_unavailable")));
    std::fs::remove_dir(&lock).unwrap();
    // The worker's pass, which waits 2 seconds for the lock held here, is within the wait: with
    // a wait of 2 seconds, refused right after the pass, not a whole wait after it. Timed inside
    // `applied`, from the pass's end to the refusal, which a slow runner does not move (#782:
    // from the post's start, macOS CI took 3.6 s and more).
    let held = crate::worker::lock(h.s.home.path()).unwrap().unwrap();
    crate::claims::WAIT.set(Some(Duration::from_millis(2_000)));
    let pending = h.post(answer);
    crate::claims::WAIT.set(None);
    let after = crate::claims::AFTER_PASS.take().unwrap();
    assert_eq!(pending, Err((409, "my_work_pending")));
    assert!(after < Duration::from_millis(1_000), "{after:?}");
    drop(held);
    h.s.run();
    assert!(h.groups().suggested.contains_key(&key("s2")), "not written");
}

/// #775: an exclusion of the repository while an answer stands hides its requests, and taken
/// back, the answer is as it was: the owner's op read nothing, so no exclusion hides it.
#[test]
fn an_exclusion_hides_the_answered_requests_and_leaves_the_answer() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.answer_on("s2", "join").unwrap();
    h.s.raw.exclude(R, false).unwrap();
    assert!(h.page().is_empty(), "{:?}", h.page());
    h.s.raw.exclude(R, true).unwrap();
    assert_eq!(h.request_of("s2")["id"], h.request_of("s1")["id"]);
}

/// Codex on #776: a Not this leaves the conversation where it is, so a model's current
/// decision that continues its request gives that request its title, as C5's newest title.
#[test]
fn a_current_grouping_continuation_can_title_a_not_this_request() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.answer_on("s2", "not-this").unwrap();
    h.said("s3", 10, "The trailing comma, in the tests too.");
    h.grouped(&["s2", "s3"], "Trailing commas");
    let own = h.request_of("s2");
    assert_eq!(own["title"], "Trailing commas");
    assert_eq!(h.request_of("s3")["id"], own["id"]);
}

/// Decision 81: the judge's own evidence in Japanese when oboete's words are, else English,
/// whatever the language the summaries are written in.
#[test]
fn the_judges_evidence_is_in_the_notes_language() {
    assert_eq!(
        picked_evidence(false, "Jev", &considered(false, 19, 21), 0.94, 0.9),
        "Jev: picked over the 19 most recently active of 21 open requests (p 0.94); the check \
             \"continues that request\" scored 0.90."
    );
    assert_eq!(considered(false, 1, 1), "1 open request");
    assert_eq!(considered(true, 2, 2), "開いている依頼 2 件");
    assert!(rules_evidence(true, true).contains("ツール呼び出し"));
    assert!(rules_evidence(false, false).ends_with("a work list both wrote."));
    for (language, ja) in [("Japanese", true), ("English", false), ("French", false)] {
        assert_eq!(config::notes_japanese(language), ja, "{language}");
    }
}

/// W3b-1: a suggestion whose answer would take an anchor out of where it is now is not shown: the
/// carrier's anchor moved into another request since, or the target's; a Join to it is refused
/// then, recording nothing, and Not this, which moves only its own conversations, closes it.
#[test]
fn a_suggestion_whose_answer_would_move_an_anchor_is_not_shown() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "");
    assert!(
        h.request_of("s3").get("suggestion").is_some(),
        "shown while it places what it shows"
    );
    h.said("s4", 12, "Another thing entirely.");
    h.grouped(&["s4", "s2"], "Other");
    let carrier = h.request_of("s3");
    assert!(carrier.get("suggestion").is_none(), "{carrier}");

    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s7", -5, "Seven.");
    h.grouped(&["s7", "s1"], "Seven");
    assert_eq!(h.request_of("s2")["suggestion"]["title"], "Seven");
    let card = h.card("s2");
    h.said("s8", -8, "Eight.");
    h.grouped(&["s8", "s7"], "Eight");
    let own = h.request_of("s2");
    assert!(own.get("suggestion").is_none(), "{own}");
    // The card read before names the pick's request as it is (its anchor, Seven's, now out of its
    // own group): a Join is refused, nothing recorded; a Not this, which places nothing there,
    // still closes the suggestion.
    let before = h.recorded();
    assert_eq!(
        h.answer(&card, "join"),
        Err((409, "suggestion_target_gone"))
    );
    assert_eq!(h.recorded(), before);
    assert_eq!(h.answer(&card, "not-this").unwrap()["state"], "applied");
}

/// W3b-1: an answer from a page read before the carrier's anchor moved is refused for either
/// action, and nothing is recorded.
#[test]
fn an_answer_whose_placement_changed_is_refused_and_records_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "");
    let card = h.card("s3");
    h.said("s4", 12, "Another thing entirely.");
    h.grouped(&["s4", "s2"], "Other");
    let before = h.recorded();
    for action in ["join", "not-this"] {
        let answer = h.answer(&card, action);
        assert_eq!(answer, Err((409, "suggestion_changed")), "{action}");
    }
    assert_eq!(h.recorded(), before);
}

/// W3b-1: an op this device appends of a kind the answer reads (a regroup, here of another
/// conversation), or a forget's denial, between the answer's check and its write refuses it, and
/// nothing is recorded.
#[test]
fn the_stores_moving_between_the_check_and_the_write_refuse_the_answer() {
    /// What moves the stores, by its name.
    type Moved = (&'static str, fn(&std::path::Path));
    let moves: [Moved; 2] = [
        ("a regroup", |home| {
            let mut raw = crate::raw::open(home).unwrap();
            let op = MyWorkOp {
                job: "group".into(),
                about: R.into(),
                entry: "fake".into(),
                ts: crate::db::now_ms(),
                read: Read::default(),
                removed: vec![],
                fields: BTreeMap::from([
                    (
                        "groups".to_owned(),
                        json!([{"conversations": [["claude", "s9"]], "title": "Elsewhere",
                            "links": []}]),
                    ),
                    ("ignored".to_owned(), json!([])),
                ]),
                skipped: false,
                forgotten: None,
            };
            let op = serde_json::to_value(op).unwrap();
            raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        }),
        ("a forget's denial", |home| {
            let raw = Connection::open(home.join("raw.db")).unwrap();
            raw.execute(
                "INSERT INTO denied_records(origin, device, seq, ts, session, job)
                 VALUES ('o', 'd', 1, 0, 's', 'j')",
                [],
            )
            .unwrap();
        }),
    ];
    for (what, move_) in moves {
        let mut h = Home::new();
        let _served = suggested(&mut h);
        let home = h.s.home.path().to_owned();
        CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(move || move_(&home))));
        let before = h.recorded();
        assert_eq!(
            h.answer_on("s2", "join"),
            Err((409, "my_work_pending")),
            "{what}"
        );
        assert_eq!(h.recorded(), before, "{what}");
        assert!(
            h.request_of("s2").get("suggestion").is_some(),
            "{what}: still open"
        );
    }
}

/// W3b-1, R6: an answer whose token was replaced while it waited records nothing.
#[test]
fn an_answer_whose_token_was_replaced_records_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let mut body = h.card("s2");
    body["action"] = json!("join");
    let body = serde_json::to_vec(&body).unwrap();
    let before = h.recorded();
    let refused = act(h.s.home.path(), &Mutex::new(()), &body, &|| false).unwrap_err();
    assert_eq!((refused.status, refused.code), (401, "token"));
    assert_eq!(h.recorded(), before);
    assert!(h.request_of("s2").get("suggestion").is_some(), "still open");
}

/// W3b-1: curation's op between the answer's check and its write leaves it be: the worker writes
/// claims, windows and turns all day, and an answer is not refused for them.
#[test]
fn curation_between_the_check_and_the_write_leaves_the_answer_be() {
    let mut h = Home::new();
    let (_served, ask) = suggested(&mut h);
    let home = h.s.home.path().to_owned();
    let curated = move || {
        let mut raw = crate::raw::open(&home).unwrap();
        let text = "Build the parser.";
        let evidence = crate::claims::Evidence {
            device: raw.device().to_owned(),
            seq: ask,
            offset: 0,
            length: text.len() as i64,
            sentence: 0,
            quote: text.into(),
            claim_at: None,
        };
        let op = crate::claims::ClaimOp {
            id: format!("c{ask}"),
            kind: "decision".into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: text.into(),
            evidence: vec![evidence],
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
            support: Vec::new(),
        };
        let op = serde_json::to_value(op).unwrap();
        raw.append_ops(&[(OpKind::Claim, op)]).unwrap();
    };
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(curated)));
    assert_eq!(h.answer_on("s2", "join").unwrap()["state"], "applied");
    assert_eq!(h.request_of("s2")["id"], h.request_of("s1")["id"]);
}

/// W3b-1: Join is refused when the request its pick is in is no request now (a work state took
/// that conversation into a request it names: round 1 of the plan review), and nothing is
/// recorded; Not this, which moves nothing into it, still closes the suggestion.
#[test]
fn join_refuses_when_its_pick_is_no_request_now() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    h.named("s1", "A");
    // W3b-2 (#1114): the request the suggestion names holds the pick no more: not shown.
    assert!(
        h.request_of("s2").get("suggestion").is_none(),
        "{}",
        h.request_of("s2")
    );
    let before = h.recorded();
    let join = h.answer(&card, "join");
    assert_eq!(join, Err((409, "suggestion_target_gone")));
    assert_eq!(h.recorded(), before);
    let not_this = h.answer(&card, "not-this");
    assert_eq!(not_this.unwrap()["state"], "applied");
    assert!(!h.groups().suggested.contains_key(&key("s2")));
}

/// W3b-1: a request's `Held` carries the conversations its suggestion's answer places; when a
/// work state takes one of them into a request it names, the suggestion is not shown, and an answer
/// to it is refused for either action, recording nothing.
#[test]
fn a_suggestion_whose_group_lost_a_conversation_is_not_shown_nor_answered() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "");
    let carrier = h.request_of("s3");
    assert!(carrier.get("suggestion").is_some(), "{carrier}");
    let card = h.card("s3");
    let shown = carrier["id"].as_str().unwrap();
    let (raw, k) = crate::search::b::stores(h.s.home.path()).unwrap().unwrap();
    let checked = crate::my_work_focus::held_in((&raw, &k), h.s.home.path(), shown).unwrap();
    drop((raw, k));
    let mut placement = checked.unwrap().placement;
    placement.sort();
    assert_eq!(placement, vec![key("s2"), key("s3")]);
    h.named("s3", "B");
    let carrier = h.request_of("s2");
    assert!(carrier.get("suggestion").is_none(), "{carrier}");
    let before = h.recorded();
    for action in ["join", "not-this"] {
        let answer = h.answer(&card, action);
        assert_eq!(answer, Err((409, "suggestion_changed")), "{action}");
    }
    assert_eq!(h.recorded(), before);
}

/// Codex on #1113: a resident viewer an upgrade exec'd keeps the viewer's role and cannot run the
/// worker's pass: an answer there, with an op of this device the consumer has not applied yet,
/// waits for the worker that runs, and lands. W3b-2 (#1114): the worker starts only once the
/// answer has found that op not applied (`claims::WAITING`), so that the wait is the one taken.
#[test]
fn an_answer_in_an_execd_viewer_waits_for_the_worker_that_runs() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    /// Stops the worker's thread when the test ends, a failed one too.
    struct Stop(std::sync::Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, SeqCst);
        }
    }
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    h.grouped(&["s9"], "Unrelated");
    let home = h.s.home.path().to_owned();
    let stop = Stop(std::sync::Arc::new(AtomicBool::new(false)));
    let running = std::sync::Arc::clone(&stop.0);
    let (started, worker) = std::sync::mpsc::channel();
    crate::claims::WAITING.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let worker = std::thread::spawn(move || {
                while !running.load(SeqCst) {
                    crate::worker::run_once(&home).unwrap();
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            started.send(worker).unwrap();
        }))
    });
    crate::executable::RESUMED_AS.set(Some(crate::executable::Role::Viewer));
    let answer = h.answer(&card, "join");
    crate::executable::RESUMED_AS.set(None);
    drop(stop);
    let worker = worker.try_recv().expect("the answer waited for the op");
    worker.join().unwrap();
    assert_eq!(answer.unwrap()["state"], "applied");
    assert_eq!(h.request_of("s2")["id"], h.request_of("s1")["id"]);
}

/// Codex on #1113: in a resident viewer an upgrade exec'd, an op of this device that no worker
/// applies within the wait refuses the answer as pending (it cannot tell a worker that failed from
/// one still busy), and raw.db holds no answer.
#[test]
fn an_execd_viewers_answer_whose_op_no_worker_applies_is_pending() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let mut answer = h.card("s2");
    answer["action"] = json!("not-this");
    h.grouped(&["s9"], "Unrelated");
    let before = h.recorded();
    crate::executable::RESUMED_AS.set(Some(crate::executable::Role::Viewer));
    crate::claims::WAIT.set(Some(Duration::from_millis(300)));
    let pending = h.post(answer);
    crate::claims::WAIT.set(None);
    crate::executable::RESUMED_AS.set(None);
    assert_eq!(pending, Err((409, "my_work_pending")));
    assert_eq!(h.recorded(), before);
}

/// Codex on #1113: the worker busy (in a provider's call, which keeps it from the record index)
/// and an agent's record not yet indexed: the answer is not held for the index, and is recorded,
/// told as pending while the worker cannot apply it.
#[test]
fn an_answer_lands_while_the_worker_is_busy_and_the_record_index_is_behind() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let mut answer = h.card("s2");
    answer["action"] = json!("not-this");
    h.said("s5", 20, "Something else.");
    let before = h.recorded();
    let busy = crate::worker::lock(h.s.home.path()).unwrap().unwrap();
    crate::claims::WAIT.set(Some(Duration::from_millis(300)));
    let landed = h.post(answer);
    crate::claims::WAIT.set(None);
    drop(busy);
    assert_eq!(landed.unwrap()["state"], "pending");
    assert_eq!(h.recorded(), before + 1);
}

impl Home {
    /// The owner's hide of `target` (`hide::CARD`, `hide::SUMMARY`), as the page records it.
    fn hid(&mut self, target: &str) {
        let device = self.s.raw.device().to_owned();
        let body = json!({"target": target, "device": device, "op_seq": 1, "hidden": true});
        self.s.raw.append_ops(&[(OpKind::Hide, body)]).unwrap();
    }

    /// A my_work op of `job` that groups and places nothing, as a goals skip is.
    fn skipped(&mut self, job: &str) {
        let op = MyWorkOp {
            job: job.into(),
            about: R.into(),
            entry: "fake".into(),
            ts: crate::db::now_ms(),
            read: Read::default(),
            removed: vec![],
            fields: BTreeMap::new(),
            skipped: true,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }

    /// The worker's pass with only `consumers`, as a round reads the log one consumer at a time.
    fn applied_by(&self, consumers: Vec<Box<dyn crate::worker::Consumer>>) {
        let mut consumers = consumers;
        let mut k = crate::knowledge::open(self.s.home.path()).unwrap();
        crate::worker::drain(&self.s.raw, &mut k, &mut consumers).unwrap();
    }

    /// An answer with `card` while the worker is held, within `WAIT`'s 300 ms.
    fn held_answer(&self, card: &Value, action: &str) -> std::result::Result<Value, (u16, &str)> {
        let held = crate::worker::lock(self.s.home.path()).unwrap().unwrap();
        crate::claims::WAIT.set(Some(Duration::from_millis(300)));
        let answer = self.answer(card, action);
        crate::claims::WAIT.set(None);
        drop(held);
        answer
    }
}

/// W3b-2 (#1114): a card read before a conversation joined the group answers for the group it
/// showed: its binding is another now, and either answer is refused, nothing recorded; the card
/// read again answers, and its Join moves the new member too.
#[test]
fn an_old_card_after_a_member_joined_answers_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    h.said("s3", 10, "The trailing comma again.");
    h.grouped(&["s2", "s3"], "");
    assert!(
        h.request_of("s3").get("suggestion").is_some(),
        "shown, for both now"
    );
    let before = h.recorded();
    for action in ["join", "not-this"] {
        assert_eq!(
            h.answer(&card, action),
            Err((409, "suggestion_changed")),
            "{action}"
        );
    }
    assert_eq!(h.recorded(), before);
    assert_eq!(h.answer_on("s2", "join").unwrap()["state"], "applied");
    for s in ["s2", "s3"] {
        assert_eq!(h.request_of(s)["id"], h.request_of("s1")["id"], "{s}");
    }
}

/// W3b-2 (review of the plan, round 1): the request the pick is in moved, the carrier's own
/// conversations as they were: a card read before names another target, and either answer is
/// refused, nothing recorded; read again, Join lands in the request the pick is in now.
#[test]
fn an_old_card_whose_target_moved_answers_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    h.said("s0", -10, "Ship the reader.");
    h.grouped(&["s0", "s1"], "Reader");
    assert_eq!(h.request_of("s2")["suggestion"]["title"], "Reader");
    let before = h.recorded();
    for action in ["join", "not-this"] {
        assert_eq!(
            h.answer(&card, action),
            Err((409, "suggestion_changed")),
            "{action}"
        );
    }
    assert_eq!(h.recorded(), before);
    h.answer_on("s2", "join").unwrap();
    assert_eq!(h.request_of("s2")["id"], h.request_of("s0")["id"]);
}

/// W3b-2 (#1114): a hide of a card the my_work consumer took and the cards' consumer did not (a
/// round reads the log one consumer at a time), the worker held: the answer waits for cards, and
/// is refused when the wait runs out, nothing recorded; the same for a summary's hide and turns.
#[test]
fn an_answer_waits_for_cards_and_turns_to_take_a_hide() {
    for target in [crate::hide::CARD, crate::hide::SUMMARY] {
        let mut h = Home::new();
        let _served = suggested(&mut h);
        let card = h.card("s2");
        h.hid(target);
        h.applied_by(vec![Box::new(crate::consumer::my_work::MyWork)]);
        let before = h.recorded();
        assert_eq!(
            h.held_answer(&card, "not-this"),
            Err((409, "my_work_pending")),
            "{target}"
        );
        assert_eq!(h.recorded(), before, "{target}");
        h.s.run();
        assert_eq!(
            h.answer(&card, "not-this").unwrap()["state"],
            "applied",
            "{target}"
        );
    }
}

/// W3b-2 (#1114, R3): what no answer depends on is not waited for: curation's turn in the turns'
/// and the my_work consumer's lag, and a goals skip in the my_work consumer's, the worker held:
/// the answer is recorded, told as pending.
#[test]
fn an_answer_does_not_wait_for_curation_nor_a_goals_skip() {
    /// What is appended to the log, by its name.
    type Owed = (&'static str, fn(&mut Home));
    let owed: [Owed; 2] = [
        ("a parked turn", |h| {
            let body = json!({"text": "a turn parked before its call"});
            h.s.raw.append_ops(&[(OpKind::Turn, body)]).unwrap();
        }),
        ("a goals skip", |h| h.skipped(crate::my_work_goals::NAME)),
    ];
    for (what, owed) in owed {
        let mut h = Home::new();
        let _served = suggested(&mut h);
        let card = h.card("s2");
        owed(&mut h);
        let before = h.recorded();
        let answer = h.held_answer(&card, "not-this");
        assert_eq!(answer.unwrap()["state"], "pending", "{what}");
        assert_eq!(h.recorded(), before + 1, "{what}");
    }
}

/// W3b-2 (R1): an earlier answer recorded after the catch-up and before config.lock, the my_work
/// consumer not yet applying it: the check reads it on its own raw.db, finds it not applied, and
/// refuses the second, which records nothing.
#[test]
fn an_answer_after_an_earlier_one_the_consumer_has_not_applied_records_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    let home = h.s.home.path().to_owned();
    let earlier = move || {
        let declined = json!({"conversation": ["claude", "s2"], "action": "not-this",
            "groups": [{"conversations": [["claude", "s2"]], "title": null, "links": []}]});
        let op = MyWorkOp {
            job: NAME.into(),
            about: R.into(),
            entry: String::new(),
            ts: crate::db::now_ms(),
            read: Read::default(),
            removed: vec![],
            fields: serde_json::from_value(declined).unwrap(),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        let mut raw = crate::raw::open(&home).unwrap();
        raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    };
    CAUGHT_UP.with(|hook| *hook.borrow_mut() = Some(Box::new(earlier)));
    let before = h.recorded();
    assert_eq!(h.answer(&card, "join"), Err((409, "my_work_pending")));
    assert_eq!(h.recorded(), before + 1, "the earlier answer alone");
}

/// W3b-2 (#1114: the rebuild): a knowledge.db put in place while the answer waits for config.lock
/// that has not taken this device's ops (a projection made again, a rebuild that stopped part way)
/// is not answered on: 409, nothing recorded.
#[test]
fn a_knowledge_db_short_of_the_ops_is_not_answered_on() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    let home = h.s.home.path().to_owned();
    CAUGHT_UP.with(|hook| *hook.borrow_mut() = Some(Box::new(move || set_aside(&home))));
    let before = h.recorded();
    assert_eq!(h.answer(&card, "join"), Err((409, "my_work_pending")));
    assert_eq!(h.recorded(), before);
}

/// W3b-2: knowledge.db replaced after the check is the answer's no more (the snapshot is the
/// decision): the answer is recorded, and the log, read again into the new knowledge.db, gives
/// the grouping it was checked on.
#[test]
fn a_knowledge_db_replaced_after_the_check_keeps_the_answer() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    let home = h.s.home.path().to_owned();
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(move || set_aside(&home))));
    let before = h.recorded();
    h.answer(&card, "join").unwrap();
    assert_eq!(h.recorded(), before + 1);
    assert_eq!(h.request_of("s2")["id"], h.request_of("s1")["id"]);
}

/// W3b-2 (#1114: the restore): raw.db put in place after the check as a restore leaves it, the
/// same device and the same ops in another file: the answer is refused, and the new raw.db holds
/// none. (A restore itself refuses while this test's store keeps raw.db open.)
#[test]
fn a_raw_db_restored_after_the_check_records_nothing() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    let home = h.s.home.path().to_owned();
    let device = h.s.raw.device().to_owned();
    let restore = move || {
        let (path, copy) = (home.join("raw.db"), home.join("raw.db.restored"));
        Connection::open(&path)
            .unwrap()
            .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        // Its own file, as a restore's raw.db names itself.
        Connection::open(&copy)
            .unwrap()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'store_file'",
                [crate::db::store_file(&copy)],
            )
            .unwrap();
        for journal in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(journal));
        }
        std::fs::rename(&copy, &path).unwrap();
    };
    CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(restore)));
    let before = h.recorded();
    assert_eq!(h.answer(&card, "join"), Err((409, "my_work_pending")));
    let raw = crate::raw::open(h.s.home.path()).unwrap();
    assert_eq!(raw.device(), device, "the restore kept the device");
    drop(raw);
    assert_eq!(h.recorded(), before);
}

/// W3b-2 (R5): the receipt is the raw commit's: a wait for the consumer that fails after the
/// answer is written answers it as pending, and exactly one op is recorded.
#[test]
fn a_wait_that_fails_after_the_write_answers_pending() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    let card = h.card("s2");
    let home = h.s.home.path().to_owned();
    let broken = move || {
        let k = Connection::open(home.join("knowledge.db")).unwrap();
        k.execute_batch("DROP TABLE op_checkpoints").unwrap();
    };
    APPENDED.with(|hook| *hook.borrow_mut() = Some(Box::new(broken)));
    let before = h.recorded();
    assert_eq!(h.answer(&card, "join").unwrap()["state"], "pending");
    assert_eq!(h.recorded(), before + 1);
}

/// knowledge.db and its journal set aside under other names, as a projection made again leaves
/// the path: the next open makes a new one, which has taken no op.
fn set_aside(home: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        let path = home.join(format!("knowledge.db{suffix}"));
        if path.exists() {
            std::fs::rename(&path, home.join(format!("knowledge.db.aside{suffix}"))).unwrap();
        }
    }
}

/// W3b-2 (#1114): the pick in a group of two, a work state taking the pick alone into a request it
/// names: the group's request is still a request, without the pick, so the suggestion is not shown,
/// and a Join on the card read before is refused, nothing recorded; a Not this still closes it.
#[test]
fn a_join_whose_pick_left_a_group_that_stays_is_refused() {
    let mut h = Home::new();
    let _served = suggested(&mut h);
    h.said("s0", -10, "Ship the reader.");
    h.grouped(&["s0", "s1"], "Reader");
    assert_eq!(h.request_of("s2")["suggestion"]["title"], "Reader");
    let card = h.card("s2");
    h.named("s1", "A");
    assert_eq!(
        h.request_of("s0")["title"],
        "Reader",
        "the group's request stays"
    );
    assert!(
        h.request_of("s2").get("suggestion").is_none(),
        "{}",
        h.request_of("s2")
    );
    let before = h.recorded();
    assert_eq!(
        h.answer(&card, "join"),
        Err((409, "suggestion_target_gone"))
    );
    assert_eq!(h.recorded(), before);
    assert_eq!(h.answer(&card, "not-this").unwrap()["state"], "applied");
}
