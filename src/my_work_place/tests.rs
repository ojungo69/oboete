    use super::*;
    use crate::config::Summary;
    use crate::curate::{Phase, Reading, Reads};
    use crate::my_work_ops::MyWorkOp;
    use crate::my_work_run::Runner;
    use crate::provider::tests::{request_body, serve_each};
    use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
    use crate::raw::OpKind;
    use crate::search::b::fixture::Store;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    const R: &str = "/r/parser";
    const MINUTE: i64 = 60_000;
    const AREA: &str = "fa_00000000000a";

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

        fn said(&mut self, session: &str, at: i64, text: &str) -> i64 {
            self.s.said(session, R, self.t + at * MINUTE, text)
        }

        /// `n` requests, each a group of two conversations titled `Request <i>` and its review:
        /// their placements' subjects, in the order of their titles.
        fn requests(&mut self, n: usize) -> Vec<String> {
            let mut groups = Vec::new();
            for i in 1..=n {
                let (a, b) = (format!("s{i}a"), format!("s{i}b"));
                let at = i64::try_from(i).unwrap();
                self.said(&a, at, &format!("Build part {i} of the parser."));
                self.said(&b, at + 30, &format!("Test part {i} of the parser."));
                groups.push(json!({"conversations": [["claude", a], ["claude", b]],
                    "title": format!("Request {i}"), "links": []}));
            }
            self.op("group", R, json!({"groups": groups, "ignored": []}));
            let ids = self.ids();
            for id in &ids {
                let fields = json!({"state": "in-progress", "summary": "The parser reads lines."});
                self.op(crate::my_work_progress::NAME, id, fields);
            }
            ids.iter()
                .map(|id| crate::my_work_focus::subject(id))
                .collect()
        }

        /// A `job` op about `about` keeping `fields`: its op seq.
        fn op(&mut self, job: &str, about: &str, fields: Value) -> i64 {
            self.op_of(job, about, fields, (crate::db::now_ms(), false))
        }

        /// The op the runner keeps of a job it gave up on (`my_work_run::given_up`), at `at`
        /// minutes from the start.
        fn skip(&mut self, about: &str, at: i64) -> i64 {
            let ts = self.t + at * MINUTE;
            self.op_of(NAME, about, json!({}), (ts, true))
        }

        fn op_of(&mut self, job: &str, about: &str, fields: Value, at: (i64, bool)) -> i64 {
            let (ts, skipped) = at;
            let op = MyWorkOp {
                job: job.into(),
                about: about.into(),
                entry: "fake".into(),
                ts,
                read: Read::default(),
                removed: vec![],
                fields: fields.as_object().unwrap().clone().into_iter().collect(),
                skipped,
                forgotten: None,
            };
            let op = serde_json::to_value(op).unwrap();
            let seqs = self.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
            self.s.run();
            seqs[0]
        }

        /// An area of `status`, decided by `by`: its op seq.
        fn area(&mut self, id: &str, name: &str, status: &str, by: &str) -> i64 {
            let fields = json!({"kind": "area", "name": name, "status": status,
                "source": "owner-goals", "decided_by": by});
            self.op(crate::my_work_focus::NAME, id, fields)
        }

        /// The newest place op about `subject`, a skip too.
        fn last(&self, subject: &str) -> Option<Kept> {
            self.with(|s| {
                let none = HashSet::new();
                crate::my_work_ops::newest_shown(s.k, (NAME, subject), None, &none, false)
            })
            .unwrap()
        }

        fn with<T>(&self, f: impl FnOnce(&Stores) -> T) -> T {
            self.with_held(&crate::my_work_run::NOT_HELD, f)
        }

        fn with_held<T>(
            &self,
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
                summary: &self.summary,
                reading: &reading,
                held,
            })
        }

        /// The requests' ids, in the order of their titles.
        fn ids(&self) -> Vec<String> {
            let mut inputs = self.with(|s| {
                let since = crate::db::now_ms() - ACTIVE_MS;
                crate::my_work::review_inputs(s.raw, s.k, s.rules, s.reading, since).unwrap()
            });
            inputs.sort_by_key(|i| i.title.clone());
            inputs.into_iter().map(|i| i.id).collect()
        }

        /// Every subject due, in any batch, and `other` when the Other pass is.
        fn due(&self) -> Vec<String> {
            self.due_held(&crate::my_work_run::NOT_HELD)
        }

        fn due_held(&self, held: &BTreeSet<(String, String)>) -> Vec<String> {
            let mut due: Vec<String> = self.with_held(held, |s| {
                let round = round(s, crate::db::now_ms()).unwrap();
                let other = round.other.map(|_| OTHER.to_owned());
                (round.batches.into_iter().flat_map(|(_, b)| b))
                    .chain(other)
                    .collect()
            });
            due.sort();
            due
        }

        fn focus(&self) -> Focus {
            self.with(|s| Focus::read(s.raw, s.k, s.reading).unwrap())
        }

        /// One pass of a runner of the placement job alone, the chain answering `answer` and
        /// refusing what the check refuses: the prompts sent.
        fn pass(&mut self, answer: &dyn Fn(&str) -> Value) -> Vec<String> {
            self.pass_with(None, Box::new(Place::default()), answer)
        }

        /// `pass` of `job`, with the judge (its stub at `judge::AT`) when one is given.
        fn pass_with(
            &mut self,
            judge: Option<&crate::config::Judge>,
            job: Box<dyn Job>,
            answer: &dyn Fn(&str) -> Value,
        ) -> Vec<String> {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let db = crate::providers_db::open(self.s.home.path()).unwrap();
            let rules = Rules::load(self.s.home.path()).unwrap();
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
                (&rules, &self.summary, "chain"),
            )
            .judge(judge.map(|j| (j, 5.0)));
            let _: Phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
            drop(runner);
            self.s.run();
            sent.into_inner()
        }

        /// The judge as the runner is given it, its key set and `[judge]` in config.toml; nothing
        /// a test does not serve reaches Typesafe.
        fn judge(&self) -> crate::config::Judge {
            let home = self.s.home.path();
            let key_file = home.join("TYPESAFE_KEY.md");
            std::fs::write(&key_file, "# a test key\nTESTKEY_for_the_stub\n").unwrap();
            let table = format!("[judge]\nkey_file = '{}'\n", key_file.display());
            std::fs::write(home.join("config.toml"), table).unwrap();
            crate::judge::AT.set(Some("http://127.0.0.1:9".into()));
            crate::config::Judge {
                enabled: true,
                destination: crate::config::Destination::Jev,
                key_file: Some(key_file),
                cloudflare_key_file: None,
                cloudflare_account_id: None,
                liquid_key_file: None,
                model: "jev-latest".into(),
                timeout_s: 5,
                daily_budget: 300,
            }
        }

        /// The focus judge's ops (J5), the oldest first.
        fn judged(&self) -> Vec<Kept> {
            let none = HashSet::new();
            let job = (crate::my_work_focus_judge::NAME, None);
            let mut ops = self
                .with(|s| crate::my_work_ops::shown(s.k, job, None, &none, false, usize::MAX))
                .unwrap();
            ops.reverse();
            ops
        }

        /// The home after the record `seq` of this device was forgotten and purged.
        fn forgot(self, seq: i64) -> Self {
            let Home { s, t, summary } = self;
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
                summary,
            };
            h.s.run();
            h
        }
    }

    /// The stub answering `answers` in Jev's place.
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

    /// Jev's answer choosing `choice` at `p`.
    fn chose(choice: &str, p: f64) -> (&'static str, Vec<u8>) {
        let body = json!({"model": "jev-1.14.0",
            "answers": {"area": {"type": "choice", "choice": choice, "confidence": p,
                "probabilities": {choice: p}}},
            "usage": {"input_tokens": 9_000, "output_tokens": 12}});
        ("200 OK", body.to_string().into_bytes())
    }

    /// Each request shown, by its label, with the `<i>` of its title `Request <i>`.
    fn shown(prompt: &str) -> Vec<(String, usize)> {
        (prompt.split("### ").skip(1))
            .filter_map(|block| {
                let label = block.lines().next()?.to_owned();
                let title = block
                    .lines()
                    .find_map(|l| l.strip_prefix("Title: Request "))?;
                Some((label, title.parse().ok()?))
            })
            .collect()
    }

    /// A placing answer putting `Request <i>` in `areas[i - 1]`.
    fn placed(prompt: &str, areas: &[&str]) -> Value {
        let placements: Vec<Value> = (shown(prompt).into_iter())
            .map(|(label, i)| json!({"request": label, "area": areas[i - 1], "evidence": "Fits."}))
            .collect();
        json!({"placements": placements, "proposals": []})
    }

    fn proposal(kind: &str, name: &str, requests: &[&str], same_as: &str) -> Value {
        json!({"kind": kind, "name": name, "goal": "Ship it.", "about": "The parser.",
            "requests": requests, "same_as": same_as, "evidence": "They belong together."})
    }

    /// A5: what the job read is kept for the round, and `Job::reread` drops it. Here
    /// a request made since is in its next read, and not while the first is
    /// kept. The runner tells every job so when the list changed between a round's two passes
    /// (`my_work_run`).
    #[test]
    fn the_job_keeps_its_read_until_it_is_told_to_read_again() {
        let mut h = Home::new();
        let job = Place::default();
        let due = |h: &Home| h.with(|s| job.due(s, crate::db::now_ms()).unwrap().len());
        assert_eq!(due(&h), 0);
        h.requests(2);
        assert_eq!(due(&h), 0, "kept for the round");
        job.reread();
        assert!(due(&h) > 0, "read again");
    }

    /// F3: every request is placed once, by its anchor, in a listed area or Other; placed, it is
    /// not due; a conversation joining it places it again, alone.
    #[test]
    fn every_request_is_placed_once_and_a_moved_one_again() {
        let mut h = Home::new();
        let subjects = h.requests(2);
        let area = h.area(AREA, "The parser", "kept", "owner");
        let mut due = subjects.clone();
        due.sort();
        assert_eq!(h.due(), due);
        let sent = h.pass(&|p| placed(p, &[AREA, OTHER]));
        assert_eq!(sent.len(), 1, "both in one call");
        assert!(
            sent[0].contains(&format!("- {AREA} (kept): The parser")),
            "{}",
            sent[0]
        );
        assert!(sent[0].contains("Its other conversations' opening asks:\n- Test part 1"));
        let focus = h.focus();
        assert_eq!(
            (focus.of(&subjects[0]), focus.of(&subjects[1])),
            (Some(AREA), Some(OTHER))
        );
        assert!(!h.due().contains(&subjects[0]), "{:?}", h.due());
        // S2: it read each first and opening ask, and the area's op.
        let read = h.last(&subjects[0]).unwrap().op.read;
        assert_eq!((read.spans.len(), read.ops.contains(&area)), (4, true));
        // Request 1 gains a conversation.
        h.said("s1c", 90, "Document part 1 of the parser.");
        let groups = json!([
            {"conversations": [["claude", "s1a"], ["claude", "s1b"], ["claude", "s1c"]],
                "title": "Request 1", "links": []},
            {"conversations": [["claude", "s2a"], ["claude", "s2b"]], "title": "Request 2",
                "links": []}]);
        h.op("group", R, json!({"groups": groups, "ignored": []}));
        assert_eq!(h.due(), [subjects[0].clone()]);
    }

    /// F3 (CodeRabbit on #753): a conversation's opening ask is its first ask, not an
    /// acknowledgement, and the original's conversation shows none.
    #[test]
    fn an_opening_ask_is_an_ask_of_another_conversation() {
        let mut h = Home::new();
        h.said("s1a", 0, "ok");
        h.said("s1a", 1, "Build part 1 of the parser.");
        h.said("s1b", 30, "続けて");
        h.said("s1b", 31, "Test part 1 of the parser.");
        let groups = json!([{"conversations": [["claude", "s1a"], ["claude", "s1b"]],
            "title": "Request 1", "links": []}]);
        h.op("group", R, json!({"groups": groups, "ignored": []}));
        let sent = h.pass(&|p| placed(p, &[OTHER]));
        let openings = "Its other conversations' opening asks:\n- Test part 1 of the parser.\n";
        assert!(sent[0].contains(openings), "{}", sent[0]);
        assert!(
            !sent[0].contains("- ok\n") && !sent[0].contains("続けて"),
            "{}",
            sent[0]
        );
    }

    /// F3: the refusals of claude-mem's validation, each its own outcome.
    #[test]
    fn the_answer_is_checked_as_claude_mem_checks_it() {
        let labels = vec!["R1".to_owned(), "R2".to_owned()];
        let areas: HashSet<String> = [AREA.to_owned()].into();
        let dismissed: HashSet<String> = ["fa_00000000000d".to_owned()].into();
        let run = |q, a: Value| check(&a, q, &labels, (&areas, &dismissed));
        let place = |r1: &str, evidence: &str, proposals: Value| {
            json!({"placements": [{"request": "R1", "area": r1, "evidence": evidence},
                {"request": "R2", "area": OTHER, "evidence": "Fits."}], "proposals": proposals})
        };
        let ok = place(AREA, "Fits.", json!([]));
        assert_eq!(run(Question::Place, ok), None);
        let promote = json!([proposal("promote", "Lexer", &["R1"], "")]);
        assert_eq!(run(Question::Place, place(AREA, "Fits.", promote)), None);
        let cases = [
            (place("fa_00000000000b", "Fits.", json!([])), "unanchored"),
            (place("fa_00000000000d", "Fits.", json!([])), "dismissed"),
            (place(AREA, " ", json!([])), "shape"),
            (
                place(AREA, &"x".repeat(EVIDENCE + 1), json!([])),
                "over_cap",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("new", "Lexer", &["R1", "R2"], "")]),
                ),
                "shape",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("promote", "Lexer", &["R1", "R2"], "")]),
                ),
                "shape",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("promote", "My Work", &["R1"], "")]),
                ),
                "page_word",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("promote", &"n".repeat(81), &["R1"], "")]),
                ),
                "over_cap",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("promote", "Lexer", &["R1"], "fa_1")]),
                ),
                "unanchored",
            ),
            (
                place(
                    AREA,
                    "Fits.",
                    json!([proposal("promote", "Lexer", &["R1"], "fa_00000000000d")]),
                ),
                "dismissed",
            ),
            (
                json!({"placements": [{"request": "R1", "area": AREA, "evidence": "Fits."}],
                    "proposals": []}),
                "shape",
            ),
            (
                json!({"placements": [{"request": "R1", "area": AREA, "evidence": "Fits."},
                    {"request": "R1", "area": AREA, "evidence": "Fits."},
                    {"request": "R2", "area": AREA, "evidence": "Fits."}], "proposals": []}),
                "shape",
            ),
        ];
        for (answer, outcome) in cases {
            assert_eq!(
                run(Question::Place, answer.clone()),
                Some(outcome),
                "{answer}"
            );
        }
        let other = |proposals: Value| json!({"proposals": proposals});
        let new = proposal("new", "Lexer", &["R1", "R2"], "");
        assert_eq!(run(Question::Other, other(json!([new]))), None);
        let lone = proposal("new", "Lexer", &["R1"], "");
        assert_eq!(run(Question::Other, other(json!([lone]))), Some("shape"));
        let twice = json!([new, proposal("promote", "Tests", &["R1"], "")]);
        assert_eq!(run(Question::Other, other(twice)), Some("shape"));
        let unknown = proposal("new", "Lexer", &["R1", "R3"], "");
        assert_eq!(run(Question::Other, other(json!([unknown]))), Some("shape"));
        let into_other = proposal("new", "Lexer", &["R1", "R2"], OTHER);
        assert_eq!(
            run(Question::Other, other(json!([into_other]))),
            Some("unanchored")
        );
    }

    /// F3: a request the runner gave up on waits until it moves again; one whose area the owner
    /// dismissed is placed again.
    #[test]
    fn a_given_up_request_waits_and_a_dismissed_area_places_again() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        h.skip(&subjects[0], 60);
        assert!(h.due().is_empty(), "{:?}", h.due());
        h.said("s1a", 100, "And make part 1 faster.");
        assert_eq!(h.due(), subjects);
        h.pass(&|p| placed(p, &[AREA]));
        assert!(h.due().is_empty(), "{:?}", h.due());
        let dismissed = json!({"kind": "area", "status": "dismissed", "decided_by": "owner"});
        h.op(crate::my_work_focus::NAME, AREA, dismissed);
        assert_eq!(h.due(), subjects);
    }

    /// F3: a request promoted while it is placed makes a suggested area, by the system, sourced
    /// `promotion`, under a new id of My Focus's form, and goes in it.
    #[test]
    fn a_promoted_request_gets_a_suggested_area_of_its_own() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        h.pass(&|p| {
            let mut a = placed(p, &[OTHER]);
            a["proposals"] = json!([proposal("promote", "Parser parts", &["R1"], "")]);
            a
        });
        let focus = h.focus();
        let (id, area) = focus.areas.iter().next().expect("an area");
        assert!(crate::my_work_focus::is_id(&json!(id)), "{id}");
        assert_eq!(
            (
                area.name(),
                area.status(),
                area.text("source"),
                area.text("decided_by")
            ),
            (
                "Parser parts",
                "suggested",
                Some("promotion"),
                Some("system")
            )
        );
        assert_eq!(focus.of(&subjects[0]), Some(id.as_str()));
        assert!(h.due().is_empty(), "{:?}", h.due());
    }

    /// F3: a request the owner placed in a live area is never placed by the router; once the area
    /// is dismissed, it is.
    #[test]
    fn the_owners_placement_is_never_placed_again_while_its_area_lives() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let placement = json!({"kind": "placement", "area": AREA, "by": "owner"});
        h.op(crate::my_work_focus::NAME, &subjects[0], placement);
        assert!(h.due().is_empty(), "{:?}", h.due());
        let dismissed = json!({"kind": "area", "status": "dismissed", "decided_by": "owner"});
        h.op(crate::my_work_focus::NAME, AREA, dismissed);
        assert_eq!(h.due(), subjects);
        let sent = h.pass(&|p| placed(p, &[OTHER]));
        assert!(
            sent[0].contains("never to propose again\n- The parser\n"),
            "{}",
            sent[0]
        );
        assert!(
            !sent[0].contains(AREA),
            "a dismissed area is no id to place in"
        );
    }

    /// F4: the requests the router put in Other make the Other pass due; its `new` proposal makes a
    /// suggested area sourced `clustering` holding them; the pass is not due again until the set
    /// left moves.
    #[test]
    fn the_other_pass_proposes_areas_from_other() {
        let mut h = Home::new();
        let subjects = h.requests(3);
        h.pass(&|p| placed(p, &[OTHER, OTHER, OTHER]));
        assert_eq!(h.due(), [OTHER]);
        let sent = h.pass(&|p| {
            let held: Vec<String> = (shown(p).into_iter())
                .filter(|(_, i)| *i <= 2)
                .map(|(label, _)| label)
                .collect();
            let held: Vec<&str> = held.iter().map(String::as_str).collect();
            json!({"proposals": [proposal("new", "Parser work", &held, "")]})
        });
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("Place nothing."), "{}", sent[0]);
        let focus = h.focus();
        let (id, area) = focus.areas.iter().next().expect("an area");
        assert_eq!(
            (area.status(), area.text("source")),
            ("suggested", Some("clustering"))
        );
        assert_eq!(
            [&subjects[0], &subjects[1], &subjects[2]].map(|s| focus.of(s)),
            [Some(id.as_str()), Some(id.as_str()), Some(OTHER)]
        );
        assert!(h.due().is_empty(), "{:?}", h.due());
        // #730: a pass a failure holds waits.
        h.op(NAME, OTHER, json!({"digest": 1}));
        assert_eq!(h.due(), [OTHER]);
        let held = BTreeSet::from([(NAME.to_owned(), OTHER.to_owned())]);
        assert!(h.due_held(&held).is_empty());
        // A proposal the same as a listed area places its requests there, and adds none.
        h.area(AREA, "The parser", "kept", "owner");
        h.pass(&|p| {
            let label = shown(p).into_iter().find(|(_, i)| *i == 3).unwrap().0;
            json!({"proposals": [proposal("promote", "Lexer", &[label.as_str()], AREA)]})
        });
        let focus = h.focus();
        assert_eq!((focus.areas.len(), focus.of(&subjects[2])), (2, Some(AREA)));
    }

    /// S6: an exclusion of the repository hides the placements made on what it showed.
    #[test]
    fn an_exclusion_hides_the_placements_it_read() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        h.pass(&|p| placed(p, &[AREA]));
        assert_eq!(h.focus().of(&subjects[0]), Some(AREA));
        (h.s.raw).exclude_capture(("repo", R), false).unwrap();
        h.s.run();
        assert_eq!(h.focus().router_of(&subjects[0]), None);
    }

    /// The line a request's block shows a pick in (J5).
    const PICK: &str = "The judge's pick, a suggestion only:";

    /// J5: with a kept area, the judge is asked about a queued request before the chain, once, as
    /// claude-mem asks it: its pick is kept as an op of its own, and the placement's call shows it as
    /// a suggestion and lists its op among what it read, so that what takes the pick takes the
    /// placement.
    #[test]
    fn the_judge_picks_once_and_the_placement_is_shown_its_pick() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let rx = stub(vec![chose(AREA, 0.82)]);
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        assert!(
            h.pass_with(Some(&judge), Box::new(Place::default()), &none)
                .is_empty()
        );
        let asked = bodies(&rx);
        assert_eq!(asked.len(), 1);
        let question = &asked[0]["questions"]["area"];
        assert_eq!(question["type"], "choice");
        assert!(
            (question["instructions"].as_str().unwrap())
                .starts_with("`request` is one piece of the owner's work.")
        );
        assert_eq!(
            question["criteria"],
            json!({AREA: format!("`areas.{AREA}`: The parser"), "none": "None of these focus areas"})
        );
        let state = &asked[0]["state"];
        assert_eq!(state["request"]["title"], "Request 1");
        assert_eq!(
            state["request"]["asks"],
            json!(["Build part 1 of the parser."])
        );
        assert_eq!(
            state["areas"],
            json!({AREA: {"name": "The parser", "goal": null, "about": null}})
        );
        let judged = h.judged();
        assert_eq!(judged.len(), 1);
        let pick = &judged[0];
        assert_eq!(
            (
                pick.op.about.as_str(),
                &pick.op.fields["focus"],
                &pick.op.fields["p"]
            ),
            (subjects[0].as_str(), &json!(AREA), &json!(0.82))
        );
        assert_eq!(
            pick.op.fields["evidence"],
            "Jev: \"The parser\" over 1 kept focus area (p 0.82)."
        );
        let sent = h.pass_with(Some(&judge), Box::new(Place::default()), &|p| {
            placed(p, &[AREA])
        });
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].contains(&format!("{PICK} {AREA} (p 0.82)")),
            "{}",
            sent[0]
        );
        assert!(sent[0].contains("It is only a suggestion"), "{}", sent[0]);
        let placement = h.last(&subjects[0]).unwrap();
        assert!(placement.op.read.ops.contains(&pick.op_seq));
        assert!(bodies(&rx).is_empty(), "asked once");
    }

    /// J7: the focus judge's pick names the destination that made it, as its op's entry and in its
    /// evidence.
    #[test]
    fn the_focus_judge_s_pick_names_its_destination() {
        let mut h = Home::new();
        h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let mut judge = h.judge();
        judge.destination = crate::config::Destination::D1Local;
        let config = h.s.home.path().join("config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("{text}destination = \"d1-local\"\n")).unwrap();
        let _rx = stub(vec![chose(AREA, 0.82)]);
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        assert!(
            h.pass_with(Some(&judge), Box::new(Place::default()), &none)
                .is_empty()
        );
        let judged = h.judged();
        assert_eq!(judged[0].op.entry, "judge:d1-local");
        assert_eq!(
            judged[0].op.fields["evidence"],
            "d1 on this PC: \"The parser\" over 1 kept focus area (p 0.82)."
        );
    }

    /// J5: with no kept area (a suggested one only), or with the judge off, nothing is asked and the
    /// placement's call shows no pick.
    #[test]
    fn with_no_kept_area_or_the_judge_off_nothing_is_asked() {
        let mut h = Home::new();
        h.requests(1);
        h.area(AREA, "The parser", "suggested", "system");
        let judge = h.judge();
        let rx = stub(vec![chose(AREA, 0.9)]);
        let sent = h.pass_with(Some(&judge), Box::new(Place::default()), &|p| {
            placed(p, &[AREA])
        });
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].contains(PICK), "{}", sent[0]);
        assert!(bodies(&rx).is_empty() && h.judged().is_empty());
        let mut h = Home::new();
        h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let sent = h.pass_with(None, Box::new(Place::default()), &|p| placed(p, &[AREA]));
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].contains(PICK), "{}", sent[0]);
        assert!(h.judged().is_empty());
    }

    /// J5 (claude-mem's `pickAmong`): more kept areas than one question takes, 19, are asked in
    /// sets, then over the sets' winners; with no winner, none at the lowest probability a set gave.
    #[test]
    fn more_kept_areas_than_a_question_takes_are_asked_in_sets_then_over_the_winners() {
        let ids: Vec<String> = (1..=20).map(|i| format!("fa_{i:012x}")).collect();
        let home = || {
            let mut h = Home::new();
            h.requests(1);
            for (i, id) in ids.iter().enumerate() {
                h.area(id, &format!("Area {:02}", i + 1), "kept", "owner");
            }
            h
        };
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        let offered = |b: &Value| {
            b["questions"]["area"]["criteria"]
                .as_object()
                .unwrap()
                .len()
                - 1
        };
        let mut h = home();
        let judge = h.judge();
        let rx = stub(vec![
            chose(&ids[0], 0.7),
            chose(&ids[19], 0.6),
            chose(&ids[19], 0.9),
        ]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        let asked = bodies(&rx);
        assert_eq!(asked.iter().map(offered).collect::<Vec<_>>(), [19, 1, 2]);
        let final_question = asked[2]["questions"]["area"]["criteria"]
            .as_object()
            .unwrap();
        assert!(final_question.contains_key(&ids[0]) && final_question.contains_key(&ids[19]));
        let pick = &h.judged()[0].op;
        assert_eq!(
            (&pick.fields["focus"], &pick.fields["p"]),
            (&json!(ids[19]), &json!(0.9))
        );
        assert_eq!(
            pick.fields["evidence"],
            "Jev: \"Area 20\" over 20 kept focus areas, asked in 2 sets of up to 19 and 1 final \
             question over the set winners (p 0.90)."
        );
        let mut h = home();
        let judge = h.judge();
        let rx = stub(vec![chose("none", 0.8), chose("none", 0.6)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        assert_eq!(bodies(&rx).len(), 2);
        let pick = &h.judged()[0].op;
        assert_eq!(
            (&pick.fields["focus"], &pick.fields["p"]),
            (&json!(OTHER), &json!(0.6))
        );
    }

    /// J5: a call refused or failed is kept with its outcome, for the request's basis and the kept
    /// areas, so the next pass places the request with no pick and asks Jev nothing more (claude-mem
    /// asks no call again); once the request moves, it is asked again.
    #[test]
    fn a_failed_call_is_kept_and_asked_again_only_once_the_request_moves() {
        let mut h = Home::new();
        h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        let rx = stub(vec![("500 Internal Server Error", b"{}".to_vec())]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        assert_eq!(bodies(&rx).len(), 1);
        let judged = h.judged();
        assert_eq!(judged.len(), 1);
        assert_eq!(
            (
                &judged[0].op.fields["focus"],
                &judged[0].op.fields["outcome"]
            ),
            (&Value::Null, &json!("error"))
        );
        let sent = h.pass_with(Some(&judge), Box::new(Place::default()), &|p| {
            placed(p, &[AREA])
        });
        assert_eq!(sent.len(), 1, "placed with no pick");
        assert!(!sent[0].contains(PICK), "{}", sent[0]);
        assert_eq!(h.judged().len(), 1, "asked nothing more");
        // The request moves: a third conversation joins it.
        h.said("s1c", 50, "Document part 1 of the parser.");
        let keys = json!([["claude", "s1a"], ["claude", "s1b"], ["claude", "s1c"]]);
        let groups = json!([{"conversations": keys, "title": "Request 1", "links": []}]);
        h.op("group", R, json!({"groups": groups, "ignored": []}));
        let rx = stub(vec![chose(AREA, 0.8)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        assert_eq!(bodies(&rx).len(), 1);
        assert_eq!(h.judged().len(), 2);
    }

    /// J5: a repository of the request excluded after the round read it, before the judge is asked:
    /// nothing goes, and nothing is kept of the judge.
    #[test]
    fn an_exclusion_found_on_the_way_sends_nothing_and_keeps_nothing() {
        const R2: &str = "/r/lexer";
        struct Racing(Place, std::path::PathBuf);
        impl Job for Racing {
            fn name(&self) -> &'static str {
                self.0.name()
            }
            fn reread(&self) {
                self.0.reread()
            }
            fn due(&self, s: &Stores, now: i64) -> Result<Vec<Due>> {
                self.0.due(s, now)
            }
            fn ask(&self, s: &Stores, about: &str, now: i64) -> Result<Option<Ask>> {
                self.0.ask(s, about, now)
            }
            /// The round's read first, then the exclusion, then the judge.
            fn judged(&self, s: &Stores, judge: &mut Judging, now: i64) -> Result<Vec<MyWorkOp>> {
                self.0.due(s, now)?;
                crate::raw::open(&self.1)
                    .unwrap()
                    .exclude(R2, false)
                    .unwrap();
                self.0.judged(s, judge, now)
            }
        }
        // A request of two conversations in two repositories, the second's excluded on the way:
        // the check covers each of the request's repositories, not its anchor's alone.
        let mut h = Home::new();
        h.said("s1a", 1, "Build part 1 of the parser.");
        h.s.said("s1b", R2, h.t + 31 * MINUTE, "Test part 1 of the lexer.");
        let keys = json!([["claude", "s1a"], ["claude", "s1b"]]);
        let groups = json!([{"conversations": keys, "title": "Request 1", "links": []}]);
        h.op("group", R, json!({"groups": groups, "ignored": []}));
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let rx = stub(vec![chose(AREA, 0.9)]);
        let job = Racing(Place::default(), h.s.home.path().to_owned());
        let sent = h.pass_with(Some(&judge), Box::new(job), &|p| placed(p, &[AREA]));
        assert!(sent.is_empty(), "nor the chain");
        assert!(bodies(&rx).is_empty());
        assert!(h.judged().is_empty());
    }

    /// J5, S5: a forget of what the judge read, a prompt of the request's anchor, takes its pick and
    /// so the placement that showed it; the request is asked about again.
    #[test]
    fn a_forget_of_what_the_judge_read_takes_its_pick_and_the_placement() {
        let mut h = Home::new();
        let subjects = h.requests(1);
        let read = h.said("s1a", 2, "And its error messages.");
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        let _rx = stub(vec![chose(AREA, 0.82)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        h.pass_with(Some(&judge), Box::new(Place::default()), &|p| {
            placed(p, &[AREA])
        });
        let pick = h.judged().remove(0);
        assert!(
            pick.op.read.spans.contains(&(read, read)),
            "{:?}",
            pick.op.read
        );
        let mut h = h.forgot(read);
        assert!(h.judged()[0].op.forgotten.is_some(), "the pick is taken");
        assert!(
            h.last(&subjects[0]).unwrap().op.forgotten.is_some(),
            "and the placement"
        );
        let rx = stub(vec![chose(AREA, 0.7)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        assert_eq!(bodies(&rx).len(), 1, "asked again");
    }

    /// J5, #730: a request a failed batch holds is in no batch, so the judge asks nothing of it, and
    /// the chain's call in the same pass, of the round the judge's pass read, leaves it out too
    /// (Slice 11c's "Holds").
    #[test]
    fn a_held_request_is_neither_judged_nor_placed_until_its_wait_ends() {
        let mut h = Home::new();
        let subjects = h.requests(2);
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let db = crate::providers_db::open(h.s.home.path()).unwrap();
        let later = crate::db::now_ms() + 3_600_000;
        crate::providers_db::set_my_work_held(&db, (NAME, &subjects[0]), "time", later).unwrap();
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        let rx = stub(vec![chose(AREA, 0.8)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        let sent = h.pass_with(Some(&judge), Box::new(Place::default()), &|p| {
            placed(p, &[AREA, AREA])
        });
        assert_eq!(bodies(&rx).len(), 1);
        let judged = h.judged();
        assert_eq!(judged.len(), 1);
        assert_eq!(judged[0].op.about, subjects[1]);
        assert_eq!(sent.len(), 1);
        assert_eq!(shown(&sent[0]), [("R1".to_owned(), 2)], "{}", sent[0]);
    }

    /// J5: a kept area renamed, or given another goal, before the request was placed: the pick
    /// for the areas as they were is not the current one, and the request is asked about again.
    #[test]
    fn a_kept_area_changed_before_the_placement_is_asked_about_again() {
        let mut h = Home::new();
        h.requests(1);
        h.area(AREA, "The parser", "kept", "owner");
        let judge = h.judge();
        let none = |_: &str| -> Value { panic!("the chain is not asked") };
        let rx = stub(vec![chose(AREA, 0.6), chose(AREA, 0.9)]);
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        h.area(AREA, "The parser and its lexer", "kept", "owner");
        h.pass_with(Some(&judge), Box::new(Place::default()), &none);
        let asked = bodies(&rx);
        assert_eq!(asked.len(), 2);
        assert_eq!(
            asked[1]["state"]["areas"][AREA]["name"],
            "The parser and its lexer"
        );
        let judged = h.judged();
        assert_eq!(judged.len(), 2);
        assert_eq!(judged[1].op.fields["p"], json!(0.9));
    }

    /// J5: the judge is told each repository of the request, the anchor's and its other
    /// conversation's, so that its own check covers them all; one it finds excluded keeps nothing,
    /// so the next round reads the request again, as J3's `left`.
    #[test]
    fn the_judge_is_told_each_repository_and_an_exclusion_keeps_nothing() {
        const R2: &str = "/r/lexer";
        let mut h = Home::new();
        h.said("s1a", 1, "Build part 1 of the parser.");
        h.s.said("s1b", R2, h.t + 31 * MINUTE, "Test part 1 of the lexer.");
        let keys = json!([["claude", "s1a"], ["claude", "s1b"]]);
        let groups = json!([{"conversations": keys, "title": "Request 1", "links": []}]);
        h.op("group", R, json!({"groups": groups, "ignored": []}));
        h.area(AREA, "The parser", "kept", "owner");
        let told = RefCell::new(Vec::new());
        let ops = h.with(|s| {
            let mut ask = |ask: crate::judge::Ask| -> Result<crate::judge::Asked> {
                told.borrow_mut().push(ask.repos.to_vec());
                Ok(crate::judge::Asked::Skipped("excluded"))
            };
            let mut judge = Judging {
                entry: crate::judge::NAME,
                label: "Jev",
                ask: &mut ask,
            };
            Place::default().judged(s, &mut judge, crate::db::now_ms())
        });
        assert!(ops.unwrap().is_empty());
        assert_eq!(told.into_inner(), [vec![R2.to_owned(), R.to_owned()]]);
    }
