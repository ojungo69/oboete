use super::*;
use crate::config::Summary;
use crate::curate::{Phase, Reads};
use crate::my_work_run::Runner;
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
use crate::search::b::fixture::Store;

const R: &str = "github.com/o/r";
const MINUTE: i64 = 60_000;

/// A home whose records start an hour ago.
struct Home {
    s: Store,
    t: i64,
}

impl Home {
    fn new() -> Self {
        Self {
            s: Store::new(),
            t: crate::db::now_ms() - 60 * MINUTE,
        }
    }

    /// A prompt of `session` in `repo` at `at` minutes from the start: its seq.
    fn said(&mut self, session: &str, repo: &str, at: i64, text: &str) -> i64 {
        self.s.said(session, repo, self.t + at * MINUTE, text)
    }

    /// A write of `list` in `repo` and the call of `session` that sent it, as oboete's
    /// server and its caller record them.
    fn wrote(&mut self, session: &str, repo: &str, list: &str, fields: Value) {
        let fields = fields.as_object().unwrap().clone();
        let seq = self.s.raw.work_state(repo, list, &fields, None).unwrap();
        let ts: i64 = rusqlite::Connection::open(self.s.home.path().join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT ts FROM ops WHERE device = ?1 AND op_seq = ?2",
                rusqlite::params![self.s.raw.device(), seq],
                |r| r.get(0),
            )
            .unwrap();
        let input = json!({"list": list, "fields": fields}).to_string();
        let body = json!({"tool": "mcp__oboete__work_state_write", "input": input,
                "output": "Saved.", "failed": false});
        self.s.event("tool", session, (repo, "main"), ts + 20, body);
    }

    /// One pass of a runner of grouping alone, the chain answering `answer` to each prompt
    /// and refusing what the check refuses: what it did, and the prompts sent.
    fn pass(&mut self, answer: &dyn Fn(&str) -> Value) -> (Phase, Vec<String>) {
        self.s.run();
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
            vec![Box::new(Grouping::default()) as Box<dyn Job>],
            Box::new(call),
            (&rules, &summary, "chain"),
        );
        let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
        drop(runner);
        self.s.run();
        (phase, sent.into_inner())
    }

    fn groups(&self) -> Groups {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        groups(&self.s.raw, &k, &reading).unwrap()
    }

    /// Grouping's newest op as long ago as `minutes` minutes, as C5 counts it.
    fn age(&self, minutes: i64) {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        k.execute(
            "UPDATE my_work SET ts = ts - ?1 WHERE job = 'group'",
            [minutes * MINUTE],
        )
        .unwrap();
    }

    fn due(&self, summary: &Summary) -> Vec<(String, i64)> {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let rules = Rules::load(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        let stores = Stores {
            raw: &self.s.raw,
            k: &k,
            rules: &rules,
            summary,
            reading: &reading,
            held: &crate::my_work_run::NOT_HELD,
        };
        let due = Grouping::default()
            .due(&stores, crate::db::now_ms())
            .unwrap();
        due.into_iter().map(|d| (d.about, d.since)).collect()
    }

    fn ask(&self, summary: &Summary, about: &str) -> Option<Ask> {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let rules = Rules::load(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        let stores = Stores {
            raw: &self.s.raw,
            k: &k,
            rules: &rules,
            summary,
            reading: &reading,
            held: &crate::my_work_run::NOT_HELD,
        };
        Grouping::default()
            .ask(&stores, about, crate::db::now_ms())
            .unwrap()
    }
}

fn key(session: &str) -> Key {
    ("claude".to_owned(), session.to_owned())
}

fn group(conversations: &[&str], title: &str) -> Value {
    let links = if conversations.len() > 1 {
        json!([{"conversations": conversations, "evidence": "The same work, asked again."}])
    } else {
        json!([])
    };
    json!({"conversations": conversations, "title": title, "links": links})
}

fn answer(groups: &[Value], ignored: &[(&str, &str)]) -> Value {
    let ignored: Vec<Value> = (ignored.iter())
        .map(|(c, r)| json!({"conversation": c, "reason": r}))
        .collect();
    json!({"groups": groups, "ignored": ignored})
}

/// Codex's security review of #669: a batch that shows an open request names the grouping
/// ops that made it in what it read (S5), so a forget that takes one of them takes the op built
/// on it, its title with it, whichever of the request's asks the batch left out.
#[test]
fn a_batch_names_the_grouping_ops_its_open_requests_came_from() {
    let mut h = Home::new();
    h.said(
        "s1",
        R,
        0,
        "Build the parser and keep its tables between runs.",
    );
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.s.run();
    let k = crate::knowledge::open(h.s.home.path()).unwrap();
    let first: i64 = k
        .query_row("SELECT op_seq FROM my_work WHERE job = 'group'", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(k);
    h.age(31);
    h.said("s2", R, 1, "Write the docs for the release notes.");
    let ask = h.ask(&Summary::default(), R).unwrap();
    assert!(ask.prompt.contains("\"Parser\""), "{}", ask.prompt);
    assert_eq!(ask.read.ops, vec![first]);
    // A batch that shows no open request reads no op.
    let none = Summary {
        window_tokens: 0,
        ..Summary::default()
    };
    let ask = h.ask(&none, R).unwrap();
    assert!(!ask.prompt.contains("\"Parser\""), "{}", ask.prompt);
    assert!(ask.read.ops.is_empty());
}

/// C5: a decided conversation is never asked about again; a repository is asked again 30
/// minutes after its last op, its open request offered for a new conversation to continue,
/// and continuing keeps the earlier links where claude-mem replaces them.
#[test]
fn a_conversation_is_decided_once_and_never_asked_again() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.said("s2", R, 5, "Add tests for the parser.");
    let first = answer(&[group(&["C1", "C2"], "The parser")], &[]);
    let (phase, sent) = h.pass(&|_| first.clone());
    assert_eq!((phase, sent.len()), (Phase::Covered, 1));
    assert!(sent[0].contains("C1 · claude") && sent[0].contains("Build the parser."));
    let g = h.groups();
    assert_eq!(g.of[&key("s2")], key("s1"));
    assert_eq!(g.groups[&key("s1")].members, vec![key("s1"), key("s2")]);
    assert_eq!(g.groups[&key("s1")].title.as_deref(), Some("The parser"));
    assert_eq!(h.pass(&|_| panic!("nothing is undecided")).1.len(), 0);
    h.said("s3", R, 10, "Fix the parser's error messages.");
    // Within 30 minutes of the last op, the repository waits, the worker staying up for it.
    let (phase, sent) = h.pass(&|_| panic!("asked within 30 minutes"));
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert!(sent.is_empty());
    h.age(31);
    let next = answer(&[group(&["R1", "C1"], "The parser and its errors")], &[]);
    let (phase, sent) = h.pass(&|_| next.clone());
    assert_eq!(phase, Phase::Covered);
    assert!(sent[0].contains("R1 · \"The parser\""), "{}", sent[0]);
    assert!(sent[0].contains("C1 · claude") && sent[0].contains("error messages"));
    assert!(
        !sent[0].contains("C2 ·"),
        "a decided conversation is not asked again"
    );
    let g = h.groups();
    let parser = &g.groups[&key("s1")];
    assert_eq!(parser.members, vec![key("s1"), key("s2"), key("s3")]);
    assert_eq!(parser.title.as_deref(), Some("The parser and its errors"));
    assert_eq!(parser.links.len(), 2, "continuing keeps the earlier links");
}

/// C2, C4: a call is about one repository, whose worktrees share its key; another
/// repository's open requests are not offered.
#[test]
fn a_group_stays_in_one_repository_and_worktrees_share_it() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    let body = json!({"prompt": "Review the parser in the other worktree."});
    h.s.event("prompt", "s2", (R, "feature"), h.t + 5 * MINUTE, body);
    h.said("s3", "github.com/o/other", 6, "Write the release notes.");
    let summary = Summary::default();
    h.s.run();
    let due: Vec<String> = h.due(&summary).into_iter().map(|d| d.0).collect();
    assert_eq!(due, vec!["github.com/o/other".to_owned(), R.to_owned()]);
    let ask = h.ask(&summary, R).unwrap();
    assert!(ask.prompt.contains("Build the parser.") && ask.prompt.contains("other worktree"));
    assert!(!ask.prompt.contains("release notes"));
}

/// C4: the check refuses an answer that leaves a conversation undecided, decides one twice,
/// names a label it was not asked about or an open request anywhere but first, joins without
/// links that reach every member, or gives evidence over its cap; it takes a whole answer.
#[test]
fn an_answer_that_leaves_a_conversation_undecided_is_refused() {
    let labels: BTreeMap<String, Key> = [("C1", "s1"), ("C2", "s2"), ("R1", "s0")]
        .into_iter()
        .map(|(l, s)| (l.to_owned(), key(s)))
        .collect();
    let good = answer(
        &[group(&["R1", "C1"], "The parser")],
        &[("C2", "A greeting.")],
    );
    assert_eq!(check(&good, &labels), None);
    let cases = [
        (answer(&[group(&["C1"], "The parser")], &[]), "shape"),
        (
            answer(&[group(&["C1", "C1"], "The parser")], &[("C2", "x")]),
            "shape",
        ),
        (
            answer(&[group(&["C1", "R1"], "The parser")], &[("C2", "x")]),
            "shape",
        ),
        (
            answer(&[group(&["C3"], "The parser")], &[("C1", "x"), ("C2", "x")]),
            "shape",
        ),
        (
            answer(&[group(&["C1"], "a"), group(&["C2"], "b")], &[("C2", "x")]),
            "shape",
        ),
        (json!({"groups": []}), "shape"),
    ];
    for (a, outcome) in cases {
        assert_eq!(check(&a, &labels), Some(outcome), "{a}");
    }
    let joined = |links: Value| {
        json!({"groups": [{"conversations": ["R1", "C1", "C2"], "title": "t", "links": links}],
                "ignored": []})
    };
    assert_eq!(check(&joined(json!([])), &labels), Some("unanchored"));
    let partial = json!([{"conversations": ["R1", "C1"], "evidence": "Same list."}]);
    assert_eq!(check(&joined(partial), &labels), Some("unanchored"));
    let chain = json!([{"conversations": ["R1", "C1"], "evidence": "Same list."},
            {"conversations": ["C2", "C1"], "evidence": "Same task."}]);
    assert_eq!(check(&joined(chain), &labels), None);
    let long = json!([{"conversations": ["R1", "C1", "C2"], "evidence": "x".repeat(161)}]);
    assert_eq!(check(&joined(long), &labels), Some("over_cap"));
    let empty = json!([{"conversations": ["R1", "C1", "C2"], "evidence": " "}]);
    assert_eq!(check(&joined(empty), &labels), Some("unanchored"));
}

/// C2: within `[summary] window_tokens`, the open requests a proven link names come first,
/// then the conversations, then the other open requests, the newest first; a title over its
/// cap is dropped from what is kept, never cut.
#[test]
fn open_requests_fit_the_budget_linked_first_then_newest() {
    let mut h = Home::new();
    let ask = |what: &str| format!("{what} {}", "and keep its tables between runs ".repeat(12));
    // Three open requests: s1's (linked by a list it shares with s9), then s2's and s3's,
    // s3's the newest.
    h.said("s1", R, 0, &ask("Build the parser"));
    h.said("s2", R, 1, &ask("Write the docs"));
    h.said("s3", R, 2, &ask("Tune the cache"));
    let first = answer(
        &[
            group(&["C1"], "Parser"),
            group(&["C2"], "Docs"),
            group(&["C3"], "Cache"),
        ],
        &[],
    );
    h.pass(&|_| first.clone());
    h.age(31);
    h.said("s9", R, 3, &ask("Continue the parser"));
    h.wrote("s1", R, "parser", json!({"status": "doing"}));
    h.wrote("s9", R, "parser", json!({"status": "doing"}));
    // Room for the linked request, the conversation and one more request, of about 120, 130
    // and 120 estimated tokens, and not for a fourth.
    let room = |window_tokens: u32| Summary {
        window_tokens,
        ..Summary::default()
    };
    let prompt = h.ask(&room(420), R).unwrap().prompt;
    let at = |s: &str| prompt.find(s).unwrap_or(usize::MAX);
    assert!(at("\"Parser\"") < usize::MAX && at("Continue the parser") < usize::MAX);
    assert!(
        at("\"Cache\"") < usize::MAX,
        "the newest other request fits: {prompt}"
    );
    assert_eq!(at("\"Docs\""), usize::MAX, "the oldest does not: {prompt}");
    assert!(prompt.contains("- work list parser: C1, R1"), "{prompt}");
    // Room for one conversation alone: it is shown all the same.
    let prompt = h.ask(&room(0), R).unwrap().prompt;
    assert!(prompt.contains("Continue the parser") && !prompt.contains("\"Cache\""));
    let labels = BTreeMap::from([("C1".to_owned(), key("s9"))]);
    let kept = keep(
        &answer(&[group(&["C1"], &"t".repeat(121))], &[]),
        &labels,
        &Rules::default(),
    );
    assert_eq!(kept["groups"][0]["title"], Value::Null);
    assert_eq!(
        kept["groups"][0]["conversations"],
        json!([["claude", "s9"]])
    );
}

/// A5: what the job read is kept for the round, and `Job::reread` drops it. Here
/// a repository whose conversation came since is in its next read, and not while the first is
/// kept. The runner tells every job so when the list changed between a round's two passes
/// (`my_work_run`).
#[test]
fn the_job_keeps_its_read_until_it_is_told_to_read_again() {
    let mut h = Home::new();
    let job = Grouping::default();
    let due = |h: &Home| {
        let k = crate::knowledge::open(h.s.home.path()).unwrap();
        let rules = Rules::load(h.s.home.path()).unwrap();
        let summary = Summary::default();
        let reading = Reading::now(&h.s.raw, Reads::Live).unwrap();
        let stores = Stores {
            raw: &h.s.raw,
            k: &k,
            rules: &rules,
            summary: &summary,
            reading: &reading,
            held: &crate::my_work_run::NOT_HELD,
        };
        job.due(&stores, crate::db::now_ms()).unwrap().len()
    };
    assert_eq!(due(&h), 0);
    h.said("s1", R, 0, "Build the parser.");
    h.s.run();
    assert_eq!(due(&h), 0, "kept for the round");
    job.reread();
    assert_eq!(due(&h), 1, "read again");
}

/// C1: a conversation whose work names a request id is that request's, named by its agent,
/// and not grouped; nor is one last active more than 14 days ago.
#[test]
fn a_conversation_naming_request_ids_is_not_grouped() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.wrote(
        "s1",
        R,
        "parser",
        json!({"request_id": "R-7", "status": "doing"}),
    );
    h.said("s2", R, -15 * 24 * 60, "Write the release notes.");
    h.s.run();
    assert!(h.due(&Summary::default()).is_empty());
    h.said("s3", R, 1, "Tune the cache.");
    h.s.run();
    assert_eq!(h.due(&Summary::default()).len(), 1);
    let ask = h.ask(&Summary::default(), R).unwrap();
    assert!(ask.prompt.contains("Tune the cache") && !ask.prompt.contains("release notes"));
}

/// C4, C5: a conversation ignored, with its reason, is decided, and not asked about again.
#[test]
fn an_ignored_conversation_is_decided_and_not_asked_again() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.said("s2", R, 5, "Is the build green?");
    let first = answer(
        &[group(&["C1"], "The parser")],
        &[("C2", "A question answered at once.")],
    );
    h.pass(&|_| first.clone());
    let g = h.groups();
    let reason = g.ignored[&key("s2")].as_deref();
    assert_eq!(reason, Some("A question answered at once."));
    assert!(!g.of.contains_key(&key("s2")));
    h.age(31);
    assert_eq!(h.pass(&|_| panic!("nothing is undecided")).1.len(), 0);
}

/// S6: an op shown a record of a repository excluded now decides nothing while the exclusion
/// stands, and decides again once it is taken back.
#[test]
fn an_op_shown_an_excluded_record_decides_nothing_while_the_exclusion_stands() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.said("s2", R, 5, "Add tests for the parser.");
    let first = answer(&[group(&["C1", "C2"], "The parser")], &[]);
    h.pass(&|_| first.clone());
    assert_eq!(h.groups().groups.len(), 1);
    h.s.raw.exclude(R, false).unwrap();
    let g = h.groups();
    assert!(g.groups.is_empty() && g.of.is_empty(), "{g:?}");
    h.s.raw.exclude(R, true).unwrap();
    assert_eq!(h.groups().groups[&key("s1")].members.len(), 2);
}

/// Codex on #669 (S6): an op built on one shown an excluded record (among its `read.ops`)
/// decides nothing either while the exclusion stands, though its own records missed the
/// excluded conversation's: the request it continued, and its title, came from it.
#[test]
fn an_op_built_on_an_op_shown_an_excluded_record_decides_nothing_either() {
    const X: &str = "github.com/o/x";
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.said("s2", R, 5, "Add tests for the parser.");
    h.pass(&|_| answer(&[group(&["C1", "C2"], "The parser")], &[]));
    // The next call shows the request's opening ask and its three newest asks, all of s1's.
    for (at, text) in [
        (6, "Make it faster."),
        (7, "Cache the tables."),
        (8, "Ship it."),
    ] {
        h.said("s1", R, at, text);
    }
    h.age(31);
    h.said("s3", R, 10, "Fix the parser's error messages.");
    let next = answer(&[group(&["R1", "C1"], "The parser and its errors")], &[]);
    let (_, sent) = h.pass(&|_| next.clone());
    assert!(
        !sent[0].contains("Add tests for the parser."),
        "{}",
        sent[0]
    );
    assert_eq!(h.groups().groups[&key("s1")].members.len(), 3);
    // s2 is seen in another repository, which is then excluded.
    h.said("s2", X, 11, "Look at the other repository.");
    h.s.run();
    h.s.raw.exclude(X, false).unwrap();
    let g = h.groups();
    assert!(g.groups.is_empty() && g.of.is_empty(), "{g:?}");
    h.s.raw.exclude(X, true).unwrap();
    assert_eq!(h.groups().groups[&key("s1")].members.len(), 3);
}

/// Codex on #669: the conversation a call decides takes its room first, a linked request only
/// what room it leaves, and its work lists are shown `LISTS` at most, then how many more.
#[test]
fn the_conversation_takes_its_room_first_and_shows_its_lists_bounded() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.said("s9", R, 3, "Continue the parser.");
    h.wrote("s1", R, "parser", json!({"status": "doing"}));
    h.wrote("s9", R, "parser", json!({"status": "doing"}));
    for n in 0..30 {
        let list = format!("list-{n:02}-{}", "x".repeat(150));
        h.wrote("s9", R, &list, json!({"status": "doing"}));
    }
    // Room for the linked request alone, of about 25 estimated tokens: the conversation, of
    // about 900, takes it.
    let room = Summary {
        window_tokens: 150,
        ..Summary::default()
    };
    let prompt = h.ask(&room, R).unwrap().prompt;
    assert!(prompt.contains("Continue the parser."), "{prompt}");
    assert!(!prompt.contains("\"Parser\""), "{prompt}");
    assert!(prompt.contains("and 11 more"), "{prompt}");
    assert!(prompt.matches("list-").count() <= LISTS, "{prompt}");
}

/// #677: the proven links are charged to the room left and the rest are counted, so
/// conversations that share 240 lists of 200 characters keep the prompt's data within
/// `[summary] window_tokens`, the conversation and the request shown as before.
#[test]
fn proven_links_fit_the_room_left_and_say_how_many_more() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.said("s9", R, 3, "Continue the parser.");
    for n in 0..240 {
        let list = format!("shared-{n:03}-{}", "y".repeat(189));
        h.wrote("s1", R, &list, json!({"status": "doing"}));
        h.wrote("s9", R, &list, json!({"status": "doing"}));
    }
    let summary = Summary::default();
    let prompt = h.ask(&summary, R).unwrap().prompt;
    let data = &prompt[prompt.find("## Conversations to decide").unwrap()..];
    // The section heads and the labels, about 40 tokens, are not charged, as before.
    assert!(
        crate::budget::estimate(data) <= summary.window_tokens + 50,
        "{data}"
    );
    let shown = data.matches("- work list ").count();
    assert!(shown > 0 && shown < 240, "{data}");
    assert!(
        data.contains(&format!("- and {} more", 240 - shown)),
        "{data}"
    );
    assert!(data.contains("C1 · claude"), "{data}");
    assert!(
        data.contains("R1 · \"Parser\""),
        "the linked request still fits: {data}"
    );
}

/// #686: the line that counts the links left out is charged to the room too: at every window,
/// links filling the room to its last token among them, the data stays within it, the section
/// heads and labels aside.
#[test]
fn the_count_of_the_links_left_out_is_charged_to_the_room() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.said("s9", R, 3, "Continue the parser.");
    // Each link line 125 characters, so its cost is whole (`budget::estimate`): no rounding
    // hides what the count line adds.
    for n in 0..30 {
        let list = format!("shared-{n:02}-{}", "y".repeat(94));
        h.wrote("s1", R, &list, json!({"status": "doing"}));
        h.wrote("s9", R, &list, json!({"status": "doing"}));
    }
    let heads = "## Conversations to decide, the oldest first\nC1 · \
                     ## Open requests in this repository\nR1 · ## Proven links\n";
    let mut counted = 0;
    for window in 1_000..1_060 {
        let room = Summary {
            window_tokens: window,
            ..Summary::default()
        };
        let prompt = h.ask(&room, R).unwrap().prompt;
        let start = prompt.find("## Conversations to decide").unwrap();
        let fence = (prompt[..start].trim_end_matches('\n').rsplit('\n'))
            .next()
            .unwrap();
        let data = &prompt[start..prompt.len() - fence.len()];
        assert!(data.contains("- work list "), "{window}: {data}");
        assert!(
            crate::budget::estimate(data) <= window + crate::budget::estimate(heads),
            "{window}: {data}"
        );
        counted += usize::from(data.contains(" more\n"));
    }
    assert!(counted > 0, "some windows leave links out");
}

/// #677: a link that names the conversation decided first is kept before one between the
/// others, so the first conversation's proven links fit first.
#[test]
fn links_naming_the_first_conversation_are_kept_first() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.said("s8", R, 2, "Continue the parser.");
    h.said("s9", R, 3, "Test the parser.");
    // The lists the second conversation shares with the request sort before the first's.
    for (prefix, other, count) in [("a", "s9", 40), ("z", "s8", 3)] {
        for n in 0..count {
            let list = format!("{prefix}-{n:02}-{}", "y".repeat(150));
            h.wrote("s1", R, &list, json!({"status": "doing"}));
            h.wrote(other, R, &list, json!({"status": "doing"}));
        }
    }
    let room = Summary {
        window_tokens: 1_500,
        ..Summary::default()
    };
    let prompt = h.ask(&room, R).unwrap().prompt;
    let data = &prompt[prompt.find("## Conversations to decide").unwrap()..];
    assert!(
        data.contains("C2 · claude"),
        "both conversations fit: {data}"
    );
    let links = &data[data.find("## Proven links").unwrap()..];
    for n in 0..3 {
        let line = format!("- work list z-{n:02}-{}: C1, R1\n", "y".repeat(150));
        assert!(links.contains(&line), "{links}");
    }
    assert!(links.contains("- work list a-00-"), "{links}");
    assert!(links.contains(" more\n"), "the others are counted: {links}");
}

/// O4: an opening ask an agent wrote, by its marks or by the text tests, is shown labelled.
#[test]
fn an_opening_ask_an_agent_wrote_is_labelled() {
    let mut h = Home::new();
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    h.said("s1", R, 0, "Build the parser.");
    let write = json!({"tool": "Write", "input": json!({"content": delegated}).to_string(),
            "output": "ok", "failed": false});
    h.s.event("tool", "s1", (R, "main"), h.t + MINUTE, write);
    h.said("s2", R, 2, &delegated);
    let prompt = h.ask(&Summary::default(), R).unwrap().prompt;
    let (c1, c2) = (prompt.find("C1 · ").unwrap(), prompt.find("C2 · ").unwrap());
    assert!(
        prompt[c2..].contains("[agent prompt] Review the parser"),
        "{prompt}"
    );
    assert!(!prompt[c1..c2].contains("[agent prompt]"), "{prompt}");
}

/// 3c (C3): a conversation to decide whose opening ask another conversation sent in a tool
/// call is linked to it, the one sent first in the line: to that conversation when it is
/// decided in the batch too, else to the open request it belongs to, which is shown. An ask
/// the marks say an agent sent is linked as a typed one is; the conversation's own call, and
/// a reply, link nothing.
#[test]
fn an_ask_another_conversation_sent_is_a_proven_link() {
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    let call = |text: &str| {
        let input = json!({"command": format!("codex exec '{text}'")}).to_string();
        json!({"tool": "Bash", "input": input, "output": "ok", "failed": false})
    };
    let links = |h: &Home| {
        let prompt = h.ask(&Summary::default(), R).unwrap().prompt;
        let at = prompt.find("## Proven links").unwrap();
        (prompt.clone(), prompt[at..].to_owned())
    };
    // Both decided in the batch.
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.s.event("tool", "s1", (R, "main"), h.t + MINUTE, call(&delegated));
    h.said("s2", R, 2, &delegated);
    let (prompt, shown) = links(&h);
    assert!(
        shown.contains("- C1 sent C2's opening ask in a tool call\n"),
        "{prompt}"
    );
    // The sender in an open request: the request's line, and the request shown.
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.s.event(
        "tool",
        "s1",
        (R, "main"),
        h.t + 2 * MINUTE,
        call(&delegated),
    );
    h.said("s2", R, 3, &delegated);
    let (prompt, shown) = links(&h);
    assert!(
        shown.contains("- a conversation of R1 sent C1's opening ask in a tool call\n"),
        "{prompt}"
    );
    assert!(prompt.contains("R1 · \"Parser\""), "{prompt}");
    // An ask the marks say an agent sent: a Codex session another conversation started.
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.s.event("tool", "s1", (R, "main"), h.t + MINUTE, call(&delegated));
    let marked = crate::raw::Event {
        kind: "prompt".into(),
        agent: "codex".into(),
        session: "s3".into(),
        repo: Some(R.into()),
        ts: h.t + 2 * MINUTE,
        ..crate::raw::test_event(&json!({"prompt": delegated, "agent_sent": true}).to_string())
    };
    h.s.raw.append(&marked).unwrap();
    let (prompt, shown) = links(&h);
    assert!(
        shown.contains("- C1 sent C2's opening ask in a tool call\n"),
        "{prompt}"
    );
    // With room for one open request after the conversation, the sender's goes before a newer
    // one (A4: the requests a proven link names first).
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    h.pass(&|_| answer(&[group(&["C1"], "Parser")], &[]));
    h.age(31);
    h.said("s5", R, 1, "Rename the lexer.");
    h.pass(&|_| answer(&[group(&["C1"], "Lexer")], &[]));
    h.age(31);
    h.s.event(
        "tool",
        "s1",
        (R, "main"),
        h.t + 2 * MINUTE,
        call(&delegated),
    );
    h.said("s2", R, 3, &delegated);
    let (prompt, _) = links(&h);
    let start = prompt.find("C1 · ").unwrap();
    let end = prompt.find("## Open requests").unwrap();
    let first = crate::budget::estimate(&prompt[start + "C1 · ".len()..end]);
    let room = Summary {
        window_tokens: first + 13,
        ..Summary::default()
    };
    let prompt = h.ask(&room, R).unwrap().prompt;
    assert!(
        prompt.contains("\"Parser\"") && !prompt.contains("\"Lexer\""),
        "{prompt}"
    );
    // Its own call, and another conversation's reply, link nothing.
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    let reply = json!({"assistant": format!("Here is the task: {delegated}")});
    h.s.event("reply", "s1", (R, "main"), h.t + MINUTE, reply);
    h.s.event(
        "tool",
        "s2",
        (R, "main"),
        h.t + MINUTE + 1_000,
        call(&delegated),
    );
    h.said("s2", R, 2, &delegated);
    let (prompt, shown) = links(&h);
    assert!(!shown.contains(" sent "), "{prompt}");
}

/// #692: a shown delegation link puts the tool record that proved it in the grouping op's read
/// spans, so a forget of that record alone takes the op that showed the link (S5).
#[test]
fn the_record_that_proves_a_delegation_is_read_and_its_forget_takes_the_op() {
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    let input = json!({"command": format!("codex exec '{delegated}'")}).to_string();
    let call = json!({"tool": "Bash", "input": input, "output": "ok", "failed": false});
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    let tool = h.s.event("tool", "s1", (R, "main"), h.t + MINUTE, call);
    h.said("s2", R, 2, &delegated);
    let ask = h.ask(&Summary::default(), R).unwrap();
    assert!(ask.read.spans.contains(&(tool, tool)), "{:?}", ask.read);
    let both = answer(&[group(&["C1", "C2"], "The parser")], &[]);
    h.pass(&|_| both.clone());
    let (device, home) = (h.s.raw.device().to_owned(), h.s.home.path().to_owned());
    let Home { s, .. } = h;
    let crate::search::b::fixture::Store { home: _kept, raw } = s;
    drop(raw);
    let target = crate::forget::Target::Record { device, seq: tool };
    crate::forget::start(&home, &crate::forget::preview(&home, target).unwrap()).unwrap();
    crate::forget::purge(&home).unwrap();
    let bodies: Vec<Value> = rusqlite::Connection::open(home.join("raw.db"))
        .unwrap()
        .prepare("SELECT body FROM ops WHERE type = 'my_work'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|b| serde_json::from_str(&b.unwrap()).unwrap())
        .collect();
    let grouped = (bodies.iter())
        .find(|b| b["job"] == "group" && !b["skipped"].as_bool().unwrap_or(false))
        .or_else(|| bodies.iter().find(|b| b["forgotten"].is_string()));
    assert!(
        grouped.is_some_and(|b| b["forgotten"].is_string()),
        "{bodies:?}"
    );
}

/// #692: a call that starts another agent and waits for it is recorded at its end, after that
/// agent's opening ask. Its start is the time compared: begun before the ask, it sent it, and
/// its record is in the op's read spans; begun after it, or with no start, it sent nothing. A
/// call that ran over a day is compared at its end (#709).
#[test]
fn a_waiting_call_that_began_before_the_ask_sent_it() {
    let delegated = format!(
        "Review the parser change in src/parse.rs and report each finding with its line. {}",
        "Check the error paths, the tables kept between runs and the tests. ".repeat(3)
    );
    let sent = |begun: Option<i64>, ended: i64| {
        let mut h = Home::new();
        h.said("s1", R, 0, "Build the parser.");
        h.said("s2", R, 2, &delegated);
        let input = json!({"command": format!("codex exec '{delegated}'")}).to_string();
        let mut call = json!({"tool": "Bash", "input": input, "output": "ok", "failed": false});
        if let Some(ms) = begun {
            call["started"] = json!(h.t + ms);
        }
        let tool = h.s.event("tool", "s1", (R, "main"), h.t + ended, call);
        let ask = h.ask(&Summary::default(), R).unwrap();
        let at = ask.prompt.find("## Proven links").unwrap();
        let linked = ask.prompt[at..].contains("- C1 sent C2's opening ask in a tool call\n");
        assert!(
            !linked || ask.read.spans.contains(&(tool, tool)),
            "{:?}",
            ask.read
        );
        linked
    };
    assert!(sent(Some(MINUTE), 3 * MINUTE));
    assert!(!sent(Some(2 * MINUTE + 1_000), 3 * MINUTE));
    assert!(!sent(None, 3 * MINUTE));
    let day = crate::typed::LONGEST_CALL_MS;
    assert!(sent(Some(62 * MINUTE - day), 62 * MINUTE));
    assert!(!sent(Some(2 * MINUTE - 2 * day), 62 * MINUTE));
}

/// A8, C5: a repository every entry fails is kept as a skip after three attempts, and is not
/// asked again until a new conversation comes, which is asked with the one given up on.
#[test]
fn a_skip_waits_for_a_new_conversation() {
    let mut h = Home::new();
    h.said("s1", R, 0, "Build the parser.");
    let wrong = answer(&[], &[]);
    for _ in 0..crate::curate::ATTEMPTS {
        h.pass(&|_| wrong.clone());
    }
    let k = crate::knowledge::open(h.s.home.path()).unwrap();
    let skipped: bool = k
        .query_row("SELECT skipped FROM my_work WHERE job = 'group'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(skipped);
    h.age(31);
    assert_eq!(h.pass(&|_| panic!("given up")).1.len(), 0);
    h.said("s2", R, 5, "Add tests for the parser.");
    let both = answer(&[group(&["C1", "C2"], "The parser")], &[]);
    let (_, sent) = h.pass(&|_| both.clone());
    assert!(sent[0].contains("Build the parser.") && sent[0].contains("Add tests"));
}

/// Slice 3a-2's measurement (docs/my-work-model.md As built): on a copy of a store, never the
/// live one, how long grouping's due check and one call's prompt take to read, and their
/// sizes; no model call. By hand: `OBOETE_MEASURE_HOME=<copy> cargo test --release --bin
/// oboete -- --ignored --nocapture measure_grouping`.
#[test]
#[ignore]
fn measure_grouping_on_a_copy_of_a_store() {
    let home = std::env::var("OBOETE_MEASURE_HOME").expect("a copy of a store");
    let home = std::path::Path::new(&home);
    let raw = crate::raw::open(home).unwrap();
    let k = crate::knowledge::open(home).unwrap();
    let rules = Rules::load(home).unwrap();
    let summary = Summary::default();
    let reading = Reading::now(&raw, Reads::Live).unwrap();
    let stores = Stores {
        raw: &raw,
        k: &k,
        rules: &rules,
        summary: &summary,
        reading: &reading,
        held: &crate::my_work_run::NOT_HELD,
    };
    let now = crate::db::now_ms();
    let started = std::time::Instant::now();
    let seen = crate::my_work::seen(&raw, &k, &rules).unwrap();
    let read = started.elapsed();
    let g = groups(&raw, &k, &reading).unwrap();
    let all = candidates(&seen, &g, raw.device(), now);
    let sizes: Vec<usize> = all.values().map(Vec::len).collect();
    println!(
        "seen: {} conversations in {read:?}; candidates by repository: {sizes:?}",
        seen.len()
    );
    let job = Grouping::default();
    let started = std::time::Instant::now();
    let due = job.due(&stores, now).unwrap();
    println!(
        "due check, its read included: {} due in {:?}",
        due.len(),
        started.elapsed()
    );
    for d in due.iter().take(3) {
        let started = std::time::Instant::now();
        let ask = job.ask(&stores, &d.about, now).unwrap().unwrap();
        println!(
            "one call: {} characters, {} tokens estimated, {} records shown, read in {:?}",
            ask.prompt.chars().count(),
            crate::budget::estimate(&ask.prompt),
            ask.read.spans.len(),
            started.elapsed()
        );
    }
    let peak = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    println!(
        "{}",
        peak.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("")
    );
}
