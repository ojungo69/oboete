    use super::*;
    use crate::config::Summary;
    use crate::curate::{Phase, Reading, Reads};
    use crate::my_work_ops::MyWorkOp;
    use crate::my_work_run::Runner;
    use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Fallback, Gate, Skip};
    use crate::raw::OpKind;
    use crate::search::b::fixture::Store;

    const MINUTE: i64 = 60_000;

    /// A checkout with no remote: its repository key is its path.
    fn checkout() -> tempfile::TempDir {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
        std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        cwd
    }

    struct Home {
        s: Store,
    }

    impl Home {
        fn new() -> Self {
            Self { s: Store::new() }
        }

        fn op(&mut self, job: &str, about: &str, fields: Value) -> i64 {
            self.op_reading(job, about, fields, Read::default())
        }

        fn op_reading(&mut self, job: &str, about: &str, fields: Value, read: Read) -> i64 {
            let op = MyWorkOp {
                job: job.into(),
                about: about.into(),
                entry: "fake".into(),
                ts: crate::db::now_ms(),
                read,
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

        /// A review of `request` keeping `preferences` (statement, quote).
        fn reviewed(&mut self, request: &str, preferences: &[(&str, &str)]) -> i64 {
            self.reviewed_reading(request, preferences, Read::default())
        }

        fn reviewed_reading(
            &mut self,
            request: &str,
            preferences: &[(&str, &str)],
            read: Read,
        ) -> i64 {
            let preferences: Vec<Value> = (preferences.iter())
                .map(|(statement, quote)| json!({"statement": statement, "quote": quote}))
                .collect();
            let fields = json!({"state": "in-progress", "percent": 40, "summary": "S.",
                "milestones": [], "preferences": preferences});
            self.op_reading(crate::my_work_progress::NAME, request, fields, read)
        }

        /// Slice 8: the owner's action on a suggestion, as the page posts it.
        fn act(&self, body: Value) -> std::result::Result<Value, crate::settings::Refusal> {
            let body = serde_json::to_vec(&body).unwrap();
            crate::settings::claims::rule(self.s.home.path(), &std::sync::Mutex::new(()), &body)
        }

        fn page(&self) -> Value {
            page(self.s.home.path(), crate::db::now_ms()).unwrap()
        }

        /// `f` with raw.db closed, as a rebuild and a purge need it.
        fn closed(self, f: impl FnOnce(&std::path::Path)) -> Self {
            let Store { home, raw } = self.s;
            drop(raw);
            f(home.path());
            let raw = crate::raw::open(home.path()).unwrap();
            Self {
                s: Store { home, raw },
            }
        }

        /// The texts of the owner's rules of every repository My Rules shows, each with whether it
        /// is on.
        fn yours(&self) -> Vec<(String, bool)> {
            let page = self.page();
            let rules = page["yours"][0]["rules"].as_array().unwrap().clone();
            (rules.iter())
                .map(|r| (r["text"].as_str().unwrap().to_owned(), r["on"] == true))
                .collect()
        }

        fn waiting(&self) -> Vec<String> {
            let page = self.page();
            (page["waiting"].as_array().unwrap().iter())
                .map(|w| w["id"].as_str().unwrap().to_owned())
                .collect()
        }

        /// Whether each of the three agents is given `text` at the start of a session.
        fn started_with(&self, text: &str) -> [bool; 3] {
            let cwd = checkout();
            self.started_in(cwd.path(), text)
        }

        /// `started_with`, in checkout `cwd`.
        fn started_in(&self, cwd: &std::path::Path, text: &str) -> [bool; 3] {
            self.starts(cwd).map(|start| start.contains(text))
        }

        /// What each of the three agents is given at the start of a session in checkout `cwd`.
        fn starts(&self, cwd: &std::path::Path) -> [String; 3] {
            ["claude", "codex", "grok"].map(|agent| {
                let session = format!("start-{agent}-{}", crate::db::now_ms());
                crate::hook::started(self.s.home.path(), agent, &session, cwd)
            })
        }

        /// A checkout records name (a prompt there), and its repository key.
        fn known(&mut self) -> (tempfile::TempDir, String) {
            let cwd = checkout();
            let repo = crate::repo::key(cwd.path());
            let session = format!("in-{repo}");
            self.s.said(&session, &repo, crate::db::now_ms(), "Hello.");
            self.s.run();
            (cwd, repo)
        }

        fn with<T>(&self, f: impl FnOnce(&Stores) -> T) -> T {
            self.with_window(Summary::default().window_tokens, f)
        }

        fn with_window<T>(&self, window_tokens: u32, f: impl FnOnce(&Stores) -> T) -> T {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let rules = Rules::load(self.s.home.path()).unwrap();
            let summary = Summary {
                window_tokens,
                ..Summary::default()
            };
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

        fn due(&self) -> Vec<Due> {
            self.with(|s| Suggestions::default().due(s, crate::db::now_ms()).unwrap())
        }

        fn pass(&mut self, answer: &dyn Fn(&str) -> Value) -> (Phase, Vec<String>) {
            self.pass_within(Summary::default().window_tokens, answer)
        }

        fn pass_within(
            &mut self,
            window_tokens: u32,
            answer: &dyn Fn(&str) -> Value,
        ) -> (Phase, Vec<String>) {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let db = crate::providers_db::open(self.s.home.path()).unwrap();
            let rules = Rules::load(self.s.home.path()).unwrap();
            let summary = Summary {
                window_tokens,
                ..Summary::default()
            };
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
                vec![Box::new(Suggestions::default()) as Box<dyn Job>],
                Box::new(call),
                (&rules, &summary, "chain"),
            );
            let phase = runner.pass(&mut self.s.raw, &k, &db, false).unwrap();
            drop(runner);
            self.s.run();
            (phase, sent.into_inner())
        }

        /// The suggestions as kept, in their ids' order.
        fn suggestions(&self) -> Vec<Suggestion> {
            self.s.run();
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let none = HashSet::new();
            carried(&k, self.s.raw.device(), &none).unwrap().0
        }

        /// A suggestion `id` kept as its op keeps it.
        fn suggest(&mut self, id: &str, statement: &str, seen: u64, at: i64) -> i64 {
            let requests: Vec<String> = (0..seen).map(|n| format!("g:{id}{n}")).collect();
            let fields = json!({"statement": statement, "seen": seen, "requests": requests,
                "quotes": [{"text": "q", "request": "g:0", "at": at}]});
            self.op(NAME, id, fields)
        }

        /// The rules ops as if written `minutes` earlier.
        fn age(&self, minutes: i64) {
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            k.execute(
                "UPDATE my_work SET ts = ts - ?1 WHERE job = ?2",
                rusqlite::params![minutes * MINUTE, NAME],
            )
            .unwrap();
        }
    }

    /// An answer giving each new preference shown, in order, `items`'s (joins, statement).
    fn answer(items: &[(&str, &str)]) -> Value {
        let items: Vec<Value> = (items.iter().enumerate())
            .map(|(n, (joins, statement))| {
                json!({"preference": format!("P{}", n + 1), "joins": joins, "statement": statement})
            })
            .collect();
        json!({"preferences": items})
    }

    /// R1: a new preference makes a suggestion; one the next review words differently joins it,
    /// which keeps its id and wording and counts the second request.
    #[test]
    fn a_reworded_preference_joins_its_suggestion() {
        let mut h = Home::new();
        h.reviewed("g:1", &[("Tests come first.", "write the tests first")]);
        let (_, sent) = h.pass(&|_| answer(&[("", "Write the tests before the code.")]));
        assert!(sent[0].contains("(none yet)") && sent[0].contains("P1: Tests come first."));
        let first = h.suggestions();
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].id.as_str(), first[0].requests.len()), ("s1", 1));
        let before = crate::db::now_ms();
        h.reviewed(
            "g:2",
            &[("Tests before code, always.", "tests before the code please")],
        );
        let written = before..=crate::db::now_ms();
        h.age(4 * 60);
        let (_, sent) = h.pass(&|_| answer(&[("s1", "")]));
        assert!(
            sent[0].contains(
                "- s1 (seen in 1 request, not decided): Write the tests before the code."
            ),
            "{}",
            sent[0]
        );
        assert!(
            !sent[0].contains("Tests come first."),
            "a preference merged is not shown again"
        );
        let after = h.suggestions();
        assert_eq!(after.len(), 1, "joined, not a new one");
        assert_eq!(after[0].statement, "Write the tests before the code.");
        assert_eq!(
            (after[0].seen, &after[0].requests[..]),
            (2, &["g:2".to_owned(), "g:1".to_owned()][..])
        );
        assert_eq!(
            after[0].quotes[0].text, "tests before the code please",
            "the newest quote first"
        );
        assert!(
            written.contains(&after[0].quotes[0].at),
            "its review's time"
        );
        assert!(after[0].quotes[1].at <= after[0].quotes[0].at);
        h.reviewed("g:2", &[("Tests first, really.", "tests first really")]);
        h.age(4 * 60);
        h.pass(&|_| answer(&[("s1", "")]));
        let again = h.suggestions();
        assert_eq!(again[0].seen, 2, "a request reviewed again counts once");
        assert_eq!(again[0].requests, ["g:2", "g:1"]);
        assert_eq!(again[0].quotes.len(), 3);
    }

    /// R1, R3: a suggestion the owner rejected is shown as rejected, a preference saying it again
    /// joins it, and it stays rejected: the decision is the owner's `rule` op, kept apart.
    #[test]
    fn a_rejected_suggestion_stays_rejected_when_seen_again() {
        let mut h = Home::new();
        h.reviewed("g:1", &[("Use tabs.", "use tabs")]);
        h.pass(&|_| answer(&[("", "Indent with tabs.")]));
        h.op(RULE, "s1", json!({"action": "reject"}));
        h.reviewed("g:2", &[("Tabs, not spaces.", "tabs not spaces")]);
        h.age(4 * 60);
        let (_, sent) = h.pass(&|_| answer(&[("s1", "")]));
        assert!(
            sent[0].contains("- s1 (seen in 1 request, rejected): Indent with tabs."),
            "{}",
            sent[0]
        );
        assert_eq!(h.suggestions().len(), 1);
        let state = h.with(|s| decided(s.k, s.raw.device(), &HashSet::new()).unwrap());
        assert_eq!(state["s1"].state, "rejected");
        h.op(RULE, "s1", json!({"action": "undo"}));
        let state = h.with(|s| decided(s.k, s.raw.device(), &HashSet::new()).unwrap());
        assert!(!state.contains_key("s1"), "undone");
    }

    /// R1: ten preferences a call, by whole reviews, the oldest first; the rest at once.
    #[test]
    fn ten_preferences_a_call_the_rest_at_once() {
        let mut h = Home::new();
        let three = |n: usize| -> Vec<(String, String)> {
            (0..3)
                .map(|i| (format!("Rule {n}.{i}."), format!("rule {n} {i}")))
                .collect()
        };
        for n in 1..=4 {
            let p = three(n);
            let p: Vec<(&str, &str)> = p.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            h.reviewed(&format!("g:{n}"), &p);
        }
        let new = |prompt: &str| -> Value {
            let n = prompt.lines().filter(|l| l.starts_with("- P")).count();
            let items: Vec<(String, String)> = (1..=n)
                .map(|i| (String::new(), format!("Rule {i} of {n}.")))
                .collect();
            let items: Vec<(&str, &str)> = items
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            answer(&items)
        };
        let (_, sent) = h.pass(&new);
        let shown = sent[0].lines().filter(|l| l.starts_with("- P")).count();
        assert_eq!(shown, 9, "three whole reviews: {}", sent[0]);
        assert!(
            sent[0].contains("Rule 1.0.") && !sent[0].contains("Rule 4.0."),
            "the oldest first"
        );
        let due = h.due();
        assert_eq!(due.len(), 1, "the rest");
        let last = h.with(|s| round(s).unwrap().latest.unwrap());
        assert_eq!(due[0].since, last, "at once");
        // #734: one left from before the run and one written since: at once still.
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.reviewed("g:5", &[("Rule 5.", "rule 5")]);
        assert_eq!(
            h.due()[0].since,
            last,
            "at once, though a newer review came"
        );
        let (_, sent) = h.pass(&new);
        assert!(
            sent[0].contains("Rule 4.0.") && !sent[0].contains("Rule 1.0."),
            "{}",
            sent[0]
        );
        assert!(h.due().is_empty());
    }

    /// A5: what the job read is kept for the round, and `Job::reread` drops it. Here
    /// a preference a review kept since is in its next read, and not while the first is
    /// kept. The runner tells every job so when the list changed between a round's two passes
    /// (`my_work_run`).
    #[test]
    fn the_job_keeps_its_read_until_it_is_told_to_read_again() {
        let mut h = Home::new();
        let job = Suggestions::default();
        let due = |h: &Home| h.with(|s| job.due(s, crate::db::now_ms()).unwrap().len());
        assert_eq!(due(&h), 0);
        h.reviewed("g:1", &[("Tests come first.", "tests first")]);
        assert_eq!(due(&h), 0, "kept for the round");
        job.reread();
        assert_eq!(due(&h), 1, "read again");
    }

    /// R1, P7: a review adding preferences after a run makes the suggestions due three hours after
    /// it; none before.
    #[test]
    fn new_preferences_wait_three_hours_after_the_last_run() {
        let mut h = Home::new();
        assert!(h.due().is_empty());
        h.reviewed("g:1", &[("Tests come first.", "tests first")]);
        h.pass(&|_| answer(&[("", "Write the tests first.")]));
        assert!(h.due().is_empty());
        h.reviewed("g:2", &[("Small commits.", "small commits")]);
        let last = h.with(|s| round(s).unwrap().latest.unwrap());
        let due = h.due();
        assert_eq!(due[0].since, last + AGAIN_MS);
        let (_, sent) = h.pass(&|_| answer(&[("", "Commit in small steps.")]));
        assert!(sent.is_empty());
    }

    /// A5: a quote is gated again as it is kept, so a rule added after its review masks it.
    #[test]
    fn a_quote_is_gated_again_as_it_is_kept() {
        let toml =
            "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n";
        let capture = crate::config::parse_capture(Some(toml)).unwrap();
        let rules = Rules::new(&capture.redaction).unwrap();
        let p = Preference {
            request: "g:1".into(),
            review: 1,
            ts: 0,
            statement: "Name it.".into(),
            quote: "call it codename-otter".into(),
        };
        let c = Context {
            suggestions: Vec::new(),
            taken: vec![p],
            next_id: 1,
            rules,
        };
        let kept = keep(&answer(&[("", "Name things plainly.")]), &c);
        let text = kept["s1"]["quotes"][0]["text"].as_str().unwrap();
        assert!(!text.contains("codename-otter"), "{text}");
        // #737: a suggestion carried as kept before the rule is gated again as it is written.
        let carried = Suggestion {
            id: "s1".into(),
            statement: "Keep codename-otter out of logs.".into(),
            seen: 1,
            requests: vec!["g:0".into()],
            quotes: vec![Quote {
                text: "codename-otter stays out".into(),
                request: "g:0".into(),
                at: 0,
            }],
        };
        let c = Context {
            suggestions: vec![carried],
            next_id: 2,
            ..c
        };
        let kept = keep(&answer(&[("s1", "")]), &c);
        assert!(
            !kept["s1"].to_string().contains("codename-otter"),
            "{}",
            kept["s1"]
        );
        assert_eq!(
            kept.len(),
            2,
            "through and the suggestion changed: {kept:?}"
        );
    }

    /// R1, A10: each preference answered once and no other, a join of an id shown, a new
    /// statement there and within 200 characters.
    #[test]
    fn the_check_holds_the_labels_ids_and_caps() {
        let ids: HashSet<String> = ["s1".to_owned()].into();
        assert_eq!(check(&answer(&[("s1", ""), ("", "New.")]), 2, &ids), None);
        assert_eq!(
            check(&answer(&[("s1", "")]), 2, &ids),
            Some("shape"),
            "one missed"
        );
        assert_eq!(
            check(&answer(&[("s9", ""), ("", "New.")]), 2, &ids),
            Some("unanchored")
        );
        assert_eq!(
            check(&answer(&[("", " "), ("", "New.")]), 2, &ids),
            Some("empty")
        );
        let long = "x".repeat(STATEMENT + 1);
        assert_eq!(
            check(&answer(&[("", &long), ("", "New.")]), 2, &ids),
            Some("over_cap")
        );
        let twice = json!({"preferences": [{"preference": "P1", "joins": "s1", "statement": ""},
            {"preference": "P1", "joins": "s1", "statement": ""}]});
        assert_eq!(check(&twice, 2, &ids), Some("shape"));
        // #734: P01 or P+1 is P1 again, so P2 would go unanswered.
        for other in ["P01", "P+1"] {
            let again = json!({"preferences": [{"preference": "P1", "joins": "s1", "statement": ""},
                {"preference": other, "joins": "s1", "statement": ""}]});
            assert_eq!(check(&again, 2, &ids), Some("shape"), "{other}");
        }
    }

    /// #734: a suggestion keeps its newest three quotes by their reviews' times, whatever order
    /// the answer named the preferences in.
    #[test]
    fn the_newest_quotes_are_kept_whatever_the_answers_order() {
        let mut h = Home::new();
        h.reviewed("g:1", &[("Tests come first.", "first quote")]);
        h.pass(&|_| answer(&[("", "Write the tests first.")]));
        for (request, quote) in [("g:2", "second quote"), ("g:3", "third quote")] {
            std::thread::sleep(std::time::Duration::from_millis(5));
            h.reviewed(request, &[("Tests first.", quote)]);
        }
        h.age(4 * 60);
        let reversed = json!({"preferences": [
            {"preference": "P2", "joins": "s1", "statement": ""},
            {"preference": "P1", "joins": "s1", "statement": ""}]});
        h.pass(&|_| reversed.clone());
        let quotes: Vec<String> = (h.suggestions()[0].quotes.iter())
            .map(|q| q.text.clone())
            .collect();
        assert_eq!(quotes, ["third quote", "second quote", "first quote"]);
    }

    /// Slice 8: one suggestion, `s1`, merged from a review of `g:1`.
    fn suggested(h: &mut Home) {
        h.reviewed("g:1", &[("Tests come first.", "write the tests first")]);
        h.pass(&|_| answer(&[("", "Write the tests before the code.")]));
    }

    const RULE_TEXT: &str = "Write the tests before the code.";

    /// R3, R4 (decision 60): an approved suggestion is the owner's global preference, given to
    /// Claude Code, Codex and Grok at the start of every session; My Rules lists it under the owner's
    /// rules with its suggestion, and no longer as waiting.
    #[test]
    fn an_approved_rule_is_a_decided_preference_in_every_agents_session_start() {
        let mut h = Home::new();
        suggested(&mut h);
        assert_eq!(h.started_with(RULE_TEXT), [false; 3]);
        assert_eq!(h.waiting(), ["s1"]);
        let got = h
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        assert_eq!(got["state"], "applied", "{got}");
        assert_eq!(h.started_with(RULE_TEXT), [true; 3]);
        let claim = crate::search::b::claim(h.s.home.path(), got["uid"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            (claim.scope.as_str(), claim.text.as_str()),
            ("global", RULE_TEXT)
        );
        let page = h.page();
        assert_eq!(page["waiting"], json!([]));
        let rule = &page["yours"][0]["rules"][0];
        assert_eq!(page["yours"][0]["scope"], "global");
        assert_eq!(
            (
                &rule["text"],
                &rule["on"],
                &rule["suggestion"],
                &rule["seen"]
            ),
            (&json!(RULE_TEXT), &json!(true), &json!("s1"), &json!(1))
        );
        assert_eq!(rule["quotes"][0]["text"], "write the tests first");
    }

    /// Slice 8b (R2, R4): a suggestion approved for one repository is a decided preference of it,
    /// at the start of each of the three agents' sessions there under a heading of its own, once,
    /// and in no other repository's. A preference the curator found there is no rule of the owner's:
    /// it stays among the decisions, and My Rules does not list it.
    #[test]
    fn a_rule_of_one_repository_starts_only_its_sessions() {
        let mut h = Home::new();
        suggested(&mut h);
        let ((a, in_a), (b, _)) = (h.known(), h.known());
        let found = "Keep the changelog short.";
        let seq = h.s.said("cur", &in_a, crate::db::now_ms(), found);
        h.s.claim(seq, found, ("preference", "decided", "user"), &[]);
        h.s.run();
        let got = h
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "repo", "repo": in_a}))
            .unwrap();
        assert_eq!(got["state"], "applied", "{got}");
        let claim = crate::search::b::claim(h.s.home.path(), got["uid"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            (claim.scope.as_str(), claim.text.as_str()),
            ("repo", RULE_TEXT)
        );
        for start in h.starts(a.path()) {
            let start = start.replace("\\n", "\n");
            assert!(
                !start.contains("- oboete on"),
                "the directive is no session: {start}"
            );
            let heading = "## Preferences of this repository\n";
            let block = start
                .split(heading)
                .nth(1)
                .unwrap_or_else(|| panic!("{start}"));
            let block = block.split("\n## ").next().unwrap();
            assert!(block.contains(RULE_TEXT), "{start}");
            assert!(!block.contains(found), "{start}");
            assert!(start.contains(found), "{start}");
            // Not again among the decisions or the index.
            for section in start.split("## ").skip(1) {
                let ours = section.starts_with("Preferences of this repository");
                assert!(ours || !section.contains(RULE_TEXT), "{start}");
            }
        }
        assert_eq!(h.started_in(b.path(), RULE_TEXT), [false; 3]);
        assert_eq!(h.started_with(RULE_TEXT), [false; 3]);
        let page = h.page();
        assert_eq!(page["yours"][0], json!({"scope": "global", "rules": []}));
        let ours = &page["yours"][1];
        assert_eq!(
            (&ours["scope"], &ours["repo"], &ours["rules"][0]["text"]),
            (&json!("repo"), &json!(in_a), &json!(RULE_TEXT))
        );
        assert_eq!(ours["rules"].as_array().unwrap().len(), 1, "{page}");
        assert_eq!(ours["rules"][0]["suggestion"], "s1");
        assert_eq!(page["yours"].as_array().unwrap().len(), 2, "{page}");
        let offered: Vec<&Value> = (page["repos"].as_array().unwrap().iter())
            .map(|r| &r["repo"])
            .collect();
        assert!(offered.contains(&&json!(in_a)), "{page}");
        // Turned off, it is given to no agent; undone, it is retracted and waits again.
        let uid = got["uid"].as_str().unwrap();
        crate::claims::mute(h.s.home.path(), uid, true).unwrap();
        assert_eq!(h.started_in(a.path(), RULE_TEXT), [false; 3]);
        h.act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        assert_eq!(h.waiting(), ["s1"]);
        assert_eq!(h.page()["yours"].as_array().unwrap().len(), 1);
    }

    /// #743: a repository rule's directive is no raw search hit and starts no session, and a name
    /// the gate masks is refused.
    #[test]
    fn a_repository_rules_directive_stays_out_of_search_and_sessions() {
        let mut h = Home::new();
        let (a, in_a) = h.known();
        let home = h.s.home.path().to_owned();
        crate::claims::pref_add_in(&home, "Keep the changelog short.", Some(&in_a)).unwrap();
        h.s.run();
        let k = crate::knowledge::open(&home).unwrap();
        let count = |sql: &str| -> i64 { k.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            (
                count("SELECT COUNT(*) FROM raw_docs WHERE kind = 'directive'"),
                count("SELECT COUNT(*) FROM session_start WHERE session = 'owner'"),
            ),
            (0, 0)
        );
        assert!(h.started_in(a.path(), "changelog")[0], "still delivered");
        // #748: its claim is found by its words, as every rule's is.
        let found = h.s.query(&crate::search::b::Query {
            text: "changelog".into(),
            caller: Some(in_a.clone()),
            limit: 20,
            ..Default::default()
        });
        let uid = &crate::claims::owned(&k, &in_a).unwrap()[0].uid;
        assert!(
            found.hits.iter().any(|hit| &hit.key == uid),
            "{:?}",
            found.hits
        );
        // Item 6: excluding another repository with a rule leaves this one's rule, and a global
        // one, of no excluded session.
        let (_b, in_b) = h.known();
        crate::claims::pref_add_in(&home, "Tag every release.", Some(&in_b)).unwrap();
        let global = crate::claims::pref_add(&home, "Answer in Japanese.").unwrap();
        (h.s.raw).exclude_capture(("repo", &in_b), false).unwrap();
        h.s.run();
        let k = crate::knowledge::open(&home).unwrap();
        let excluded =
            h.s.raw
                .sessions_in(&h.s.raw.exclusions_now().unwrap())
                .unwrap();
        let ours = crate::claims::owned(&k, &in_a).unwrap();
        for uid in [ours[0].uid.as_str(), global.as_str()] {
            assert!(
                !crate::curate::quotes_excluded(&h.s.raw, &k, &excluded, uid).unwrap(),
                "{uid}"
            );
        }
        // A name the gate masks.
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"t\", regex = 'tok-[0-9]{6}' }]\n",
        )
        .unwrap();
        let body =
            json!({"text": "Hi.", "apply_to_all_repos": false, "repo": "github.com/o/tok-123456"});
        let refused = crate::settings::claims::pref_add(
            &home,
            &std::sync::Mutex::new(()),
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap_err();
        assert_eq!(refused.code, "preference_repo_masked");
        // #745: so is one whose directives' session the gate masks.
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"s\", regex = 'owner:github[.]com/o/r' }]\n",
        )
        .unwrap();
        let body = json!({"text": "Hi.", "apply_to_all_repos": false, "repo": "github.com/o/r"});
        let refused = crate::settings::claims::pref_add(
            &home,
            &std::sync::Mutex::new(()),
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap_err();
        assert_eq!(refused.code, "preference_repo_masked");
        std::fs::remove_file(home.join("config.toml")).unwrap();
    }

    /// #748: a repository whose last ordinary record is forgotten, while its owner rule stays, still
    /// takes a rule.
    #[test]
    fn a_repository_known_by_its_rules_takes_another() {
        let mut h = Home::new();
        let cwd = checkout();
        let repo = crate::repo::key(cwd.path());
        let x = h.s.said("s1", &repo, crate::db::now_ms(), "Hello.");
        h.s.run();
        crate::claims::pref_add_in(h.s.home.path(), "Keep the changelog short.", Some(&repo))
            .unwrap();
        let device = h.s.raw.device().to_owned();
        let h = h.closed(|home| {
            let target = crate::forget::Target::Record { device, seq: x };
            crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            crate::forget::purge(home).unwrap();
        });
        let home = h.s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let named: bool = k
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM raw_docs WHERE repo = ?1)",
                [&repo],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!named, "no ordinary record names it");
        crate::claims::pref_add_in(home, "Tag every release.", Some(&repo)).unwrap();
        assert_eq!(crate::claims::owned(&k, &repo).unwrap().len(), 2);
    }

    /// Slice 8b (ponytail-review of 8a): an approved suggestion whose rule is gone, retracted with
    /// `oboete correct` or forgotten, is listed as removed, with Undo: undone, it waits again.
    #[test]
    fn an_approved_suggestion_whose_rule_is_gone_is_listed_as_removed() {
        let mut h = Home::new();
        suggested(&mut h);
        let got = h
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        assert_eq!(h.page()["removed"], json!([]));
        let uid = got["uid"].as_str().unwrap();
        crate::claims::correct(h.s.home.path(), uid, Some("retracted"), None).unwrap();
        let page = h.page();
        assert_eq!(page["yours"][0]["rules"], json!([]));
        assert_eq!(page["removed"][0]["id"], "s1", "{page}");
        assert_eq!(page["waiting"], json!([]));
        h.act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        let page = h.page();
        assert_eq!(
            (page["removed"].clone(), h.waiting()),
            (json!([]), vec!["s1".to_owned()])
        );
    }

    /// R3: edited, approved, then turned off; another rejected, then undone: all ops, so a rebuild
    /// keeps every one of them.
    #[test]
    fn edit_turn_off_and_undo_survive_a_rebuild() {
        let mut h = Home::new();
        h.reviewed("g:1", &[("Tests come first.", "write the tests first")]);
        h.reviewed("g:2", &[("Tabs.", "tabs please")]);
        h.pass(&|_| answer(&[("", RULE_TEXT), ("", "Indent with tabs.")]));
        let edited = "Write the tests first, then the code.";
        let got = h
            .act(
                json!({"suggestion": "s1", "action": "approve", "scope": "global",
                "statement": edited}),
            )
            .unwrap();
        let uid = got["uid"].as_str().unwrap().to_owned();
        crate::claims::correct(h.s.home.path(), &uid, None, Some("Tests first, always.")).unwrap();
        crate::claims::mute(h.s.home.path(), &uid, true).unwrap();
        h.act(json!({"suggestion": "s2", "action": "reject"}))
            .unwrap();
        assert_eq!(h.page()["rejected"][0]["id"], "s2");
        h.act(json!({"suggestion": "s2", "action": "undo"}))
            .unwrap();
        let before = (h.yours(), h.waiting());
        assert_eq!(
            before,
            (
                vec![("Tests first, always.".to_owned(), false)],
                vec!["s2".to_owned()]
            )
        );
        let h = h.closed(|home| crate::worker::rebuild(home).unwrap());
        assert_eq!((h.yours(), h.waiting()), before);
        assert_eq!(h.page()["rejected"], json!([]));
        assert_eq!(h.started_with("Tests first"), [false; 3], "turned off");
    }

    /// R3: an approve is undone from a page opened later, as from the one that made it: the
    /// suggestion waits again, its preference is retracted, and no agent is given it; approved
    /// again, it is one new preference.
    #[test]
    fn undo_works_after_a_reload() {
        let mut h = Home::new();
        suggested(&mut h);
        h.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        let reloaded = Home { s: h.s };
        assert_eq!(reloaded.waiting(), Vec::<String>::new());
        let got = reloaded
            .act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        assert_eq!(got["state"], "applied");
        assert_eq!(reloaded.waiting(), ["s1"]);
        assert_eq!(reloaded.yours(), Vec::<(String, bool)>::new());
        assert_eq!(reloaded.started_with(RULE_TEXT), [false; 3]);
        reloaded
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        assert_eq!(reloaded.yours(), [(RULE_TEXT.to_owned(), true)]);
        assert_eq!(reloaded.started_with(RULE_TEXT), [true; 3]);
    }

    /// R5: forgetting the prompt a suggestion was learned from takes the suggestion (S5), not the
    /// rule the owner approved from it.
    #[test]
    fn a_forget_of_a_suggestions_source_leaves_the_approved_rule() {
        let mut h = Home::new();
        let x =
            h.s.said("s1", "repo", 1_000, "write the tests first, always");
        let read = Read {
            spans: vec![(x, x)],
            ops: vec![],
        };
        h.reviewed_reading(
            "g:1",
            &[("Tests come first.", "write the tests first")],
            read,
        );
        h.pass(&|_| answer(&[("", RULE_TEXT)]));
        h.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        let device = h.s.raw.device().to_owned();
        let h = h.closed(|home| {
            let target = crate::forget::Target::Record { device, seq: x };
            crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            crate::forget::purge(home).unwrap();
        });
        let page = h.page();
        assert_eq!(
            (&page["waiting"], &page["rejected"]),
            (&json!([]), &json!([])),
            "{page}"
        );
        assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
        assert_eq!(page["yours"][0]["rules"][0]["suggestion"], Value::Null);
        assert_eq!(h.started_with(RULE_TEXT), [true; 3]);
    }

    /// V3, V6: what the route refuses before anything is written.
    #[test]
    fn a_rule_action_is_refused_before_it_writes() {
        let mut h = Home::new();
        suggested(&mut h);
        let (_cwd, stopped) = h.known();
        (h.s.raw)
            .exclude_capture(("repo", &stopped), false)
            .unwrap();
        let code = |body: Value| h.act(body).map(|_| ()).map_err(|r| (r.status, r.code));
        assert_eq!(
            code(json!({"suggestion": "s9", "action": "reject"})),
            Err((404, "rule_not_found"))
        );
        assert_eq!(
            code(json!({"suggestion": "x1", "action": "reject"})),
            Err((400, "bad_request"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "keep"})),
            Err((400, "bad_request"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "reject", "statement": "Other."})),
            Err((400, "bad_request"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "approve"})),
            Err((422, "rule_scope"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "approve", "scope": "repo"})),
            Err((422, "rule_scope"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "approve", "scope": "global", "repo": "r"})),
            Err((422, "rule_scope"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "reject", "repo": "r"})),
            Err((400, "bad_request"))
        );
        // Slice 8b: a repository no record names, or one the owner stopped recording.
        let repo =
            |r: &str| json!({"suggestion": "s1", "action": "approve", "scope": "repo", "repo": r});
        assert_eq!(
            code(repo("github.com/no/such")),
            Err((422, "preference_repo_unknown"))
        );
        assert_eq!(code(repo(&stopped)), Err((422, "preference_repo_excluded")));
        // Not offered either.
        let offered = h.page()["repos"].clone();
        assert!(!offered.to_string().contains(&stopped), "{offered}");
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "undo"})),
            Err((409, "rule_not_decided"))
        );
        assert_eq!(
            code(
                json!({"suggestion": "s1", "action": "approve", "scope": "global",
                "statement": "  "})
            ),
            Err((422, "preference_empty"))
        );
        assert_eq!(h.waiting(), ["s1"], "nothing decided");
        h.act(json!({"suggestion": "s1", "action": "reject"}))
            .unwrap();
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "reject"})),
            Err((409, "rule_decided"))
        );
        assert_eq!(
            code(json!({"suggestion": "s1", "action": "approve", "scope": "global"})),
            Err((409, "rule_decided"))
        );
        assert_eq!(h.yours(), Vec::<(String, bool)>::new());
    }

    /// #741: a suggestion of long request ids is kept in an op that fits, the newest requests and
    /// quotes first, its count unchanged; a short one is kept whole.
    #[test]
    fn a_suggestion_of_long_request_ids_is_kept_within_its_op() {
        let id = |n: usize| format!("c:claude:{n}-{}", "x".repeat(1_000));
        let sg = Suggestion {
            id: "s1".into(),
            statement: "Tests first.".into(),
            seen: 250,
            requests: (0..REQUESTS).map(id).collect(),
            quotes: (0..QUOTES)
                .map(|n| Quote {
                    text: "q".repeat(QUOTE),
                    request: id(n),
                    at: 10 - n as i64,
                })
                .collect(),
        };
        let gate = |t: &str| t.to_owned();
        let kept = fields(&sg, &gate);
        assert!(kept.to_string().len() <= FIELDS_BYTES);
        let requests = kept["requests"].as_array().unwrap();
        assert!(!requests.is_empty() && requests.len() < REQUESTS);
        assert_eq!(requests[0], json!(id(0)), "the newest first");
        assert_eq!(kept["seen"], 250);
        let short = Suggestion {
            requests: vec!["g:1".into()],
            ..sg.clone()
        };
        let whole = fields(&short, &gate);
        assert_eq!(
            (
                whole["requests"].clone(),
                whole["quotes"].as_array().unwrap().len()
            ),
            (json!(["g:1"]), QUOTES)
        );
        // Kept as the runner writes it: no skip.
        let op = json!({"job": NAME, "about": "s1", "entry": "e", "ts": 1, "fields": kept});
        assert!(op.to_string().len() <= crate::raw::MAX_OP_BYTES);
    }

    /// V3: the waiting suggestions strongest first, claude-mem's strength: the requests that saw
    /// it over one plus the weeks since its newest quote.
    #[test]
    fn the_waiting_suggestions_go_strongest_first() {
        let now = 100 * DAY_MS;
        let sg = |id: &str, requests: usize, days_ago: i64| Suggestion {
            id: id.into(),
            statement: id.into(),
            seen: requests as u64,
            requests: (0..requests).map(|n| format!("g:{n}")).collect(),
            quotes: vec![Quote {
                text: "q".into(),
                request: "g:0".into(),
                at: now - days_ago * DAY_MS,
            }],
        };
        assert_eq!(strength(&sg("a", 2, 0), now), 2.0);
        // `seen` counts the requests past the newest kept (#737).
        let counted = Suggestion {
            seen: 6,
            ..sg("f", 1, 0)
        };
        assert_eq!(strength(&counted, now), 6.0);
        assert_eq!(strength(&sg("b", 2, 7), now), 1.0);
        assert_eq!(strength(&sg("c", 3, 14), now), 1.0);
        assert!(strength(&sg("d", 1, 0), now) > strength(&sg("e", 3, 30), now));
    }

    /// V3: each list in its order: the suggestions waiting strongest first, the rejected ones
    /// the newest decision first, and the owner's rules on ones first, then the newest.
    #[test]
    fn my_rules_orders_each_list() {
        let mut h = Home::new();
        let now = crate::db::now_ms();
        for (id, seen, days_ago) in [
            ("s1", 1, 30),
            ("s3", 1, 0),
            ("s2", 2, 0),
            ("s4", 1, 0),
            ("s5", 1, 0),
        ] {
            h.suggest(id, &format!("Rule {id}."), seen, now - days_ago * DAY_MS);
        }
        for id in ["s4", "s5"] {
            h.act(json!({"suggestion": id, "action": "reject"}))
                .unwrap();
        }
        let mut uids = Vec::new();
        for text in ["Rule A.", "Rule B.", "Rule C."] {
            uids.push(crate::claims::pref_add(h.s.home.path(), text).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::claims::mute(h.s.home.path(), &uids[1], true).unwrap();
        let page = h.page();
        let ids = |list: &Value| -> Vec<String> {
            (list.as_array().unwrap().iter())
                .map(|v| v["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(ids(&page["waiting"]), ["s2", "s3", "s1"], "strongest first");
        assert_eq!(
            ids(&page["rejected"]),
            ["s5", "s4"],
            "the newest decision first"
        );
        let yours: Vec<(String, bool)> = h.yours();
        assert_eq!(
            yours,
            [
                ("Rule C.".to_owned(), true),
                ("Rule A.".to_owned(), true),
                ("Rule B.".to_owned(), false)
            ],
            "on first, then the newest"
        );
    }

    /// R3: an approve whose preference was forgotten since is still undone: there is nothing left
    /// to retract, and the suggestion waits again.
    #[test]
    fn an_approve_whose_rule_was_forgotten_is_still_undone() {
        let mut h = Home::new();
        suggested(&mut h);
        let got = h
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        let uid = got["uid"].as_str().unwrap().to_owned();
        let h = h.closed(|home| {
            let target = crate::forget::Target::Uid { uid };
            crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            crate::forget::purge(home).unwrap();
        });
        assert_eq!(h.yours(), Vec::<(String, bool)>::new());
        let got = h
            .act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        assert_eq!(got["state"], "applied");
        assert_eq!(h.waiting(), ["s1"]);
    }

    /// #737: a new suggestion's id is past every id an op names, a purged one's (its fields
    /// cleared, its subject kept) and the owner's decisions' too.
    #[test]
    fn a_new_id_is_past_every_id_an_op_names() {
        let mut h = Home::new();
        h.op(NAME, "s9", json!({}));
        h.op(RULE, "s12", json!({"action": "reject"}));
        h.reviewed("g:1", &[("Tests first.", "tests first")]);
        h.age(4 * 60);
        h.pass(&|_| answer(&[("", "Write the tests first.")]));
        let ids: Vec<String> = h.suggestions().into_iter().map(|sg| sg.id).collect();
        assert_eq!(ids, ["s13"]);
    }

    /// #737: a call writes an op for `through` and one for each suggestion it changed, none for
    /// the others.
    #[test]
    fn a_call_writes_only_the_suggestions_it_changed() {
        let mut h = Home::new();
        let now = crate::db::now_ms();
        h.suggest("s1", "Write the tests first.", 1, now);
        h.suggest("s2", "Indent with spaces.", 1, now);
        h.reviewed("g:1", &[("Tests before code.", "tests before code")]);
        let count = |h: &Home| -> i64 {
            h.s.run();
            let k = crate::knowledge::open(h.s.home.path()).unwrap();
            k.query_row("SELECT count(*) FROM my_work WHERE job = ?1", [NAME], |r| {
                r.get(0)
            })
            .unwrap()
        };
        h.age(4 * 60);
        let before = count(&h);
        h.pass(&|_| answer(&[("s1", "")]));
        assert_eq!(count(&h), before + 2, "through and s1");
        let kept = h.suggestions();
        assert_eq!((kept[0].seen, kept[1].seen), (2, 1));
    }

    /// #736: a call fits the window however many suggestions there are, the new preferences first;
    /// a rejected suggestion is shown before stronger undecided ones, so a preference saying it
    /// again joins it rather than making it new.
    #[test]
    fn a_call_fits_the_window_and_shows_the_rejected_first() {
        let mut h = Home::new();
        let now = crate::db::now_ms();
        let long = "x".repeat(STATEMENT - 10);
        for n in 1..=40 {
            h.suggest(&format!("s{n}"), &format!("{n} {long}"), 9, now);
        }
        let s41 = h.suggest("s41", &format!("Tabs {long}"), 1, now - 60 * DAY_MS);
        let rejected = h.op(RULE, "s41", json!({"action": "reject"}));
        h.reviewed("g:1", &[("Tabs.", "tabs please")]);
        let window = 1_500;
        let ask = h.with_window(window, |s| {
            Suggestions::default()
                .ask(s, THROUGH, crate::db::now_ms())
                .unwrap()
                .unwrap()
        });
        let data = &ask.prompt[ask.prompt.rfind("## The suggestions").unwrap()..];
        assert!(
            crate::budget::estimate(data) <= window + 64,
            "{}",
            data.len()
        );
        assert!(
            ask.prompt
                .contains("- s41 (seen in 1 request, rejected): Tabs ")
        );
        assert!(
            ask.prompt.contains("P1: Tabs."),
            "the new preference is shown"
        );
        assert!(!ask.prompt.contains("- s40 "), "the window leaves some out");
        assert_eq!((ask.check)(&answer(&[("s41", "")])), None);
        assert!(
            ask.read.ops.contains(&s41) && ask.read.ops.contains(&rejected),
            "what it was shown (S2)"
        );
        assert_eq!((ask.check)(&answer(&[("s40", "")])), Some("unanchored"));
    }

    /// #736: a first review past the window is still asked about, with no room left for the
    /// suggestions.
    #[test]
    fn a_review_past_the_window_leaves_no_suggestion_shown() {
        let mut h = Home::new();
        h.suggest("s1", "Indent with tabs.", 1, crate::db::now_ms());
        let long = "y".repeat(QUOTE);
        h.reviewed("g:1", &[("Tabs.", long.as_str())]);
        let ask = h.with_window(20, |s| {
            Suggestions::default()
                .ask(s, THROUGH, crate::db::now_ms())
                .unwrap()
                .unwrap()
        });
        assert!(ask.prompt.contains("P1: Tabs."), "{}", ask.prompt);
        assert!(!ask.prompt.contains("- s1 "), "{}", ask.prompt);
    }

    /// S2: a new statement worded as a suggestion the window left out of the prompt joins it and
    /// carries its quotes into a new op, which lists no op of that suggestion. Every call reads
    /// the last call's `through` op, though, and through it each call before: a forget of what
    /// a quote came from takes it out of My Rules, however many calls carried it since. A
    /// suggestion made before the quote was kept stays.
    #[test]
    fn a_forget_reaches_a_quote_carried_into_a_suggestion_not_shown() {
        let mut h = Home::new();
        h.reviewed("g:0", &[("Plain names.", "name things plainly")]);
        h.pass(&|_| answer(&[("", "Name things plainly.")]));
        let x = h.s.said("s1", "repo", 1_000, "no spaces to indent, ever");
        let read = Read {
            spans: vec![(x, x)],
            ops: vec![],
        };
        h.reviewed_reading("g:1", &[("Tabs.", "no spaces to indent")], read);
        h.age(4 * 60);
        h.pass(&|_| answer(&[("", "Indent with tabs.")]));
        let long = "y".repeat(QUOTE);
        for (seen, request) in [(2, "g:2"), (3, "g:3")] {
            h.reviewed(request, &[("Tabs again.", long.as_str())]);
            h.age(4 * 60);
            let (_, sent) = h.pass_within(20, &|_| answer(&[("", "Indent with tabs.")]));
            assert!(!sent[0].contains("- s2 "), "left out: {}", sent[0]);
            let kept = h.suggestions();
            assert_eq!(
                (kept.len(), kept[1].seen),
                (2, seen),
                "joined, though not shown"
            );
        }
        let carried = "no spaces to indent";
        let kept = h.suggestions();
        assert!(kept[1].quotes.iter().any(|q| q.text == carried));
        assert!(h.page().to_string().contains(carried));
        let device = h.s.raw.device().to_owned();
        let h = h.closed(|home| {
            let target = crate::forget::Target::Record { device, seq: x };
            crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            crate::forget::purge(home).unwrap();
        });
        let page = h.page().to_string();
        assert!(!page.contains(carried), "{page}");
        assert!(
            page.contains("Name things plainly."),
            "made before the quote was kept: {page}"
        );
    }
    /// #739: while a decision is recorded but not applied, the suggestion takes no other action,
    /// so an approve is not sent twice.
    #[test]
    fn a_decision_not_applied_yet_is_not_made_again() {
        let mut h = Home::new();
        suggested(&mut h);
        let op = MyWorkOp {
            job: RULE.into(),
            about: "s1".into(),
            entry: String::new(),
            ts: crate::db::now_ms(),
            read: Read::default(),
            removed: vec![],
            fields: BTreeMap::from([("action".to_owned(), json!("reject"))]),
            skipped: false,
            forgotten: None,
        };
        let op = serde_json::to_value(op).unwrap();
        h.s.raw.append_ops(&[(OpKind::MyWork, op)]).unwrap();
        let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
        let refused = h.act(approve.clone()).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_pending"));
        assert_eq!(
            h.yours(),
            Vec::<(String, bool)>::new(),
            "no preference made"
        );
        h.s.run();
        let refused = h.act(approve).unwrap_err();
        assert_eq!(
            (refused.status, refused.code),
            (409, "rule_decided"),
            "applied, it holds"
        );
    }

    /// Decision 81: the prompt writes in the language set, or each conversation's own.
    #[test]
    fn its_prompt_writes_in_the_language_set() {
        for language in ["English", "conversation"] {
            let asked = prompt(language, "## The new preferences\n");
            assert!(
                asked.contains(&crate::config::write_in(language)),
                "{language}: {asked}"
            );
        }
    }
