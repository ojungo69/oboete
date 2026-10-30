//! The request judge (docs/my-work-model.md J2, J3): before grouping, each new conversation once,
//! as claude-mem 15.4's `routeUndecided` routes it. A single proven link to one open request joins
//! it with no call (claude-mem's rules judge); with no open request it is a request of its own;
//! else Jev picks among the repository's 19 most recently active open requests or none, and checks
//! the pick: from 0.8 it joins, from 0.5 it is its own request with a suggestion to join, below its
//! own request. A call that fails leaves it to grouping, and it is not asked again (J1).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use serde_json::{Value, json};

use crate::judge::{Ask, Asked};
use crate::my_work::Seen;
use crate::my_work_group::{Group, Groups, Key, Proof};
use crate::my_work_ops::{MyWorkOp, Read};
use crate::my_work_run::{Judging, Stores};
use crate::redact::Rules;

/// The ops' job (S1).
pub(crate) const NAME: &str = "judge";
/// The entry of a decision made with no call; Jev's is `judge::NAME`.
const RULES: &str = "judge:rules";

// claude-mem's request judge (request-judge.mjs, 15.4.0, as 15.7.0 has it), word for word.
const CANDIDATES: usize = 19;
const JOIN: f64 = 0.8;
const SUGGEST: f64 = 0.5;
const ASKS: usize = 3;
const ASK_CHARS: usize = 250;
const TITLE_CHARS: usize = 120;
const PICK: &str = "`conversation` was started after each of `requests`. Which of `requests` does \
it continue: the same overall piece of work (same feature, page, skill, release or investigation), \
or another run of the same recurring routine? A person tracking their requests would want both on \
one card. Pick `none` if it is a separate ask of its own.";
const CHECK: &str = "`conversation` was started after `request`. It continues `request`: the same \
overall piece of work (same feature, page, skill, release or investigation), or another run of the \
same recurring routine. A person tracking their requests would want both on one card.";
const NONE: &str = "None of these: a separate ask of its own";
/// claude-mem's `ownTitle` when a request has no title and its ask makes none.
const UNTITLED: &str = "Untitled request";
/// What is gated of a text before it is cleaned and cut, in characters.
const WINDOW: usize = 2_000;

#[cfg(test)]
thread_local! {
    /// A smaller op cap a test sets, in `raw::MAX_OP_BYTES`'s place.
    pub(crate) static CAP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// What the judge was shown of this device's records and ops (S2): the prompt records, and the
/// card and grouping ops whose text it read.
#[derive(Default, Clone)]
struct Shown {
    spans: BTreeSet<i64>,
    ops: BTreeSet<i64>,
}

/// The earliest candidate (C1) no judge op names yet, judged (J3): the op that says what was
/// decided, or that the conversation was left to grouping; none when no conversation is left to
/// judge, or when the judge did not ask (its repository excluded on the way, or its key gone: the
/// next round reads it again).
pub(crate) fn request(
    s: &Stores,
    seen: &[Seen],
    groups: &Groups,
    judge: &mut Judging,
    now: i64,
) -> Result<Vec<MyWorkOp>> {
    let device = s.raw.device();
    let candidates = crate::my_work_group::candidates(seen, groups, device, now);
    let Some(c) = (candidates.values().flatten())
        .filter(|c| !groups.asked.contains(&c.key))
        .min_by_key(|c| c.ask.as_ref().map(|a| (a.2, a.1)))
        .copied()
    else {
        return Ok(Vec::new());
    };
    let (Some(repo), Some((_, opening, ..))) = (&c.repo, &c.ask) else {
        return Ok(Vec::new());
    };
    let by_key: HashMap<&Key, &Seen> = seen.iter().map(|c| (&c.key, c)).collect();
    let open: Vec<(&Key, &Group)> = (groups.groups.iter())
        .filter(|(_, g)| g.repo == *repo)
        .collect();
    let mut shown = Shown::default();
    shown.spans.insert(*opening);
    let op_of = |shown: Shown, entry: &str, fields: Vec<(&str, Value)>| -> Result<MyWorkOp> {
        let mut fields: BTreeMap<String, Value> = (fields.into_iter())
            .map(|(name, v)| (name.to_owned(), v))
            .collect();
        fields.insert("conversation".into(), key(&c.key));
        let top = s.raw.max_seq_of(device)?;
        let mut removed = Vec::new();
        for &seq in &shown.spans {
            removed.extend(s.raw.removed_in(device, seq, seq, Some(top))?);
        }
        removed.sort();
        removed.dedup();
        Ok(MyWorkOp {
            job: NAME.into(),
            about: repo.clone(),
            entry: entry.into(),
            ts: crate::db::now_ms(),
            read: Read {
                spans: shown.spans.iter().map(|&seq| (seq, seq)).collect(),
                ops: shown.ops.into_iter().collect(),
            },
            removed,
            fields,
            skipped: false,
            forgotten: None,
        })
    };
    // Too big for an op's cap, a decision would be kept as a skip that names no conversation, and
    // the judge would ask for it again each round: it is left to grouping instead.
    let too_big = || {
        let shown = Shown {
            spans: BTreeSet::from([*opening]),
            ops: BTreeSet::new(),
        };
        op_of(
            shown,
            crate::judge::NAME,
            vec![("outcome", json!("too_big"))],
        )
    };
    let decided =
        |shown: Shown, entry: &str, fields: Vec<(&str, Value)>| -> Result<Vec<MyWorkOp>> {
            let op = op_of(shown, entry, fields)?;
            Ok(vec![if fits(&op)? { op } else { too_big()? }])
        };
    let own = || json!([{"conversations": [key(&c.key)], "title": null, "links": []}]);
    let joined = |anchor: &Key, kind: &str, evidence: String| {
        let ends = json!([key(anchor), key(&c.key)]);
        json!([{"conversations": ends, "title": null,
            "links": [{"conversations": ends, "kind": kind, "evidence": evidence}]}])
    };
    let group_ops = |anchor: &Key, shown: &mut Shown| {
        if let Some((_, g)) = open.iter().find(|(a, _)| *a == anchor) {
            let mine = g.ops.iter().filter(|(d, _)| d == device);
            shown.ops.extend(mine.map(|(_, seq)| *seq));
        }
    };

    // claude-mem's rules judge: a single open request a proven link names (C3) joins, no call.
    let (_, senders) = crate::my_work_group::agent_sent(s.raw, &[c])?;
    let (links, linked) = crate::my_work_group::proven(repo, &[c], &open, &by_key, &senders);
    if let [anchor] = linked.iter().collect::<Vec<_>>()[..] {
        let proof = (links.iter()).find_map(|(proof, ends)| ends.contains(anchor).then_some(proof));
        let how = match proof {
            Some(Proof::Sent(record)) => {
                shown.spans.insert(*record);
                "a conversation of it sent this conversation's opening ask in a tool call"
            }
            _ => "a work list both wrote",
        };
        group_ops(anchor, &mut shown);
        let evidence =
            format!("Joined with no call: one proven link, to this request alone: {how}.");
        return decided(
            shown,
            RULES,
            vec![
                ("outcome", json!("rules")),
                ("groups", joined(anchor, "rules", evidence)),
            ],
        );
    }
    // claude-mem's Jev judge with no open request to continue: a request of its own, no call.
    if open.is_empty() {
        return decided(
            shown,
            crate::judge::NAME,
            vec![("outcome", json!("alone")), ("groups", own())],
        );
    }

    // The pick, over the 19 most recently active open requests (J3).
    let newest = |anchor: &Key, g: &Group| {
        (g.members.iter().chain([anchor]))
            .filter_map(|m| by_key.get(m).map(|c| c.last))
            .max()
            .unwrap_or(0)
    };
    let mut ranked: Vec<&(&Key, &Group)> = open.iter().collect();
    ranked.sort_by_key(|(a, g)| std::cmp::Reverse(newest(a, g)));
    ranked.truncate(CANDIDATES);
    let considered = if ranked.len() == open.len() {
        let s = if open.len() == 1 { "" } else { "s" };
        format!("{} open request{s}", open.len())
    } else {
        format!(
            "the {} most recently active of {} open requests",
            ranked.len(),
            open.len()
        )
    };
    let conversation = context(s, c, &mut shown);
    let mut requests = serde_json::Map::new();
    let mut criteria = serde_json::Map::new();
    let mut labels: BTreeMap<String, (&Key, Value)> = BTreeMap::new();
    for (i, (anchor, g)) in ranked.iter().enumerate() {
        let label = format!("r{}", i + 1);
        let anchor_seen = by_key.get(*anchor).copied();
        let context = match anchor_seen {
            Some(a) => context(s, a, &mut shown),
            None => json!({"worktree": repo, "started": null, "asks": [], "agent_recorded": []}),
        };
        let asked = anchor_seen.and_then(|a| a.ask.as_ref());
        let title = (g.title.clone())
            .or_else(|| {
                asked.map(|a| crate::my_work::title(&crate::my_work::shown(&a.3, WINDOW, s.rules)))
            })
            .filter(|t| !t.trim().is_empty());
        if g.title.is_none()
            && let Some((d, seq, ..)) = asked
            && d == device
        {
            shown.spans.insert(*seq);
        }
        let title = title.map_or_else(
            || UNTITLED.to_owned(),
            |t| crate::my_work::shown(&t, TITLE_CHARS, s.rules),
        );
        group_ops(anchor, &mut shown);
        // claude-mem's request: its title, then its anchor's context.
        let mut request = json!({"title": title});
        if let (Some(r), Some(c)) = (request.as_object_mut(), context.as_object()) {
            r.extend(c.clone());
        }
        criteria.insert(label.clone(), json!(format!("`requests.{label}`: {title}")));
        requests.insert(label.clone(), request.clone());
        labels.insert(label, (*anchor, request));
    }
    criteria.insert("none".into(), json!(NONE));
    let evidence = |p: f64, score: f64| {
        format!(
            "Jev: picked over {considered} (p {p:.2}); the check \"continues that request\" \
             scored {score:.2}."
        )
    };
    // Nothing is paid for that could not be kept: the largest decision this read can make.
    let largest = vec![
        ("outcome", json!("suggested")),
        ("groups", joined(ranked[0].0, "judge", evidence(1.0, 1.0))),
        ("suggestion", json!({"request": key(&c.key), "score": 1.0})),
    ];
    if !fits(&op_of(shown.clone(), crate::judge::NAME, largest)?)? {
        return Ok(vec![too_big()?]);
    }
    let span = format!("{NAME} {repo}");
    let repos = [repo.clone()];
    let picked = judge(Ask {
        span: &span,
        state: json!({"conversation": conversation, "requests": requests}),
        questions: json!({"pick": {"type": "choice", "instructions": PICK, "criteria": criteria}}),
        repos: &repos,
    })?;
    let pick = match picked {
        Asked::Answered(answers) => answers["pick"].clone(),
        Asked::Skipped(why) => return left(why, shown, decided),
    };
    let choice = pick["choice"].as_str().unwrap_or("none");
    let p = pick["probabilities"][choice].as_f64().unwrap_or(0.0);
    let Some((anchor, request)) = labels.get(choice) else {
        return decided(
            shown,
            crate::judge::NAME,
            vec![("outcome", json!("none")), ("groups", own())],
        );
    };

    // The check of the pick: its score is the band (claude-mem's `bandFor`).
    let checked = judge(Ask {
        span: &span,
        state: json!({"request": request, "conversation": conversation}),
        questions: json!({"same": {"type": "noul", "instructions": CHECK,
            "criteria": {"true": "Same work", "false": "Different work"}}}),
        repos: &repos,
    })?;
    let score = match checked {
        Asked::Answered(answers) => answers["same"]["noul"].as_f64().unwrap_or(0.0),
        Asked::Skipped(why) => return left(why, shown, decided),
    };
    let fields = if score >= JOIN {
        vec![
            ("outcome", json!("joined")),
            ("groups", joined(anchor, "judge", evidence(p, score))),
        ]
    } else if score >= SUGGEST {
        vec![
            ("outcome", json!("suggested")),
            ("groups", own()),
            (
                "suggestion",
                json!({"request": key(anchor), "score": score}),
            ),
        ]
    } else {
        vec![("outcome", json!("own")), ("groups", own())]
    };
    decided(shown, crate::judge::NAME, fields)
}

/// What a call the judge did not answer leaves (J1): a refused or failed one an op naming the
/// conversation and why, with no decision, so grouping decides it and the judge does not ask
/// again; none when nothing went because the judge was off, its key gone or the repository
/// excluded on the way, as the next round reads each.
fn left(
    why: &'static str,
    shown: Shown,
    decided: impl FnOnce(Shown, &str, Vec<(&str, Value)>) -> Result<Vec<MyWorkOp>>,
) -> Result<Vec<MyWorkOp>> {
    if matches!(why, "off" | "key" | "excluded") {
        return Ok(Vec::new());
    }
    decided(shown, crate::judge::NAME, vec![("outcome", json!(why))])
}

/// A conversation as claude-mem's `contextReader` gives it (J2): its repository, the minute it
/// started, its first asks and its first cards' titles, each cut at the attached files, its white
/// space collapsed, gated and cut; what it reads of this device's records and ops in `shown`.
fn context(s: &Stores, c: &Seen, shown: &mut Shown) -> Value {
    let device = s.raw.device();
    let mut asks = Vec::new();
    for (d, seq, _, text) in c.asks.iter().take(ASKS) {
        let text = said(text, ASK_CHARS, s.rules);
        if text.is_empty() {
            continue;
        }
        if d == device {
            shown.spans.insert(*seq);
        }
        asks.push(text);
    }
    let mut recorded = Vec::new();
    for (d, op, title) in &c.cards {
        if d == device {
            shown.ops.insert(*op);
        }
        recorded.push(said(title, TITLE_CHARS, s.rules));
    }
    let started = (c.ask.as_ref()).map(|a| crate::db::utc(a.2)[..16].replacen(' ', "T", 1));
    json!({"worktree": c.repo, "started": started, "asks": asks, "agent_recorded": recorded})
}

/// Whether `op` stays whole within an op's cap, its removals aside (`raw::fitted` drops those
/// first), with room for a decision's few words more than the one measured.
fn fits(op: &MyWorkOp) -> Result<bool> {
    #[cfg(test)]
    let cap = CAP.get().unwrap_or(crate::raw::MAX_OP_BYTES);
    #[cfg(not(test))]
    let cap = crate::raw::MAX_OP_BYTES;
    let bare = MyWorkOp {
        removed: Vec::new(),
        ..op.clone()
    };
    Ok(serde_json::to_string(&bare)?.len() + 512 <= cap)
}

/// A text as Jev is shown it (J2): gated first, as it was recorded (A5), so that a rule over
/// several lines still matches; then claude-mem's `clean`, and cut.
fn said(text: &str, max: usize, rules: &Rules) -> String {
    let gated = crate::my_work::shown(text, WINDOW, rules);
    crate::my_work::shown(&clean(&gated), max, rules)
}

/// claude-mem's `clean`: the text before `# Attached files`, its white space collapsed.
fn clean(text: &str) -> String {
    let before = text.split("# Attached files").next().unwrap_or_default();
    before.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn key((agent, session): &Key) -> Value {
    json!([agent, session])
}

#[cfg(test)]
mod tests {
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
                    key_file,
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
            let table = format!("[judge]\nkey_file = '{}'\n", self.judge.key_file.display());
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
        std::fs::write(&h.judge.key_file, "# no key\n").unwrap();
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
                        "Jev: picked over 1 open request (p 0.94); the check \"continues that \
                         request\" scored 0.90."
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
        h.config(
            "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n",
        );
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
                .contains("over the 19 most recently active of 21 open requests"),
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
                judge(Ask {
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
                let mut turning = |ask: Ask<'_>| {
                    let asked = judge(ask);
                    let config = self.1.join("config.toml");
                    let text = std::fs::read_to_string(&config).unwrap();
                    std::fs::write(&config, format!("{text}enabled = false\n")).unwrap();
                    asked
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
}
