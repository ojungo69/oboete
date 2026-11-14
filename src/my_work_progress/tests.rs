    use super::*;
    use crate::config::Summary;
    use crate::curate::{Phase, Reading, Reads};
    use crate::my_work_ops::MyWorkOp;
    use crate::my_work_run::Runner;
    use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
    use crate::raw::OpKind;
    use crate::search::b::fixture::Store;

    const R: &str = "github.com/o/r";
    const MINUTE: i64 = 60_000;

    /// A home whose records start two hours ago.
    struct Home {
        s: Store,
        t: i64,
    }

    impl Home {
        fn new() -> Self {
            Self {
                s: Store::new(),
                t: crate::db::now_ms() - 120 * MINUTE,
            }
        }

        /// A prompt of `session` at `at` minutes from the start: its seq.
        fn said(&mut self, session: &str, at: i64, text: &str) -> i64 {
            self.s.said(session, R, self.t + at * MINUTE, text)
        }

        /// A prompt another agent sent `session`, marked so at capture (O2): its seq.
        fn sent(&mut self, session: &str, at: i64, text: &str) -> i64 {
            let body = json!({"prompt": text, "agent_sent": true});
            let ts = self.t + at * MINUTE;
            self.s.event("prompt", session, (R, "main"), ts, body)
        }

        fn reply(&mut self, session: &str, at: i64) -> i64 {
            let ts = self.t + at * MINUTE;
            (self.s).event("reply", session, (R, "main"), ts, json!({"text": "Done."}))
        }

        /// A summary of `session`'s turn `(from, through)`, its reply at `at`: its op seq.
        fn summarized(&mut self, session: &str, turn: (i64, i64), at: i64, done: &str) -> i64 {
            let id = self
                .s
                .turn(json!({"agent": "claude", "session": session, "repo": R,
                "ts": self.t + at * MINUTE, "from": turn.0, "through": turn.1, "read": [],
                "goals": [], "removed": [], "fields": {"completed": done}, "skipped": false}));
            id[1..].parse().unwrap()
        }

        /// A write of `list` and the call of `session` that sent it, as oboete's server and its
        /// caller record them.
        fn wrote(&mut self, session: &str, list: &str, fields: Value) {
            let fields = fields.as_object().unwrap().clone();
            let seq = self.s.raw.work_state(R, list, &fields, None).unwrap();
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
            self.s.event("tool", session, (R, "main"), ts + 20, body);
        }

        /// A grouping op that made `members` one request, the first its anchor, shown `shown`.
        fn grouped(&mut self, members: &[&str], shown: &[i64]) {
            let keys: Vec<Value> = members.iter().map(|s| json!(["claude", s])).collect();
            let group = json!({"conversations": keys, "title": "The parser", "links": []});
            let op = MyWorkOp {
                job: "group".into(),
                about: R.into(),
                entry: "fake".into(),
                ts: crate::db::now_ms(),
                read: Read {
                    spans: shown.iter().map(|&s| (s, s)).collect(),
                    ops: vec![],
                },
                removed: vec![],
                fields: BTreeMap::from([
                    ("groups".to_owned(), json!([group])),
                    ("ignored".to_owned(), json!([])),
                ]),
                skipped: false,
                forgotten: None,
            };
            let op = serde_json::to_value(op).unwrap();
            self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        }

        fn with<T>(&self, f: impl FnOnce(&Stores) -> T) -> T {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let rules = Rules::load(self.s.home.path()).unwrap();
            let summary = Summary::default();
            let reading = Reading::now(&self.s.raw, Reads::Live).unwrap();
            f(&Stores {
                raw: &self.s.raw,
                k: &k,
                rules: &rules,
                summary: &summary,
                reading: &reading,
                held: &crate::my_work_run::NOT_HELD,
            })
        }

        fn due(&self) -> Vec<(String, i64)> {
            let due = self.with(|s| Progress::default().due(s, crate::db::now_ms()).unwrap());
            due.into_iter().map(|d| (d.about, d.since)).collect()
        }

        fn ask(&self, about: &str) -> Option<Ask> {
            self.with(|s| (Progress::default().ask(s, about, crate::db::now_ms())).unwrap())
        }

        /// One pass of a runner of the review job alone, the chain answering `answer` and
        /// refusing what the check refuses: what it did, and the prompts sent.
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
                vec![Box::new(Progress::default()) as Box<dyn Job>],
                Box::new(call),
                (&rules, &summary, "chain"),
            );
            let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
            drop(runner);
            self.s.run();
            (phase, sent.into_inner())
        }

        /// The reviews kept, the newest first.
        fn reviews(&self) -> Vec<Kept> {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let abouts: Vec<String> = (k
                .prepare("SELECT DISTINCT about FROM my_work WHERE job = ?1"))
            .unwrap()
            .query_map([NAME], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
            let mut out = Vec::new();
            for about in abouts {
                let all = crate::my_work_ops::shown(
                    &k,
                    (NAME, Some(&about)),
                    None,
                    &HashSet::new(),
                    false,
                    usize::MAX,
                );
                out.extend(all.unwrap());
            }
            out.sort_by_key(|k| std::cmp::Reverse((k.op.ts, k.op_seq)));
            out
        }

        /// A review of request `id` written `ago` minutes ago, its fields as the job keeps
        /// `answer`'s.
        fn reviewed(&mut self, id: &str, ago: i64, answer: Value) {
            self.reviewed_with(id, ago, answer, &|_| ());
        }

        /// `reviewed`, the op as `edit` leaves it.
        fn reviewed_with(
            &mut self,
            id: &str,
            ago: i64,
            answer: Value,
            edit: &dyn Fn(&mut MyWorkOp),
        ) {
            let rules = Rules::load(self.s.home.path()).unwrap();
            let mut op = MyWorkOp {
                job: NAME.into(),
                about: id.into(),
                entry: "fake".into(),
                ts: crate::db::now_ms() - ago * MINUTE,
                read: Read::default(),
                removed: vec![],
                fields: keep(&answer, &rules, &[], &Marks::default()),
                skipped: false,
                forgotten: None,
            };
            edit(&mut op);
            let op = serde_json::to_value(op).unwrap();
            self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        }

        /// The one request of repository `R` My Work shows, as the page reads it.
        fn page(&self) -> Value {
            self.s.run();
            let now = crate::db::now_ms();
            let v = crate::my_work::read(self.s.home.path(), crate::my_work::Range::All, now);
            let v = v.unwrap();
            let of_r: Vec<&Value> = (v["requests"].as_array().unwrap().iter())
                .filter(|r| r["repos"].as_array().unwrap().contains(&json!(R)))
                .collect();
            assert_eq!(of_r.len(), 1, "{v}");
            of_r[0].clone()
        }

        /// The reviews as long ago as `minutes` more, as P2 counts them.
        fn age(&self, minutes: i64) {
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            k.execute(
                "UPDATE my_work SET ts = ts - ?1 WHERE job = ?2",
                rusqlite::params![minutes * MINUTE, NAME],
            )
            .unwrap();
        }
    }

    fn reading(ts: i64, percent: i64) -> Reviewed {
        Reviewed {
            ts,
            percent,
            state: "in-progress".into(),
            shipped: None,
            narrative: None,
            milestones: Vec::new(),
        }
    }

    fn key(session: &str) -> Key {
        ("claude".to_owned(), session.to_owned())
    }

    fn review(state: &str, percent: i64, shipped: &str, preferences: &[(&str, &str)]) -> Value {
        let preferences: Vec<Value> = (preferences.iter())
            .map(|(statement, quote)| json!({"statement": statement, "quote": quote}))
            .collect();
        json!({"state": state, "percent": percent, "started": "2026-10-01", "shipped": shipped,
            "last_activity": "2026-10-26", "summary": "The parser reads lines.",
            "narrative": "The parser came first, then its tests.",
            "milestones": [{"date": "2026-10-02", "text": "The parser reads a line."}],
            "preferences": preferences})
    }

    /// P2: never reviewed, due since it was asked; then due again only after something the review
    /// could not read, from an hour after it: a summary written after it of a turn that ended
    /// before it (its time the turn's), a work item written after it.
    #[test]
    fn a_review_is_due_after_new_work_at_most_hourly() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser for the config files.");
        let b = h.said("s2", 10, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let due = h.due();
        assert_eq!(due.len(), 1, "{due:?}");
        let (id, since) = due[0].clone();
        assert!(id.starts_with("g:"), "{id}");
        assert_eq!(since, h.t);
        let (_, sent) = h.pass(&|_| review("in-progress", 40, "", &[]));
        assert_eq!(sent.len(), 1);
        let first = h.reviews();
        assert_eq!(first.len(), 1);
        assert!(h.due().is_empty(), "{:?}", h.due());
        let r = h.reply("s1", 5);
        let summary = h.summarized("s1", (a, r), 5, "The parser reads a line of a config file.");
        assert_eq!(h.due(), [(id.clone(), first[0].op.ts + AGAIN_MS)]);
        let (phase, sent) = h.pass(&|_| panic!("within the hour"));
        assert!(sent.is_empty(), "{sent:?}");
        assert!(matches!(phase, Phase::Waiting { .. }), "{phase:?}");
        h.age(61);
        let ask = h.ask(&id).unwrap();
        assert!(ask.read.ops.contains(&summary), "{:?}", ask.read);
        let (_, sent) = h.pass(&|_| review("in-progress", 60, "", &[]));
        assert!(sent[0].contains("a line of a config file"), "{}", sent[0]);
        assert!(h.due().is_empty());
        h.age(61);
        h.wrote(
            "s1",
            "parser",
            json!({"status": "doing", "task": "Report the column"}),
        );
        let due = h.due();
        assert_eq!(due.len(), 1, "{due:?}");
        let ask = h.ask(&id).unwrap();
        assert!(ask.prompt.contains("Report the column"), "{}", ask.prompt);
    }

    fn input(conversations: &[&str]) -> ReviewInput {
        ReviewInput {
            id: "g:1".into(),
            conversations: (conversations.iter())
                .map(|s| ("claude".to_owned(), (*s).to_owned()))
                .collect(),
            asked: 1_000,
            activity: 50_000,
            original: Some((AskFrom::Prompt(2), "Build the parser.".into())),
            asks: vec![(5, 40_000, "And its tests.".into())],
            openings: vec![],
            repos: vec![],
            summaries: vec![(7, 30_000, BTreeMap::new())],
            cards: vec![(9, 20_000, "The parser".into())],
            work: vec![("parser".into(), None, "doing".into(), 50_000)],
            sources: crate::my_work::Sources::default(),
            title: None,
        }
    }

    fn kept(ts: i64, marks: Option<Marks>, forgotten: bool) -> Kept {
        Kept {
            device: "d".into(),
            op_seq: 3,
            op: MyWorkOp {
                job: NAME.into(),
                about: "g:1".into(),
                entry: "fake".into(),
                ts,
                read: Read::default(),
                removed: vec![],
                fields: marks.iter().flat_map(Marks::fields).collect(),
                skipped: marks.is_none(),
                forgotten: forgotten.then(|| "forget".to_owned()),
            },
        }
    }

    /// P2's rule, each mark alone: anything above what the review could read makes it due an hour
    /// after it; a forgotten review is none (S5); a skip waits for activity after it.
    #[test]
    fn a_review_is_due_on_what_it_could_not_read() {
        let i = input(&["s1", "s2"]);
        assert_eq!(due_since(&i, None), Some(1_000));
        let read = Marks::of(&i, 60_000);
        assert_eq!(due_since(&i, Some(&kept(70_000, Some(read), false))), None);
        assert_eq!(
            due_since(&i, Some(&kept(70_000, Some(read), true))),
            Some(70_000)
        );
        let again = Some(70_000 + AGAIN_MS);
        for marks in [
            Marks { asks: 4, ..read },
            Marks {
                summaries: 6,
                ..read
            },
            Marks { cards: 8, ..read },
            Marks {
                read_at: 49_999,
                ..read
            },
            Marks::of(&input(&["s1"]), 60_000),
        ] {
            assert_eq!(
                due_since(&i, Some(&kept(70_000, Some(marks), false))),
                again
            );
        }
        assert_eq!(
            due_since(&i, Some(&kept(49_999, None, false))),
            Some(49_999 + AGAIN_MS)
        );
        assert_eq!(due_since(&i, Some(&kept(50_000, None, false))), None);
    }

    /// P4's code rules (A10): a state of the five, a whole percent from 0 to 100, dates as
    /// YYYY-MM-DD or empty, shipped with percent 100 and a shipped date and only shipped with one,
    /// and the lists within their counts.
    #[test]
    fn shipped_needs_percent_100_and_a_ship_date() {
        assert_eq!(check(&review("shipped", 100, "2026-10-20", &[])), None);
        assert_eq!(check(&review("in-progress", 100, "", &[])), None);
        for wrong in [
            review("shipped", 90, "2026-10-20", &[]),
            review("shipped", 100, "", &[]),
            review("in-progress", 50, "2026-10-20", &[]),
            review("done", 50, "", &[]),
            review("in-progress", 101, "", &[]),
            review("in-progress", -1, "", &[]),
            review("in-progress", 50, "2026-10-2", &[]),
            review("shipped", 100, "2026-02-30", &[]),
        ] {
            assert_eq!(check(&wrong), Some("shape"), "{wrong}");
        }
        let mut half = review("in-progress", 50, "", &[]);
        half["percent"] = json!(50.5);
        assert_eq!(check(&half), Some("shape"));
        let mut undated = review("in-progress", 50, "", &[]);
        undated["milestones"] = json!([{"date": "", "text": "Began."}]);
        assert_eq!(check(&undated), Some("shape"));
        let mut many = review("in-progress", 50, "", &[]);
        many["milestones"] = json!(vec![json!({"date": "2026-10-02", "text": "A step."}); 9]);
        assert_eq!(check(&many), Some("over_cap"));
        let four = [("s", "q"); 4];
        assert_eq!(
            check(&review("in-progress", 50, "", &four)),
            Some("over_cap")
        );
    }

    /// P4, O4: a preference is kept only when its quote is in a typed ask the review was shown,
    /// by curation's matcher (whitespace aside), never one an agent sent, never over a mask.
    #[test]
    fn a_preference_whose_quote_is_not_in_a_typed_prompt_is_dropped() {
        let secret = format!("ghp_{}", &"A1b2C3d4E5".repeat(4)[..36]);
        let mut h = Home::new();
        let a = h.said(
            "s1",
            5,
            "Build the parser. Keep every error message in English.",
        );
        h.said(
            "s1",
            6,
            &format!("Deploy it with {secret} and keep the logs."),
        );
        // The earliest ask, so the request's original: an agent's.
        let b = h.sent("s2", 0, "Write the parser's tests with one fixture a case.");
        h.sent("s2", 7, "Use the fixture builder in every test.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let id = h.due()[0].0.clone();
        let ask = h.ask(&id).unwrap();
        assert!(
            ask.prompt
                .contains("[agent prompt] Write the parser's tests"),
            "{}",
            ask.prompt
        );
        assert!(!ask.prompt.contains(&secret));
        assert!(
            ask.prompt
                .contains("[agent prompt] Use the fixture builder"),
            "{}",
            ask.prompt
        );
        let masked = format!("with {} and keep", "*".repeat(secret.len()));
        let answer = review(
            "in-progress",
            30,
            "",
            &[
                ("English errors", "Keep every error message in English."),
                ("Errors in English", "Keep  every error message\nin English"),
                ("Logs", masked.as_str()),
                ("One fixture a case", "one fixture a case"),
                ("A builder", "Use the fixture builder"),
                ("Tabs", "Use tabs."),
            ],
        );
        let kept = (ask.keep)(&answer);
        let quotes: Vec<&str> = (kept["preferences"].as_array().unwrap().iter())
            .map(|p| p["quote"].as_str().unwrap())
            .collect();
        assert_eq!(
            quotes,
            [
                "Keep every error message in English.",
                "Keep  every error message\nin English",
                masked.as_str()
            ]
        );
    }

    /// P3: with the same conversations, a review shows the previous one, the original ask, and
    /// only what came after it; with other conversations, all again. It lists what it was shown
    /// (S2): the previous review among the ops, the asks among the records.
    #[test]
    fn a_review_amends_with_what_came_after_the_last() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let old = h.said("s1", 5, "Handle comments too.");
        let r = h.reply("s1", 6);
        h.summarized("s1", (old, r), 6, "OLD SUMMARY");
        let card = json!([{"type": "feature", "title": "OLD CARD", "narrative": "Done.",
            "files_modified": ["/home/u/old.pdf"]}]);
        h.s.cards(r, r, card, false);
        h.wrote(
            "s1",
            "parser",
            json!({"task": "OLD TASK", "status": "done"}),
        );
        let input = json!({"command": "git commit -m 'OLD COMMIT'"}).to_string();
        let body = json!({"tool": "Bash", "input": input,
            "output": "[main abc1234] OLD COMMIT\n 1 file changed", "failed": false});
        h.s.event("tool", "s1", (R, "main"), h.t + 7 * MINUTE, body);
        let b = h.said("s2", 10, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let id = h.due()[0].0.clone();
        let comments = [("Comments handled", "Handle comments too.")];
        h.pass(&|_| review("in-progress", 40, "", &comments));
        let first = h.reviews()[0].op_seq;
        assert!(h.due().is_empty(), "{:?}", h.due());
        h.age(61);
        // A card written after the review, of a window before it: new by its op (Codex on #693).
        let late = json!([{"type": "feature", "title": "LATE CARD", "narrative": "Done.",
            "files_modified": ["/home/u/late.pdf"]}]);
        h.s.cards(b, b, late, false);
        let c = h.said("s2", 100, "Now report the column of each error.");
        let ask = h.ask(&id).unwrap();
        let p = &ask.prompt;
        assert!(
            p.contains("An update") && p.contains("The parser reads lines."),
            "{p}"
        );
        assert!(p.contains("Now report the column"), "{p}");
        assert!(
            !p.contains("Build the parser.") && !p.contains("Add tests"),
            "{p}"
        );
        // The previous review's preference keeps its quote, though its ask is not shown again.
        assert!(p.contains("(quote: \"Handle comments too.\")"), "{p}");
        let carried = review(
            "in-progress",
            50,
            "",
            &[comments[0], ("Tests", "Add tests for")],
        );
        let kept = (ask.keep)(&carried);
        assert_eq!(kept["preferences"].as_array().unwrap().len(), 1, "{kept:?}");
        for seen in [
            "OLD SUMMARY",
            "OLD CARD",
            "OLD TASK",
            "old.pdf",
            "OLD COMMIT",
        ] {
            assert!(!p.contains(seen), "{seen}: {p}");
        }
        assert!(p.contains("LATE CARD") && p.contains("late.pdf"), "{p}");
        assert!(ask.read.ops.contains(&first), "{:?}", ask.read);
        assert!(ask.read.spans.contains(&(c, c)), "{:?}", ask.read);
        for seq in [a, old] {
            assert!(!ask.read.spans.contains(&(seq, seq)), "{:?}", ask.read);
        }
        h.said("s3", 101, "Write the parser's docs.");
        h.grouped(&["s1", "s3"], &[]);
        let ask = h.ask(&id).unwrap();
        let p = &ask.prompt;
        assert!(p.contains("conversations changed"), "{p}");
        assert!(
            p.contains("Build the parser.") && p.contains("Add tests"),
            "{p}"
        );
    }

    /// S6: while a request's repository is excluded, it is not reviewed, and its review is left out
    /// of what any job reads; taken back, the review is shown again.
    #[test]
    fn a_request_of_an_excluded_repository_is_not_reviewed() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let b = h.said("s2", 10, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let id = h.due()[0].0.clone();
        h.pass(&|_| review("in-progress", 40, "", &[]));
        h.age(61);
        h.said("s1", 100, "And the column.");
        assert_eq!(h.due().len(), 1);
        h.s.exclude(R);
        assert!(h.due().is_empty());
        assert!(h.ask(&id).is_none());
        let left_out = h.with(|s| {
            let hidden = crate::my_work_ops::hidden(s.raw, s.k, s.reading).unwrap();
            crate::my_work_ops::newest_shown(s.k, (NAME, &id), None, &hidden, false).unwrap()
        });
        assert!(left_out.is_none());
        let undo = json!({"repo": R, "undo": true});
        h.s.raw.append_ops(&[(OpKind::Exclusion, undo)]).unwrap();
        let ask = h.ask(&id).unwrap();
        assert!(ask.prompt.contains("An update"), "{}", ask.prompt);
    }

    /// A5: a round's two passes are one runner's (`turns::round`), and the job keeps its read for
    /// the round. A repository excluded between them, while the windows' calls are out, is in
    /// what the job read before, and the second pass's gate holds the list as it is by then: the
    /// job reads again, and nothing of the repository is sent.
    #[test]
    fn an_exclusion_between_a_rounds_two_passes_sends_nothing_of_the_repository() {
        let mut h = Home::new();
        // Due for less than an hour: the pass before the windows leaves it for the one after.
        let a = h.said("s1", 100, "Build the parser.");
        let b = h.said("s2", 105, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        h.s.run();
        let k = crate::knowledge::open(h.s.home.path()).unwrap();
        let db = crate::providers_db::open(h.s.home.path()).unwrap();
        let (rules, summary) = (Rules::load(h.s.home.path()).unwrap(), Summary::default());
        let sent = RefCell::new(Vec::new());
        let call = |_: &str,
                    prompt: &str,
                    _: &Value,
                    _: &AnswerCheck,
                    gate: &Gate|
         -> Result<ChainResult> {
            gate()?;
            sent.borrow_mut().push(prompt.to_owned());
            Ok(ChainResult {
                provider: "fake".into(),
                output: review("in-progress", 40, "", &[]),
                tier: 1,
            })
        };
        let mut runner = Runner::new(
            (true, 100),
            vec![Box::new(Progress::default()) as Box<dyn Job>],
            Box::new(call),
            (&rules, &summary, "chain"),
        );
        runner.pass(&mut h.s.raw, &k, &db, true).unwrap();
        assert_eq!(*sent.borrow(), Vec::<String>::new());
        h.s.exclude(R);
        runner.pass(&mut h.s.raw, &k, &db, false).unwrap();
        assert_eq!(*sent.borrow(), Vec::<String>::new());
    }

    /// S6: a review that read a summary of a session since excluded, or a card of a window with one
    /// of its records, is left out of what any job reads: the request is due for what came after
    /// the review before it, which its next call reads as the previous one.
    #[test]
    fn a_review_an_exclusion_hides_is_passed_over() {
        const OTHER: &str = "github.com/o/other";
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let b = h.said("s2", 10, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let id = h.due()[0].0.clone();
        h.pass(&|_| review("in-progress", 40, "", &[]));
        h.age(61);
        h.said("s1", 100, "And the column.");
        let x = h.s.said("s9", OTHER, h.t + 50 * MINUTE, "Tune the build.");
        let summary = (h.s).turn(json!({"agent": "claude", "session": "s9", "repo": OTHER,
            "ts": h.t + 51 * MINUTE, "from": x, "through": x, "read": [], "goals": [],
            "removed": [], "fields": {"completed": "Tuned."}, "skipped": false}));
        let summary: i64 = summary[1..].parse().unwrap();
        let card = json!([{"type": "feature", "title": "Build", "narrative": "Tuned."}]);
        let window = h.s.cards(x, x, card, false)[0]
            .split('.')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        // Reviews that read everything, as one whose conversations changed since may have.
        let all = Marks {
            conversations: digest(&[key("s1"), key("s2")]),
            read_at: i64::MAX / 2,
            asks: i64::MAX / 2,
            summaries: i64::MAX / 2,
            cards: i64::MAX / 2,
        };
        let mut ops = Vec::new();
        for (read, summary_text) in [(window, "FROM A CARD"), (summary, "FROM A SUMMARY")] {
            let mut fields: BTreeMap<String, Value> = all.fields().collect();
            fields.insert("summary".into(), json!(summary_text));
            let op = MyWorkOp {
                job: NAME.into(),
                about: id.clone(),
                entry: "fake".into(),
                ts: crate::db::now_ms(),
                read: Read {
                    spans: vec![],
                    ops: vec![read],
                },
                removed: vec![],
                fields,
                skipped: false,
                forgotten: None,
            };
            ops.push(
                h.s.raw
                    .append_ops(&[(OpKind::MyWork, serde_json::to_value(op).unwrap())]),
            );
        }
        assert!(h.due().is_empty(), "{:?}", h.due());
        h.s.exclude(OTHER);
        let left_out = h.with(|s| crate::my_work_ops::hidden(s.raw, s.k, s.reading).unwrap());
        let device = h.s.raw.device().to_owned();
        for op in ops {
            assert!(
                left_out.contains(&(device.clone(), op.unwrap()[0])),
                "{left_out:?}"
            );
        }
        assert_eq!(h.due().len(), 1, "{:?}", h.due());
        let p = h.ask(&id).unwrap().prompt;
        assert!(
            p.contains("The parser reads lines.") && !p.contains("FROM A"),
            "{p}"
        );
    }

    /// S5: a review lists each piece it shows by where it was read from: a card by its window op, a
    /// product by its call's record or its card's window op, though the card is past those shown.
    #[test]
    fn a_review_lists_where_each_piece_it_shows_was_read_from() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Ship the parser fix.");
        let input = json!({"command": "git commit -m 'Fix the parser'"}).to_string();
        let body = json!({"tool": "Bash", "input": input,
            "output": "[main abc1234] Fix the parser\n 1 file changed", "failed": false});
        let commit = h.s.event("tool", "s1", (R, "main"), h.t + MINUTE, body);
        let mut windows = Vec::new();
        for i in 0..=CARDS {
            let at = h.reply("s1", 2 + i64::try_from(i).unwrap());
            let files = if i == 0 {
                json!(["/home/u/report.pdf"])
            } else {
                json!([])
            };
            let card = json!([{"type": "feature", "title": format!("Card {i}"),
                "narrative": "Did it.", "files_modified": files}]);
            let id = h.s.cards(at, at, card, false);
            windows.push(id[0].split('.').next().unwrap().parse::<i64>().unwrap());
        }
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let ask = h.ask(&id).unwrap();
        let p = &ask.prompt;
        assert!(
            p.contains("commit: abc1234 Fix the parser") && p.contains("report.pdf"),
            "{p}"
        );
        assert!(p.contains("Card 12\n") && !p.contains("Card 0\n"), "{p}");
        assert!(ask.read.spans.contains(&(commit, commit)), "{:?}", ask.read);
        for w in &windows {
            assert!(ask.read.ops.contains(w), "{w}: {:?}", ask.read);
        }
    }

    /// P3: a request reviews only the prompts it owns: a conversation that worked on two request
    /// ids shares its other prompts on the page, never in a review.
    #[test]
    fn a_request_reviews_only_the_prompts_it_owns() {
        let mut h = Home::new();
        h.said("s1", 0, "Ship the release.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "notes", "request_id": "A", "status": "doing"}),
        );
        h.reply("s1", 1);
        h.said("s1", 2, "Now tag it.");
        h.wrote(
            "s1",
            "release",
            json!({"task": "tag", "request_id": "B", "status": "doing"}),
        );
        h.reply("s1", 3);
        let (a, b) = (h.ask("r:A").unwrap().prompt, h.ask("r:B").unwrap().prompt);
        assert!(!a.contains("Now tag it."), "{a}");
        assert!(b.contains("Now tag it."), "{b}");
    }

    /// P3: the later asks are those from the original on, as the page's follow-ups: a member's
    /// earlier prompt, which asked nothing, is none.
    #[test]
    fn a_prompt_before_the_original_is_no_later_ask() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.said("s2", -5, "Keep going.");
        let b = h.said("s2", 10, "Add tests for the parser.");
        h.grouped(&["s1", "s2"], &[a, b]);
        let id = h.due()[0].0.clone();
        let p = h.ask(&id).unwrap().prompt;
        assert!(p.contains("Add tests for the parser."), "{p}");
        assert!(!p.contains("Keep going"), "{p}");
    }

    /// S2, S7: a review shows only this device's records, which its spans can list: in a copy of a
    /// store, another device, the earlier asks are left out.
    #[test]
    fn a_review_shows_only_this_devices_records() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Asked on the other device.");
        h.said("s1", 1, "Asked there again.");
        let commit = |h: &mut Home, (sha, title): (&str, &str), at: i64| {
            let input = json!({"command": format!("git commit -m '{title}'")}).to_string();
            let body = json!({"tool": "Bash", "input": input,
                "output": format!("[main {sha}] {title}\n 1 file changed"), "failed": false});
            h.s.event("tool", "s1", (R, "main"), h.t + at * MINUTE, body)
        };
        commit(&mut h, ("abc1111", "Other commit"), 1);
        let r = h.reply("s1", 2);
        h.summarized("s1", (a, r), 2, "Summarized on the other device.");
        let card = json!([{"type": "feature", "title": "Other card", "narrative": "Done.",
            "files_modified": ["/home/u/other.pdf"]}]);
        h.s.cards(r, r, card, false);
        h.grouped(&["s1"], &[a]);
        let Home { s, t } = h;
        let Store { home, raw } = s;
        let other = raw.device().to_owned();
        drop(raw);
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let raw = crate::raw::open(copy.path()).unwrap();
        assert_ne!(raw.device(), other);
        let mut h = Home {
            s: Store { home: copy, raw },
            t,
        };
        let b = h.said("s1", 5, "Asked on this one.");
        let c = commit(&mut h, ("def2222", "This commit"), 6);
        let id = h.due()[0].0.clone();
        let ask = h.ask(&id).unwrap();
        for here in ["Asked on this one.", "This commit"] {
            assert!(ask.prompt.contains(here), "{here}: {}", ask.prompt);
        }
        for other in [
            "the other device",
            "Asked there",
            "Other commit",
            "Other card",
            "other.pdf",
        ] {
            assert!(!ask.prompt.contains(other), "{other}: {}", ask.prompt);
        }
        assert_eq!(
            (&ask.read.spans[..], &ask.read.ops[..]),
            (&[(b, b), (c, c)][..], &[][..])
        );
    }

    /// A4: a part takes each line that fits the room left, in order, a long one passed over for a
    /// shorter one after it; a part none of whose lines fits leaves no heading.
    #[test]
    fn a_piece_that_does_not_fit_is_passed_over_for_a_smaller_one() {
        let mut shown = Shown {
            data: String::new(),
            left: 12,
            spans: BTreeSet::new(),
            ops: BTreeSet::new(),
            typed: Vec::new(),
        };
        let long = format!("{}\n", "x ".repeat(40));
        assert_eq!(
            shown.part("## A\n", &[long.clone(), "- short\n".into()]),
            [1]
        );
        assert_eq!((shown.data.as_str(), shown.left), ("## A\n- short\n", 7));
        assert!(shown.part("## B\n", &[long]).is_empty());
        assert_eq!((shown.data.as_str(), shown.left), ("## A\n- short\n", 7));
    }

    /// A5: a text over its cap is dropped, never cut, with the milestone or the preference it is
    /// part of; one at its cap is kept, and so are the marks.
    #[test]
    fn a_text_over_its_cap_is_dropped_never_cut() {
        let home = tempfile::tempdir().unwrap();
        let rules = Rules::load(home.path()).unwrap();
        let mut answer = review(
            "in-progress",
            50,
            "",
            &[("s".repeat(STATEMENT + 1).as_str(), "q")],
        );
        answer["summary"] = json!("ab".repeat(SUMMARY / 2));
        answer["narrative"] = json!("n".repeat(NARRATIVE + 1));
        answer["milestones"] = json!([
            {"date": "2026-10-02", "text": "m".repeat(MILESTONE + 1)},
            {"date": "2026-10-03", "text": "Kept."}
        ]);
        let marks = Marks {
            read_at: 9,
            ..Marks::default()
        };
        let kept = keep(&answer, &rules, &["q".to_owned()], &marks);
        assert_eq!(kept["summary"], json!("ab".repeat(SUMMARY / 2)));
        assert_eq!(kept["narrative"], Value::Null);
        assert_eq!(
            kept["milestones"],
            json!([{"date": "2026-10-03", "text": "Kept."}])
        );
        assert_eq!(kept["preferences"], json!([]));
        assert_eq!(
            (&kept["started"], &kept["shipped"]),
            (&json!("2026-10-01"), &Value::Null)
        );
        assert_eq!(kept["read_at"], 9);
    }

    /// S5: a forget of a record a review was shown takes the review, and the request is due again
    /// at once, its next review reading what is left with no previous one.
    #[test]
    fn a_forgotten_review_makes_its_request_due_again() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let b = h.said(
            "s1",
            10,
            "Read the fixture at /home/u/private/notes.txt too.",
        );
        h.grouped(&["s1", "s2"], &[a]);
        let id = h.due()[0].0.clone();
        h.pass(&|_| review("in-progress", 40, "", &[]));
        let Home { s, t } = h;
        let Store { home, raw } = s;
        let device = raw.device().to_owned();
        drop(raw);
        let target = crate::forget::Target::Record { device, seq: b };
        crate::forget::start(
            home.path(),
            &crate::forget::preview(home.path(), target).unwrap(),
        )
        .unwrap();
        crate::forget::purge(home.path()).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let h = Home {
            s: Store { home, raw },
            t,
        };
        let reviews = h.reviews();
        assert!(reviews[0].op.forgotten.is_some(), "{:?}", reviews[0].op);
        assert_eq!(h.due(), [(id.clone(), reviews[0].op.ts)]);
        let ask = h.ask(&id).unwrap();
        assert!(ask.prompt.contains("The first review"), "{}", ask.prompt);
        assert!(!ask.prompt.contains("private/notes"), "{}", ask.prompt);
    }

    /// Slice 4a's measurement (docs/my-work-model.md As built): on a copy of a store, never the
    /// live one, how long the review job's due check and one call's prompt take to read, and their
    /// sizes; no model call. By hand: `OBOETE_MEASURE_HOME=<copy> cargo test --release --bin
    /// oboete -- --ignored --nocapture measure_review`.
    #[test]
    #[ignore]
    fn measure_review_on_a_copy_of_a_store() {
        let home = std::env::var("OBOETE_MEASURE_HOME").expect("a copy of a store");
        let home = std::path::Path::new(&home);
        let mut raw = crate::raw::open(home).unwrap();
        let k = crate::knowledge::open(home).unwrap();
        let rules = Rules::load(home).unwrap();
        let summary = Summary::default();
        let now = crate::db::now_ms();
        let reading = Reading::now(&raw, Reads::Live).unwrap();
        let none = crate::my_work::review_inputs(&raw, &k, &rules, &reading, now - ACTIVE_MS);
        if none.unwrap().is_empty() {
            // A copy whose owner never ran grouping: one request a repository of its newest three
            // candidates, written to the copy only, so a call can be measured.
            let seen = crate::my_work::seen(&raw, &k, &rules).unwrap();
            let mut by_repo: BTreeMap<&str, Vec<&crate::my_work::Seen>> = BTreeMap::new();
            for c in &seen {
                if let (Some(repo), Some(ask)) = (&c.repo, &c.ask)
                    && ask.0 == raw.device()
                    && now - c.last <= ACTIVE_MS
                {
                    by_repo.entry(repo).or_default().push(c);
                }
            }
            let mut ops = Vec::new();
            for (repo, mut list) in by_repo.into_iter().take(5) {
                list.sort_by_key(|c| std::cmp::Reverse(c.last));
                list.truncate(3);
                let keys: Vec<Value> = list.iter().map(|c| json!([c.key.0, c.key.1])).collect();
                let group = json!({"conversations": keys, "title": "measure", "links": []});
                let op = MyWorkOp {
                    job: "group".into(),
                    about: repo.into(),
                    entry: "measure".into(),
                    ts: now,
                    read: Read {
                        spans: (list.iter())
                            .filter_map(|c| c.ask.as_ref().map(|a| (a.1, a.1)))
                            .collect(),
                        ops: vec![],
                    },
                    removed: vec![],
                    fields: BTreeMap::from([
                        ("groups".to_owned(), json!([group])),
                        ("ignored".to_owned(), json!([])),
                    ]),
                    skipped: false,
                    forgotten: None,
                };
                ops.push((OpKind::MyWork, serde_json::to_value(op).unwrap()));
            }
            println!("{} groups written to the copy", ops.len());
            let device = raw.device().to_owned();
            let after = raw.max_op_seq_of(&device).unwrap();
            raw.append_ops(&ops).unwrap();
            // My Work's consumer alone: a whole worker pass on a copy rebuilds what it holds.
            use crate::worker::Consumer;
            let mine = crate::consumer::my_work::MyWork.step(&raw, &k, &device, after);
            mine.unwrap();
        }
        let reading = Reading::now(&raw, Reads::Live).unwrap();
        let stores = Stores {
            raw: &raw,
            k: &k,
            rules: &rules,
            summary: &summary,
            reading: &reading,
            held: &crate::my_work_run::NOT_HELD,
        };
        let started = std::time::Instant::now();
        let inputs = crate::my_work::review_inputs(&raw, &k, &rules, &reading, now - ACTIVE_MS);
        let inputs = inputs.unwrap();
        let read = started.elapsed();
        let started = std::time::Instant::now();
        let hidden = crate::my_work_ops::hidden(&raw, &k, &reading).unwrap();
        println!(
            "{} requests to review read in {read:?}; {} ops hidden, read in {:?}",
            inputs.len(),
            hidden.len(),
            started.elapsed()
        );
        let job = Progress::default();
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
                "one call: {} characters, {} tokens estimated, {} records and {} ops shown, read \
                 in {:?}",
                ask.prompt.chars().count(),
                crate::budget::estimate(&ask.prompt),
                ask.read.spans.len(),
                ask.read.ops.len(),
                started.elapsed()
            );
        }
        let peak = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        println!(
            "{}",
            peak.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("")
        );
    }

    /// A8: the same records make the same prompt, whose digest a wait is kept by: the data names
    /// the day, not the minute.
    #[test]
    fn the_same_records_make_the_same_prompt() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let day = (h.t / DAY_MS + 1) * DAY_MS + 60 * MINUTE;
        let at = |now: i64| {
            h.with(|s| {
                Progress::default()
                    .ask(s, &id, now)
                    .unwrap()
                    .unwrap()
                    .prompt
            })
        };
        assert_eq!(at(day), at(day + 5 * MINUTE));
    }

    /// P5: a request's readings are this device's newest two reviews, the newest's percent and
    /// the pace of the two; one a forget took is none.
    #[test]
    fn the_newest_two_reviews_are_the_readings() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        for (ago, percent) in [(180, 10), (120, 40), (60, 60)] {
            h.reviewed(&id, ago, review("in-progress", percent, "", &[]));
        }
        h.reviewed_with(&id, 30, review("in-progress", 99, "", &[]), &|op| {
            op.forgotten = Some("forget".into())
        });
        let p = &h.page()["progress"];
        assert_eq!(p["percent"], 60, "{p}");
        // 20 points an hour, 40 left: two hours on.
        let (at, reviewed) = (p["estimate"].as_i64(), p["reviewed"].as_i64());
        assert!(
            (at.unwrap() - reviewed.unwrap() - 120 * MINUTE).abs() < MINUTE,
            "{p}"
        );
    }

    /// S6, S7: a review an exclusion hides is no reading, nor, in a copy of a store, one another
    /// device wrote.
    #[test]
    fn a_hidden_or_another_devices_review_is_no_reading() {
        const OTHER: &str = "github.com/o/other";
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let x = h.s.said("s9", OTHER, h.t + MINUTE, "Tune the build.");
        h.reviewed_with(&id, 20, review("in-progress", 40, "", &[]), &|op| {
            op.read.spans = vec![(x, x)]
        });
        assert_eq!(h.page()["progress"]["percent"], 40);
        h.s.exclude(OTHER);
        assert_eq!(h.page()["progress"], Value::Null);
        h.reviewed(&id, 10, review("in-progress", 60, "", &[]));
        let Home { s, t } = h;
        let Store { home, raw } = s;
        drop(raw);
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let raw = crate::raw::open(copy.path()).unwrap();
        let h = Home {
            s: Store { home: copy, raw },
            t,
        };
        assert_eq!(h.page()["progress"], Value::Null);
    }

    /// Spec 6.2 step 1, S5: from a forget's start to its purge, a review shown a record it took is no
    /// reading, and no call is shown it as the previous review; one written after the forget, which
    /// lists the removal, is a reading.
    #[test]
    fn a_review_a_forget_took_is_no_reading_before_its_purge() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let x = h.said("s1", 10, "Keep the canary-4827 note.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let mut told = review("in-progress", 40, "", &[]);
        told["narrative"] = json!("It keeps the canary-4827 note.");
        h.reviewed_with(&id, 20, told, &|op| op.read.spans = vec![(x, x)]);
        assert_eq!(h.page()["progress"]["percent"], 40);
        let (path, device) = (h.s.home.path().to_owned(), h.s.raw.device().to_owned());
        let target = crate::forget::Target::Record {
            device: device.clone(),
            seq: x,
        };
        crate::forget::start(&path, &crate::forget::preview(&path, target).unwrap()).unwrap();
        let r = h.page();
        assert_eq!(
            (&r["progress"], &r["story"]),
            (&Value::Null, &Value::Null),
            "{r}"
        );
        let ask = h.ask(&id).unwrap();
        assert!(!ask.prompt.contains("canary-4827"), "{}", ask.prompt);
        let listed = h.s.raw.removed_in(&device, x, x, None).unwrap();
        h.reviewed_with(&id, 0, review("in-progress", 60, "", &[]), &|op| {
            op.read.spans = vec![(x, x)];
            op.removed = listed.clone();
        });
        assert_eq!(h.page()["progress"]["percent"], 60);
    }

    /// S5: before the purge, a review built on a summary or a card its reader leaves out for a
    /// forgotten record (T7, K4) is no reading either, though the review's own records are not it.
    #[test]
    fn a_review_built_on_a_summary_or_a_card_a_forget_took_is_no_reading() {
        for card in [false, true] {
            let mut h = Home::new();
            let a = h.said("s1", 0, "Build the parser.");
            let r = h.reply("s1", 1);
            let built: i64 = if card {
                let c = json!([{"type": "feature", "title": "Parser", "narrative": "Built."}]);
                let id = h.s.cards(a, r, c, false)[0].clone();
                id.split('.').next().unwrap().parse().unwrap()
            } else {
                h.summarized("s1", (a, r), 1, "Built.")
            };
            h.grouped(&["s1"], &[a]);
            let id = h.due()[0].0.clone();
            h.reviewed_with(&id, 20, review("in-progress", 40, "", &[]), &|op| {
                op.read.spans = vec![(a, a)];
                op.read.ops = vec![built];
            });
            assert_eq!(h.page()["progress"]["percent"], 40, "card {card}");
            let path = h.s.home.path().to_owned();
            let target = crate::forget::Target::Record {
                device: h.s.raw.device().to_owned(),
                seq: r,
            };
            crate::forget::start(&path, &crate::forget::preview(&path, target).unwrap()).unwrap();
            assert_eq!(h.page()["progress"], Value::Null, "card {card}");
        }
    }

    /// S5: past a forget's rebuild, what it took is marked, and the walk is off again: an op shown
    /// its record that is not marked, as none is once the rebuild has run, is no longer left out
    /// (Codex on #694: a finished forget stays in `forget_jobs`).
    #[test]
    fn a_forget_past_its_rebuild_leaves_the_walk_off() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let x = h.said("s1", 10, "Keep the note.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let path = h.s.home.path().to_owned();
        let target = crate::forget::Target::Record {
            device: h.s.raw.device().to_owned(),
            seq: x,
        };
        crate::forget::start(&path, &crate::forget::preview(&path, target).unwrap()).unwrap();
        // The purge has raw.db to itself, as `oboete forget` has it.
        let Home { s, t } = h;
        let Store { home, raw } = s;
        drop(raw);
        assert!(crate::forget::purge(home.path()).unwrap().is_some());
        let raw = crate::raw::open(home.path()).unwrap();
        assert!(!raw.forget_unrebuilt().unwrap());
        let mut h = Home {
            s: Store { home, raw },
            t,
        };
        h.reviewed_with(&id, 0, review("in-progress", 40, "", &[]), &|op| {
            op.read.spans = vec![(x, x)]
        });
        assert_eq!(h.page()["progress"]["percent"], 40);
    }

    /// S5: before the purge, a grouping op shown a record a forget took decides nothing, so its
    /// title is not shown.
    #[test]
    fn a_group_a_forget_took_decides_nothing_before_its_purge() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        let b = h.said("s2", 10, "Add the canary-4827 tests.");
        h.grouped(&["s1", "s2"], &[a, b]);
        assert_eq!(h.page()["title"], "The parser");
        let path = h.s.home.path().to_owned();
        let target = crate::forget::Target::Record {
            device: h.s.raw.device().to_owned(),
            seq: b,
        };
        crate::forget::start(&path, &crate::forget::preview(&path, target).unwrap()).unwrap();
        let r = h.page();
        assert_ne!(r["title"], "The parser", "{r}");
    }

    /// P5, claude-mem's `measuredEstimate`: the line through the last two readings when they rise
    /// below 100; none with one reading, a flat or falling pace, or at 100.
    #[test]
    fn the_estimate_needs_two_rising_readings_below_100() {
        let hour = 60 * MINUTE;
        assert_eq!(estimate(&[reading(hour, 50)]), None);
        let rising = [reading(2 * hour, 60), reading(hour, 50)];
        assert_eq!(estimate(&rising), Some(6 * hour));
        for flat in [
            [reading(2 * hour, 50), reading(hour, 50)],
            [reading(2 * hour, 40), reading(hour, 50)],
            [reading(2 * hour, 100), reading(hour, 50)],
            [reading(hour, 60), reading(hour, 50)],
        ] {
            assert_eq!(estimate(&flat), None);
        }
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        h.reviewed(&id, 120, review("in-progress", 40, "", &[]));
        h.reviewed(&id, 60, review("in-progress", 70, "", &[]));
        let p = &h.page()["progress"];
        let (at, reviewed) = (
            p["estimate"].as_i64().unwrap(),
            p["reviewed"].as_i64().unwrap(),
        );
        // 30 points an hour, 30 left: an hour on, but for the moments between the two writes.
        assert!((at - reviewed - 60 * MINUTE).abs() < MINUTE, "{p}");
    }

    /// P5: a review no status was recorded after makes shipped `shipped`, with its date, and 100%
    /// ready; a status recorded after it wins, and then its percent shows only below 100.
    #[test]
    fn a_status_recorded_after_the_review_wins() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.wrote("s1", "parser", json!({"task": "parse", "status": "doing"}));
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        assert_eq!(h.page()["status"], "working");
        h.reviewed(&id, 30, review("in-progress", 100, "", &[]));
        assert_eq!(h.page()["status"], "ready");
        h.reviewed(&id, 20, review("shipped", 100, "2026-10-20", &[]));
        let r = h.page();
        assert_eq!(
            (&r["status"], &r["progress"]["completed"]),
            (&json!("shipped"), &json!("2026-10-20"))
        );
        h.wrote("s1", "parser", json!({"request_status": "doing"}));
        let r = h.page();
        assert_eq!(
            (&r["status"], &r["progress"], &r["story"]),
            (&json!("working"), &Value::Null, &Value::Null),
            "{r}"
        );
        h.reviewed(&id, 10, review("in-progress", 80, "", &[]));
        let r = h.page();
        assert_eq!(
            (&r["status"], &r["progress"]["percent"]),
            (&json!("working"), &json!(80)),
            "{r}"
        );
    }

    /// P5: a review at 100% makes ready a request to do, of no known state or quiet, as one in
    /// progress (claude-mem's `normalizeRequestStatus`).
    #[test]
    fn a_review_at_100_makes_a_quiet_todo_or_unknown_request_ready() {
        for (status, planned, at) in [
            ("todo", true, 0),
            ("unknown", false, 0),
            ("quiet", false, -600),
        ] {
            let mut h = Home::new();
            let a = h.said("s1", at, "Build the parser.");
            if planned {
                h.wrote(
                    "s1",
                    "parser",
                    json!({"task": "parse", "status": "planned"}),
                );
            }
            h.grouped(&["s1"], &[a]);
            let id = h.due()[0].0.clone();
            assert_eq!(h.page()["status"], status);
            h.reviewed(&id, 0, review("in-progress", 100, "", &[]));
            assert_eq!(h.page()["status"], "ready", "{status}");
        }
    }

    /// P5: a ship recorded after the reviews shows 100 and the day it was recorded, with no
    /// estimate, whatever review comes after it.
    #[test]
    fn a_recorded_ship_shows_100_and_its_day() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        h.reviewed(&id, 60, review("in-progress", 40, "", &[]));
        h.reviewed(&id, 30, review("in-progress", 60, "", &[]));
        h.wrote(
            "s1",
            "parser",
            json!({"task": "parse", "request_status": "shipped"}),
        );
        let today = crate::db::utc(crate::db::now_ms())[..10].to_owned();
        let r = h.page();
        let p = &r["progress"];
        assert_eq!(r["status"], "shipped");
        assert_eq!(
            (&p["percent"], &p["completed"], &p["estimate"]),
            (&json!(100), &json!(today), &Value::Null),
            "{p}"
        );
        // A later review counts, and the recorded ship's day stays, a shipped one's date too
        // (claude-mem's `completedAtEpoch` first).
        h.reviewed(&id, 0, review("in-progress", 80, "", &[]));
        let r = h.page();
        assert_eq!(
            (&r["status"], &r["progress"]["completed"]),
            (&json!("shipped"), &json!(today)),
            "{r}"
        );
        h.reviewed(&id, 0, review("shipped", 100, "2026-10-20", &[]));
        assert_eq!(h.page()["progress"]["completed"], json!(today));
    }

    /// The build story is gated as the page reads it: a rule added after the review masks its
    /// narrative and milestones before any rescan.
    #[test]
    fn the_build_story_is_gated_as_it_is_read() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let mut told = review("in-progress", 50, "", &[]);
        told["narrative"] = json!("It signs in with code=123456 first.");
        told["milestones"] = json!([{"date": "2026-10-02", "text": "code=654321 works."}]);
        h.reviewed(&id, 10, told);
        assert!(h.page()["story"].to_string().contains("123456"));
        let rule = "[redaction]\nextra_rules = [{ id = \"r\", regex = 'code=([0-9]{6})', secret_group = 1 }]\n";
        std::fs::write(h.s.home.path().join("config.toml"), rule).unwrap();
        let now = crate::db::now_ms();
        let v = crate::my_work::read(h.s.home.path(), crate::my_work::Range::All, now).unwrap();
        let story = v["requests"][0]["story"].to_string();
        assert!(
            story.contains("signs in") && !story.contains("123456") && !story.contains("654321"),
            "{story}"
        );
    }

    /// C7, P5: a review is no part of the version a check is of, so a new one undoes no check.
    #[test]
    fn a_new_review_does_not_undo_a_check() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let home = h.s.home.path().to_owned();
        h.s.run();
        let now = crate::db::now_ms();
        let (v, held) = crate::my_work::reviewing(&home, now).unwrap();
        let version = v["requests"][0]["version"].clone();
        let lock = crate::my_work::reviews_lock(&home).unwrap();
        let filed = (&held[0], &held[..]);
        crate::my_work::set_review((&home, &lock), filed, version.as_str().unwrap(), true, now)
            .unwrap();
        drop(lock);
        h.reviewed(&id, 0, review("in-progress", 100, "", &[]));
        let r = h.page();
        assert_eq!(r["version"], version);
        assert!(r["checked"].is_i64(), "{r}");
    }

    /// P5: the build story is the narrative and the milestones, the oldest first, of the review the
    /// progress shows; a review with neither has none.
    #[test]
    fn the_build_story_is_the_reviews_narrative_and_milestones() {
        let mut h = Home::new();
        let a = h.said("s1", 0, "Build the parser.");
        h.grouped(&["s1"], &[a]);
        let id = h.due()[0].0.clone();
        let mut told = review("in-progress", 50, "", &[]);
        told["milestones"] = json!([
            {"date": "2026-10-05", "text": "Tests pass."},
            {"date": "2026-10-02", "text": "The parser reads a line."}
        ]);
        h.reviewed(&id, 10, told);
        let story = &h.page()["story"];
        assert_eq!(story["narrative"], "The parser came first, then its tests.");
        let dates: Vec<&str> = (story["milestones"].as_array().unwrap().iter())
            .map(|m| m["date"].as_str().unwrap())
            .collect();
        assert_eq!(dates, ["2026-10-02", "2026-10-05"]);
        let mut bare = review("in-progress", 60, "", &[]);
        bare["narrative"] = json!("");
        bare["milestones"] = json!([]);
        h.reviewed(&id, 5, bare);
        assert_eq!(h.page()["story"], Value::Null);
    }

    /// A4: the fixed instructions stay under their share of an 8,000-token request.
    #[test]
    fn the_instructions_fit_their_share() {
        let fixed = crate::budget::estimate(&prompt("Japanese", ""));
        assert!(fixed < 1_500, "{fixed}");
    }
