use super::*;
use crate::config::Summary;
use crate::curate::{Phase, Reading, Reads};
use crate::my_work_ops::MyWorkOp;
use crate::my_work_run::Runner;
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
use crate::raw::OpKind;
use crate::search::b::fixture::Store;

const MINUTE: i64 = 60_000;

struct Home {
    s: Store,
}

impl Home {
    fn new() -> Self {
        Self { s: Store::new() }
    }

    /// A `job` op about `about` keeping `fields`, written `ago` minutes ago: its op seq.
    fn op(&mut self, job: &str, about: &str, ago: i64, fields: Value) -> i64 {
        let op = MyWorkOp {
            job: job.into(),
            about: about.into(),
            entry: "fake".into(),
            ts: crate::db::now_ms() - ago * MINUTE,
            read: Read::default(),
            removed: vec![],
            fields: fields.as_object().unwrap().clone().into_iter().collect(),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        let seqs = self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        self.s.run();
        seqs[0]
    }

    /// A review of `request` with `milestones` (date, text), in progress at 40%.
    fn reviewed(&mut self, request: &str, milestones: &[(&str, &str)]) -> i64 {
        let milestones: Vec<Value> = (milestones.iter())
            .map(|(date, text)| json!({"date": date, "text": text}))
            .collect();
        let fields = json!({"state": "in-progress", "percent": 40, "summary": "The parser.",
                "milestones": milestones, "started": null, "shipped": null});
        self.op(crate::my_work_progress::NAME, request, 0, fields)
    }

    fn with<T>(&self, f: impl FnOnce(&Stores) -> T) -> T {
        self.with_as(&Summary::default(), &crate::my_work_run::NOT_HELD, f)
    }

    fn with_as<T>(
        &self,
        summary: &Summary,
        held: &BTreeSet<(String, String)>,
        f: impl FnOnce(&Stores) -> T,
    ) -> T {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let rules = Rules::load(self.s.home.path()).unwrap();
        let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
        f(&Stores {
            raw: &self.s.raw,
            k: &k,
            rules: &rules,
            summary,
            reading: &reading,
            held,
        })
    }

    fn due(&self) -> Vec<Due> {
        self.with(|s| Story::default().due(s, crate::db::now_ms()).unwrap())
    }

    /// One pass of a runner of the story job alone, the chain answering a chapter for each
    /// week the prompt shows and a recap: what it did, and the prompts sent.
    fn pass(&mut self) -> (Phase, Vec<String>) {
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
            let output = written(prompt);
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
            vec![Box::new(Story::default()) as Box<dyn Job>],
            Box::new(call),
            (&rules, &summary, "chain"),
        );
        let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
        drop(runner);
        self.s.run();
        (phase, sent.into_inner())
    }

    /// Every story op, the newest first.
    fn ops(&self) -> Vec<Kept> {
        self.s.run();
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        let none = HashSet::new();
        crate::my_work_ops::shown(&k, (NAME, None), None, &none, false, usize::MAX).unwrap()
    }

    /// The story ops as if written `minutes` earlier.
    fn age(&self, minutes: i64) {
        let k = crate::knowledge::open(self.s.home.path()).unwrap();
        k.execute(
            "UPDATE my_work SET ts = ts - ?1 WHERE job = ?2",
            rusqlite::params![minutes * MINUTE, NAME],
        )
        .unwrap();
    }
}

/// The weeks a prompt shows, by their Mondays.
fn weeks_of(prompt: &str) -> Vec<String> {
    (prompt.lines())
        .filter_map(|l| l.strip_prefix("## The week "))
        .map(|l| l[..10].to_owned())
        .collect()
}

/// An answer writing each week the prompt shows and a recap.
fn written(prompt: &str) -> Value {
    let chapters: Vec<Value> = (weeks_of(prompt).into_iter())
        .map(|w| json!({"week": w, "text": format!("The week of {w}.")}))
        .collect();
    json!({"chapters": chapters, "recap": "It ships weekly."})
}

fn kept(ts: i64, fields: Value) -> Kept {
    Kept {
        device: "d".into(),
        op_seq: 1,
        op: MyWorkOp {
            job: crate::my_work_progress::NAME.into(),
            about: "g:1".into(),
            entry: String::new(),
            ts,
            read: Read::default(),
            removed: vec![],
            fields: fields.as_object().unwrap().clone().into_iter().collect(),
            skipped: false,
            forgotten: None,
        },
    }
}

/// Q1 of the slice 7 plan: a milestone and the start and ship days go in the weeks of their own
/// dates, the state and percent in the week of the review.
#[test]
fn facts_go_in_the_weeks_of_their_dates() {
    let ts = chrono::Utc
        .with_ymd_and_hms(2026, 10, 27, 12, 0, 0)
        .unwrap()
        .timestamp_millis();
    let review = kept(
        ts,
        json!({"state": "shipped", "percent": 100, "started": "2026-10-05",
            "shipped": "2026-10-21", "milestones": [{"date": "2026-10-14", "text": "It parses."}]}),
    );
    let mut weeks: Vec<(String, &str)> = (facts("g:1", &review, &chrono::Utc).into_iter())
        .map(|(w, f)| (w, f.kind))
        .collect();
    weeks.sort();
    assert_eq!(
        weeks,
        [
            ("2026-10-05".to_owned(), "started"),
            ("2026-10-12".to_owned(), "milestone"),
            ("2026-10-19".to_owned(), "shipped"),
            ("2026-10-26".to_owned(), "state"),
        ]
    );
}

/// P6, P7: a first story writes the two newest weeks and the recap, then at once the week left;
/// a review that moves one week's facts rewrites that week alone, three hours after the run.
#[test]
fn only_the_weeks_whose_facts_changed_are_rewritten() {
    let mut h = Home::new();
    h.reviewed(
        "g:1",
        &[
            ("2026-09-02", "It reads a line."),
            ("2026-09-09", "It fails fast."),
        ],
    );
    let (_, sent) = h.pass();
    assert_eq!(sent.len(), 1);
    let first = weeks_of(&sent[0]);
    assert_eq!(first.len(), 2, "two weeks a call: {}", sent[0]);
    assert!(
        first.contains(&"2026-09-07".to_owned()),
        "the newer milestone's week first"
    );
    let (_, sent) = h.pass();
    assert_eq!(weeks_of(&sent[0]), ["2026-08-31"], "the week left, at once");
    assert!(h.due().is_empty());
    let week_one = (h.ops().into_iter())
        .find(|k| k.op.about == "2026-08-31")
        .unwrap();
    h.reviewed(
        "g:1",
        &[
            ("2026-09-02", "It reads a line."),
            ("2026-09-09", "It fails fast."),
            ("2026-09-10", "It fails fast on a bad footer."),
        ],
    );
    let due = h.due();
    let last = h.ops()[0].op.ts;
    assert_eq!(due.len(), 1);
    assert_eq!(
        (due[0].about.as_str(), due[0].since),
        ("2026-09-07", last + AGAIN_MS)
    );
    h.age(4 * 60);
    let (_, sent) = h.pass();
    assert_eq!(weeks_of(&sent[0]), ["2026-09-07"], "{}", sent[0]);
    assert!(sent[0].contains("It fails fast on a bad footer."));
    let again = (h.ops().into_iter())
        .find(|k| k.op.about == "2026-08-31")
        .unwrap();
    assert_eq!(
        again.op_seq, week_one.op_seq,
        "the week whose facts held is not rewritten"
    );
    assert!(h.due().is_empty());
}

/// A5: what the job read is kept for the round, and `Job::reread` drops it. Here
/// a milestone a review kept since is in its next read, and not while the first is
/// kept. The runner tells every job so when the list changed between a round's two passes
/// (`my_work_run`).
#[test]
fn the_job_keeps_its_read_until_it_is_told_to_read_again() {
    let mut h = Home::new();
    let job = Story::default();
    let due = |h: &Home| h.with(|s| job.due(s, crate::db::now_ms()).unwrap().len());
    assert_eq!(due(&h), 0);
    h.reviewed("g:1", &[("2026-09-02", "It reads a line.")]);
    assert_eq!(due(&h), 0, "kept for the round");
    job.reread();
    assert_eq!(due(&h), 1, "read again");
}

/// P7: unlike claude-mem's, a story behind its reviews is due by itself, three hours after the
/// last run; with none written, at once.
#[test]
fn a_story_behind_its_reviews_is_due_by_itself() {
    let mut h = Home::new();
    assert!(h.due().is_empty(), "no review, no story");
    let review = h.reviewed("g:1", &[("2026-09-02", "It reads a line.")]);
    let due = h.due();
    let review_ts = h.with(|s| {
        let none = HashSet::new();
        let k = crate::my_work_ops::shown(
            s.k,
            (crate::my_work_progress::NAME, None),
            None,
            &none,
            true,
            1,
        );
        k.unwrap()[0].op.ts
    });
    assert_eq!(due.len(), 1);
    assert!(due[0].since <= review_ts && review > 0, "at once");
    h.pass();
    h.pass();
    assert!(h.due().is_empty());
    h.reviewed("g:1", &[("2026-09-02", "It reads a line on Windows.")]);
    let last = h.ops()[0].op.ts;
    let due = h.due();
    assert_eq!(due.len(), 1, "behind its reviews");
    assert_eq!(due[0].since, last + AGAIN_MS);
    let (_, sent) = h.pass();
    assert!(sent.is_empty(), "not before three hours");
}

/// P6, A10: each week shown once and no other, every text there, a chapter within 1,600
/// characters and the recap within 500.
#[test]
fn the_check_holds_the_weeks_and_the_caps() {
    let weeks = vec!["2026-09-07".to_owned(), "2026-08-31".to_owned()];
    let answer = |chapters: Value, recap: &str| json!({"chapters": chapters, "recap": recap});
    let both = json!([{"week": "2026-09-07", "text": "A."}, {"week": "2026-08-31", "text": "B."}]);
    assert_eq!(check(&answer(both.clone(), "R."), &weeks), None);
    let one = json!([{"week": "2026-09-07", "text": "A."}]);
    assert_eq!(check(&answer(one, "R."), &weeks), Some("shape"));
    let other = json!([{"week": "2026-09-07", "text": "A."}, {"week": "2026-09-14", "text": "B."}]);
    assert_eq!(check(&answer(other, "R."), &weeks), Some("shape"));
    let long = "x".repeat(CHAPTER_CHARS + 1);
    let over = json!([{"week": "2026-09-07", "text": long}, {"week": "2026-08-31", "text": "B."}]);
    assert_eq!(check(&answer(over, "R."), &weeks), Some("over_cap"));
    let recap = "x".repeat(RECAP_CHARS + 1);
    assert_eq!(
        check(&answer(both.clone(), &recap), &weeks),
        Some("over_cap")
    );
    assert_eq!(check(&answer(both, " "), &weeks), Some("empty"));
}

/// P6, V4: My Work's answer has no story before one is written; then the recap and the
/// chapters, the newest week first.
#[test]
fn my_work_shows_the_recap_and_the_chapters_newest_first() {
    let mut h = Home::new();
    let read = |h: &Home| {
        h.s.run();
        let now = crate::db::now_ms();
        let v = crate::my_work::read(h.s.home.path(), crate::my_work::Range::All, now);
        v.unwrap()["story"].clone()
    };
    assert_eq!(read(&h), Value::Null);
    h.reviewed(
        "g:1",
        &[
            ("2026-09-02", "It reads a line."),
            ("2026-09-09", "It fails fast."),
        ],
    );
    h.pass();
    h.pass();
    let story = read(&h);
    assert_eq!(story["recap"], "It ships weekly.");
    let weeks: Vec<&str> = (story["chapters"].as_array().unwrap().iter())
        .map(|c| c["week"].as_str().unwrap())
        .collect();
    // This week's, of the review's state, first; then the milestones' weeks.
    assert_eq!(weeks.len(), 3, "{story}");
    assert_eq!(weeks[1..], ["2026-09-07", "2026-08-31"], "{story}");
}

/// This device's local date today, `YYYY-MM-DD`.
fn today() -> String {
    local_date(crate::db::now_ms(), &chrono::Local)
}

/// #733: a call asks for the chapters of the weeks it shows, whatever the window leaves out,
/// and an answer writing each of them passes the check.
#[test]
fn a_call_asks_only_for_the_weeks_it_shows() {
    let mut h = Home::new();
    let long = "x".repeat(150);
    let mut milestones: Vec<(String, String)> = (0..30).map(|_| (today(), long.clone())).collect();
    milestones.push(("2026-09-02".into(), "It reads a line.".into()));
    let m: Vec<(&str, &str)> = milestones
        .iter()
        .map(|(d, t)| (d.as_str(), t.as_str()))
        .collect();
    h.reviewed("g:1", &m);
    let about = h.due()[0].about.clone();
    let mut fewer = false;
    for window in (60..2_400).step_by(37) {
        let summary = Summary {
            window_tokens: window,
            ..Summary::default()
        };
        let none = &crate::my_work_run::NOT_HELD;
        let ask = h.with_as(&summary, none, |s| {
            Story::default()
                .ask(s, &about, crate::db::now_ms())
                .unwrap()
        });
        let Some(ask) = ask else { continue };
        let mut shown = weeks_of(&ask.prompt);
        fewer |= shown.len() < 2;
        shown.push(RECAP.to_owned());
        assert_eq!(ask.subjects, shown, "window {window}");
        assert_eq!((ask.check)(&written(&ask.prompt)), None, "window {window}");
    }
    assert!(fewer, "a window leaves the second week out");
}

/// #730, #733: a chapter a failed batch holds is in no batch until its wait ends; while the
/// recap is held, which every batch rewrites, none is due.
#[test]
fn a_held_chapter_waits_out_its_hold() {
    let mut h = Home::new();
    h.reviewed("g:1", &[("2026-09-02", "It reads a line.")]);
    let due = |held: &[&str]| {
        let held: BTreeSet<(String, String)> = held
            .iter()
            .map(|a| (NAME.to_owned(), (*a).to_owned()))
            .collect();
        let summary = Summary::default();
        h.with_as(&summary, &held, |s| {
            (Story::default()
                .due(s, crate::db::now_ms())
                .unwrap()
                .into_iter())
            .map(|d| d.about)
            .collect::<Vec<_>>()
        })
    };
    let this = monday(&today()).unwrap();
    assert_eq!(due(&[]), std::slice::from_ref(&this));
    assert_eq!(due(&[&this]), ["2026-08-31"], "the held week waits");
    assert_eq!(
        due(&[RECAP]),
        Vec::<String>::new(),
        "no batch while the recap waits"
    );
}

/// #733: a week whose facts a newer review took away loses its chapter on the page.
#[test]
fn a_week_whose_facts_are_gone_shows_no_chapter() {
    let mut h = Home::new();
    h.reviewed("g:1", &[("2026-09-02", "It reads a line.")]);
    h.pass();
    let story = |h: &Home| {
        h.s.run();
        let now = crate::db::now_ms();
        let v = crate::my_work::read(h.s.home.path(), crate::my_work::Range::All, now).unwrap();
        (v["story"]["chapters"].as_array().unwrap().iter())
            .map(|c| c["week"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(story(&h).contains(&"2026-08-31".to_owned()));
    h.reviewed("g:1", &[("2026-09-09", "It reads a line.")]);
    assert!(
        !story(&h).contains(&"2026-08-31".to_owned()),
        "{:?}",
        story(&h)
    );
}

/// #733: after a give-up's skips, the next call is shown the last answered recap and chapter
/// as the story so far.
#[test]
fn the_story_so_far_is_the_last_answered_text() {
    let mut h = Home::new();
    h.reviewed("g:1", &[("2026-09-02", "It reads a line.")]);
    let this = monday(&today()).unwrap();
    h.op(NAME, RECAP, 30, json!({"text": "Recap A."}));
    h.op(NAME, &this, 30, json!({"text": "Chapter A.", "digest": 0}));
    for about in [RECAP, this.as_str()] {
        let skip = MyWorkOp {
            job: NAME.into(),
            about: about.into(),
            entry: String::new(),
            ts: crate::db::now_ms() - 20 * MINUTE,
            read: Read::default(),
            removed: vec![],
            fields: BTreeMap::new(),
            skipped: true,
            forgotten: None,
        };
        let op = serde_json::to_value(skip).unwrap();
        h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
    }
    h.reviewed("g:2", &[("2026-09-03", "It fails fast.")]);
    h.age(4 * 60);
    let (_, sent) = h.pass();
    let prompt = sent.first().expect("a call");
    assert!(
        prompt.contains("## The recap so far\nRecap A.\n"),
        "{prompt}"
    );
    assert!(
        prompt.contains("The chapter so far:\nChapter A.\n"),
        "{prompt}"
    );
}
