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

    fn reviewed_reading(&mut self, request: &str, preferences: &[(&str, &str)], read: Read) -> i64 {
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
        crate::settings::claims::rule(
            self.s.home.path(),
            &std::sync::Mutex::new(()),
            &|| true,
            &body,
        )
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
        sent[0].contains("- s1 (seen in 1 request, not decided): Write the tests before the code."),
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
    let toml = "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n";
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

/// #1131 items 1 and 6: an approved preference through a rescan, a purge and a rebuild. A rule
/// that masks part of the directive, beside one that finds the decision's action and the uid it
/// names, leaves the decision naming the preference the purge writes again from the masked text,
/// with its uid: My Rules lists it as approved, and its undo applies.
#[test]
fn an_approval_keeps_its_decision_through_a_rescan_a_purge_and_a_rebuild() {
    let mut h = Home::new();
    suggested(&mut h);
    let got = h
        .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
        .unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let uid = got["uid"].as_str().unwrap().to_owned();
    let rules = format!(
        "[redaction]\nextra_rules = [{{ id = \"tests\", regex = 'tests' }}, \
         {{ id = \"words\", regex = 'approve|{uid}' }}]\n"
    );
    std::fs::write(h.s.home.path().join("config.toml"), rules).unwrap();
    crate::worker::run_once(h.s.home.path()).unwrap();
    let mut purged = None;
    let h = h.closed(|home| {
        purged = crate::forget::purge(home).unwrap();
        crate::worker::rebuild(home).unwrap();
    });
    assert_eq!(purged.map(|p| p.preferences), Some(1), "written again");
    h.s.run();
    let page = h.page();
    let rule = &page["yours"][0]["rules"][0];
    assert_eq!(
        (&rule["text"], &rule["suggestion"], &rule["uid"]),
        (
            &json!("Write the ***** before the code."),
            &json!("s1"),
            &json!(uid)
        )
    );
    let got = h
        .act(json!({"suggestion": "s1", "action": "undo"}))
        .unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    assert_eq!(h.waiting(), ["s1"]);
}

/// The seqs of `h`'s directives, events of kind `directive`, in order.
fn directive_seqs(h: &Home) -> Vec<i64> {
    let c = rusqlite::Connection::open(h.s.home.path().join("raw.db")).unwrap();
    (c.prepare(
        "SELECT seq FROM records WHERE device = ?1 AND type = 'event' AND kind = 'directive'
         ORDER BY seq",
    )
    .unwrap())
    .query_map([h.s.raw.device()], |r| r.get(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Whether `h`'s raw.db holds a row of any type at `seq` of its device.
fn row_at(h: &Home, seq: i64) -> bool {
    let c = rusqlite::Connection::open(h.s.home.path().join("raw.db")).unwrap();
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM records WHERE device = ?1 AND seq = ?2)",
        rusqlite::params![h.s.raw.device(), seq],
        |r| r.get(0),
    )
    .unwrap()
}

/// The status knowledge.db's `active` gives `uid`, if it holds it.
fn active_status(h: &Home, uid: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    let k = crate::knowledge::open(h.s.home.path()).unwrap();
    k.query_row("SELECT status FROM active WHERE uid = ?1", [uid], |r| {
        r.get(0)
    })
    .optional()
    .unwrap()
}

/// The ops of `h`'s device past op seq `after`, as (type, body).
fn ops_past(h: &Home, after: i64) -> Vec<(String, Value)> {
    let c = rusqlite::Connection::open(h.s.home.path().join("raw.db")).unwrap();
    (c.prepare("SELECT type, body FROM ops WHERE device = ?1 AND op_seq > ?2 ORDER BY op_seq")
        .unwrap())
    .query_map(rusqlite::params![h.s.raw.device(), after], |r| {
        Ok((
            r.get(0)?,
            serde_json::from_str(&r.get::<_, String>(1)?).unwrap(),
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// The records segment of `home`'s backups that holds seq `seq`.
fn segment_holding(home: &std::path::Path, seq: i64) -> std::path::PathBuf {
    let dir = home.join("backups");
    (std::fs::read_dir(&dir).unwrap())
        .map(|e| e.unwrap().path())
        .find(|path| {
            (path.file_name())
                .and_then(crate::backup::segment_name)
                .is_some_and(|(ops, _, first, last)| !ops && first <= seq && seq <= last)
        })
        .unwrap()
}

/// The seal a backup segment is checked by: its sha256, beside it.
fn sealed(segment: &std::path::Path) -> std::path::PathBuf {
    let mut path = segment.as_os_str().to_owned();
    path.push(".sha256");
    path.into()
}

/// raw.db replaced with bytes that are not a database, as `run_once` restores it from the
/// backups.
fn damage_raw(home: &std::path::Path) {
    std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.join(f));
    }
}

const APPROVE: &str = r#"{"suggestion": "s1", "action": "approve", "scope": "global"}"#;
const UNDO: &str = r#"{"suggestion": "s1", "action": "undo"}"#;

/// #1131 item 8: an approval after a restore takes a seq whose preference uid is not forgotten. A
/// preference's uid derives from its directive's seq; a restore to a point before an approval
/// frees that seq and keeps the forget of the preference, so approving the same suggestion again
/// derived the forgotten uid, refused at every try. It takes the next seq, and the seq passed
/// over holds no row.
#[test]
fn an_approval_after_a_restore_passes_over_the_seq_of_a_forgotten_preference() {
    let mut h = Home::new();
    h.s.raw
        .append(&crate::raw::test_event("before the point"))
        .unwrap();
    suggested(&mut h);
    crate::backup::export(h.s.home.path()).unwrap();
    let point = (crate::backup::points(&h.s.home.path().join("backups")).unwrap())
        .pop()
        .unwrap();
    let got = h.act(serde_json::from_str(APPROVE).unwrap()).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let first = got["uid"].as_str().unwrap().to_owned();
    let [at] = directive_seqs(&h)[..] else {
        panic!("one directive");
    };
    let forgotten = first.clone();
    let h = h.closed(|home| {
        let target = crate::forget::Target::Uid { uid: forgotten };
        crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
        crate::worker::restore_to(home, &point).unwrap();
    });
    h.s.run();
    assert_eq!(h.waiting(), ["s1"], "the restore took the approval back");
    assert!(!row_at(&h, at), "the restore freed the directive's seq");
    let got = h.act(serde_json::from_str(APPROVE).unwrap()).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let second = got["uid"].as_str().unwrap();
    assert_ne!(second, first);
    assert!(!h.s.raw.forgotten(second).unwrap());
    assert_eq!(directive_seqs(&h), [at + 1]);
    assert!(!row_at(&h, at), "the seq passed over holds no row");
    assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
}

/// `h` with an approved suggestion `s1` whose directive is alone in a middle segment of its
/// backups, which is then damaged with raw.db, so that `run_once` restores the rest: the
/// approval's ops come through, its directive does not, and knowledge.db holds no preference.
/// The preference's uid, the directive's seq, its segment, and that segment's bytes and seal's.
fn lost_directive() -> (Home, String, i64, std::path::PathBuf, [Vec<u8>; 2]) {
    let mut h = Home::new();
    h.s.raw.append(&crate::raw::test_event("before")).unwrap();
    suggested(&mut h);
    crate::backup::export(h.s.home.path()).unwrap();
    let got = h.act(serde_json::from_str(APPROVE).unwrap()).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let uid = got["uid"].as_str().unwrap().to_owned();
    let [at] = directive_seqs(&h)[..] else {
        panic!("one directive");
    };
    crate::backup::export(h.s.home.path()).unwrap();
    h.s.raw.append(&crate::raw::test_event("after")).unwrap();
    crate::backup::export(h.s.home.path()).unwrap();
    let segment = segment_holding(h.s.home.path(), at);
    let saved = [&segment, &sealed(&segment)].map(|path| std::fs::read(path).unwrap());
    let h = h.closed(|home| {
        std::fs::write(&segment, b"damaged").unwrap();
        damage_raw(home);
        crate::worker::run_once(home).unwrap();
    });
    h.s.run();
    assert!(!row_at(&h, at), "the restore lost the directive");
    assert!(directive_seqs(&h).is_empty());
    assert_eq!(
        h.waiting(),
        Vec::<String>::new(),
        "the approval came through"
    );
    assert_eq!(active_status(&h, &uid), None, "no preference derived");
    (h, uid, at, segment, saved)
}

/// #1131 item 9: an undo of an approval whose directive a restore lost for good retracts the
/// preference where the directive stood, with its decision: the claims consumer keeps the
/// retraction though it derived no preference, and a later restore that brings the directive
/// back derives the preference retracted. Main waited for an anchor that never came (409).
#[test]
fn an_undo_of_an_approval_whose_directive_a_restore_lost_retracts_it_there() {
    let (h, uid, at, segment, saved) = lost_directive();
    let (_, before) = tops(&h);
    let got = h.act(serde_json::from_str(UNDO).unwrap()).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    assert_eq!(h.waiting(), ["s1"]);
    let ops = ops_past(&h, before);
    let device = h.s.raw.device().to_owned();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(
        (ops[0].0.as_str(), &ops[0].1),
        (
            "correction",
            &json!({"uid": uid, "anchor": {"device": device, "seq": at},
                "status": "retracted", "body": null})
        )
    );
    assert_eq!(
        (ops[1].0.as_str(), &ops[1].1["about"], &ops[1].1["fields"]),
        ("my_work", &json!("s1"), &json!({"action": "undo"}))
    );
    // The directive back: the undo backed up, the segment's sound copy and its seal returned, and
    // a restore.
    let h = h.closed(|home| {
        crate::backup::export(home).unwrap();
        std::fs::write(&segment, &saved[0]).unwrap();
        std::fs::write(sealed(&segment), &saved[1]).unwrap();
        damage_raw(home);
        crate::worker::run_once(home).unwrap();
    });
    h.s.run();
    assert!(row_at(&h, at), "the directive is back");
    assert_eq!(active_status(&h, &uid).as_deref(), Some("retracted"));
    assert_eq!(h.waiting(), ["s1"]);
}

/// #1131 item 9: an undo whose approval a second viewer undid and approved again since its read
/// refuses, writing nothing: its decision would undo the new approval, whose preference would stay.
#[test]
fn an_undo_of_a_lost_approval_refuses_after_another_undo_and_approval() {
    let (h, _uid, _at, _segment, _saved) = lost_directive();
    let home = h.s.home.path().to_owned();
    // The op top once the second viewer is done: the first viewer's refusal writes nothing past
    // it (CodeRabbit on #1138).
    let top = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = top.clone();
    let device = h.s.raw.device().to_owned();
    crate::claims::LOST_UNDO.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            // Another writer's undo and approval: the route would wait for this one's config.lock.
            decided_elsewhere(&home, "s1", json!({"action": "undo"}));
            let settings = crate::capture::Settings::load(&home).unwrap();
            let (c, quote) = crate::claims::directive_text(&settings, RULE_TEXT, None).unwrap();
            let mut raw = crate::raw::open(&home).unwrap();
            crate::claims::approve_append(&mut raw, &settings, (&c, &quote), "s1", None).unwrap();
            drop(raw);
            let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
            seen.set(
                raw.query_row(
                    "SELECT MAX(op_seq) FROM ops WHERE device = ?1",
                    [&device],
                    |r| r.get(0),
                )
                .unwrap(),
            );
        }))
    });
    let refused = h.act(serde_json::from_str(UNDO).unwrap()).unwrap_err();
    assert_eq!((refused.status, refused.code), (409, "rule_moved"));
    h.s.run();
    assert!(top.get() > 0);
    assert_eq!(
        ops_past(&h, top.get()),
        Vec::<(String, Value)>::new(),
        "nothing written"
    );
    assert_eq!(h.waiting(), Vec::<String>::new(), "the new approval stands");
    assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
}

/// A rule decision of suggestion `id` holding `fields`, appended on a raw handle of its own, as
/// another writer appends one (it waits for no config.lock and no consumer).
fn decided_elsewhere(home: &std::path::Path, id: &str, fields: Value) {
    let ops = crate::my_work_ops::bodies(RULE, vec![(id.to_owned(), fields)]).unwrap();
    crate::raw::open(home).unwrap().append_ops(&ops).unwrap();
}

/// How many records, ledger rows and ops raw.db holds.
fn stored(home: &std::path::Path) -> (i64, i64, i64) {
    let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    raw.query_row(
        "SELECT (SELECT COUNT(*) FROM records), (SELECT COUNT(*) FROM ledger),
                (SELECT COUNT(*) FROM ops)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

/// The main page's rule action `body` on `home`, its token the page's.
fn acted(
    home: &std::path::Path,
    body: Value,
) -> std::result::Result<Value, crate::settings::Refusal> {
    let body = serde_json::to_vec(&body).unwrap();
    crate::settings::claims::rule(home, &std::sync::Mutex::new(()), &|| true, &body)
}

/// `f` as the main page's rule action's `CHECKED` hook: run once it read and prepared its write.
fn on_checked(f: impl FnOnce() + 'static) {
    crate::settings::claims::CHECKED.with(|hook| *hook.borrow_mut() = Some(Box::new(f)));
}

/// An event of `session`'s, a prompt in `repo`, appended on a raw handle of its own.
fn recorded_in(home: &std::path::Path, session: &str, repo: &str) {
    let e = crate::raw::Event {
        kind: "prompt".into(),
        session: session.into(),
        repo: Some(repo.into()),
        ts: crate::db::now_ms(),
        ..crate::raw::test_event(&json!({"prompt": "Hello."}).to_string())
    };
    crate::raw::open(home).unwrap().append(&e).unwrap();
}

/// #1131 items 3 and 7: the main page's rule action holds, from before its read until its write,
/// config.lock (a save of the redaction rules waits), the dispatch hold (a purge, which rewrites ops
/// in place, waits) and raw.db's shared hold (a restore cannot swap the store it writes to).
#[test]
fn a_rule_action_holds_its_locks_until_its_write() {
    let mut h = Home::new();
    suggested(&mut h);
    let held = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = held.clone();
    let h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || {
            let wait = std::time::Duration::ZERO;
            let config = crate::settings::config_lock_within(&at, wait).unwrap();
            let dispatch = crate::dispatch::exclusive_within(&at, wait);
            let lock = std::fs::OpenOptions::new()
                .write(true)
                .open(at.join("raw.lock"));
            let swap = lock.unwrap().try_lock();
            seen.borrow_mut()
                .extend([config.is_none(), dispatch.is_err(), swap.is_err()]);
        });
        let got = acted(
            home,
            json!({"suggestion": "s1", "action": "approve", "scope": "global"}),
        );
        assert_eq!(got.unwrap()["state"], "applied");
    });
    assert_eq!(
        *held.borrow(),
        [true, true, true],
        "config.lock, dispatch, raw.lock"
    );
    assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
}

/// #1131 item 4: a decision of the suggestion recorded since the main page's rule action read it
/// refuses the action's write as pending, with nothing written: an approval, a rejection, an
/// approval's undo, and the undo of an approval whose preference is forgotten.
#[test]
fn a_decision_since_its_read_refuses_a_rule_actions_write() {
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let undo = json!({"suggestion": "s1", "action": "undo"});
    for (body, approved_first, forgotten) in [
        (approve.clone(), false, false),
        (
            json!({"suggestion": "s1", "action": "reject"}),
            false,
            false,
        ),
        (undo.clone(), true, false),
        (undo, true, true),
    ] {
        let mut h = Home::new();
        suggested(&mut h);
        if approved_first {
            let got = h.act(approve.clone()).unwrap();
            if forgotten {
                let uid = got["uid"].as_str().unwrap().to_owned();
                let target = crate::forget::Target::Uid { uid };
                let home = h.s.home.path();
                crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            }
            h.s.run();
        }
        let _h = h.closed(|home| {
            let at = home.to_owned();
            on_checked(move || decided_elsewhere(&at, "s1", json!({"action": "reject"})));
            let before = stored(home);
            let refused = acted(home, body.clone()).unwrap_err();
            assert_eq!(
                (refused.status, refused.code),
                (409, "rule_moved"),
                "{body}"
            );
            let after = stored(home);
            assert_eq!(
                after,
                (before.0, before.1, before.2 + 1),
                "the other decision alone: {body}"
            );
        });
    }
}

/// #1131 item 7: an exclusion since the main page's approval read its suggestion, or a first event
/// of a session in a repository excluded at its read, refuses the approval as pending, with nothing
/// written: the suggestion's source may be excluded with it.
#[test]
fn an_exclusion_since_its_read_refuses_a_rule_actions_write() {
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let mut h = Home::new();
    suggested(&mut h);
    let h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || {
            let op = json!({"repo": "github.com/o/elsewhere", "undo": false});
            let ops = [(OpKind::Exclusion, op)];
            crate::raw::open(&at).unwrap().append_ops(&ops).unwrap();
        });
        let before = stored(home);
        let refused = acted(home, approve.clone()).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_moved"));
        assert_eq!(
            stored(home),
            (before.0, before.1, before.2 + 1),
            "the exclusion alone"
        );
    });
    drop(h);
    let mut h = Home::new();
    suggested(&mut h);
    h.s.raw.exclude("github.com/o/secret", false).unwrap();
    h.s.run();
    let _h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || recorded_in(&at, "a newcomer", "github.com/o/secret"));
        let before = stored(home);
        let refused = acted(home, approve.clone()).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_moved"));
        assert_eq!(
            stored(home),
            (before.0 + 1, before.1, before.2),
            "the event alone"
        );
    });
}

/// #1131 item 7: a forget between the main page's read and its write cannot register, since the
/// dispatch hold keeps it out (`a_rule_action_holds_its_locks_until_its_write`); a tombstone that
/// lands there anyway, as one written without that hold would, refuses the approval as pending,
/// with nothing written: the suggestion's source may be what it hides.
#[test]
fn a_tombstone_since_its_read_refuses_a_rule_actions_write() {
    let mut h = Home::new();
    suggested(&mut h);
    let seq = h.s.said(
        "learned",
        "github.com/o/open",
        crate::db::now_ms(),
        "Hello.",
    );
    h.s.run();
    let device = h.s.raw.device().to_owned();
    let _h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || {
            let target = crate::raw::Target::Record { device, seq };
            crate::raw::open(&at)
                .unwrap()
                .append_tombstone(target)
                .unwrap();
        });
        let before = stored(home);
        let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
        let refused = acted(home, approve).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_moved"));
        assert_eq!(
            stored(home),
            (before.0 + 1, before.1, before.2),
            "the tombstone alone"
        );
    });
}

/// #1131 C1 (Codex on its plan): the main page's rule action answers as it did before its locks
/// when the settings cannot be read: a suggestion it does not show is 404, a rejection 503
/// `rule_unavailable`, an approval and an approval's undo 503 `claim_unavailable`, each with
/// nothing written.
#[test]
fn a_rule_action_whose_settings_cannot_be_read_answers_as_before() {
    let mut h = Home::new();
    suggested(&mut h);
    let home = h.s.home.path().to_owned();
    let config = home.join("config.toml");
    let kept = std::fs::read(&config).ok();
    let broken = || std::fs::write(&config, "invalid = [").unwrap();
    let mended = || match &kept {
        Some(kept) => std::fs::write(&config, kept).unwrap(),
        None => std::fs::remove_file(&config).unwrap(),
    };
    let answered = |body: Value| {
        let before = stored(&home);
        let refused = acted(&home, body.clone()).unwrap_err();
        assert_eq!(stored(&home), before, "nothing written: {body}");
        (refused.status, refused.code)
    };
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    broken();
    let missing = json!({"suggestion": "s9", "action": "reject"});
    assert_eq!(answered(missing), (404, "rule_not_found"));
    let reject = json!({"suggestion": "s1", "action": "reject"});
    assert_eq!(answered(reject), (503, "rule_unavailable"));
    assert_eq!(answered(approve.clone()), (503, "claim_unavailable"));
    mended();
    acted(&home, approve).unwrap();
    h.s.run();
    broken();
    let undo = json!({"suggestion": "s1", "action": "undo"});
    assert_eq!(answered(undo), (503, "claim_unavailable"));
    mended();
}

/// #1131 item 7: a session excluded already at the main page's fence was the read's to judge: its
/// later event in the excluded repository leaves the approval be.
#[test]
fn an_event_of_a_session_excluded_at_its_fence_leaves_a_rule_action_be() {
    let mut h = Home::new();
    suggested(&mut h);
    h.s.raw.exclude("github.com/o/secret", false).unwrap();
    h.s.said(
        "old-timer",
        "github.com/o/secret",
        crate::db::now_ms(),
        "Hello.",
    );
    h.s.run();
    let h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || recorded_in(&at, "old-timer", "github.com/o/secret"));
        let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
        assert_eq!(acted(home, approve).unwrap()["state"], "applied");
    });
    assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
}

/// #1131 items 4 and 7: an undo's fence names its preference: a correction of the preference since
/// the main page's undo read it refuses the undo, with nothing written but the correction.
#[test]
fn a_correction_of_its_preference_since_its_read_refuses_an_undo() {
    let mut h = Home::new();
    suggested(&mut h);
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let uid = h.act(approve).unwrap()["uid"].as_str().unwrap().to_owned();
    h.s.run();
    let _h = h.closed(|home| {
        let at = home.to_owned();
        on_checked(move || {
            let ops = [(OpKind::Correction, json!({"uid": uid, "muted": true}))];
            crate::raw::open(&at).unwrap().append_ops(&ops).unwrap();
        });
        let before = stored(home);
        let undo = json!({"suggestion": "s1", "action": "undo"});
        let refused = acted(home, undo).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_moved"));
        assert_eq!(
            stored(home),
            (before.0, before.1, before.2 + 1),
            "the correction alone"
        );
    });
}

/// #1131 items 7 and 10: a session's first event since the main page's read under a label first
/// recorded then, which a repository never recorded may be (a mask in it), refuses its approval,
/// with nothing written but the event.
#[test]
fn a_first_event_under_a_masked_label_since_its_read_refuses_a_rule_action() {
    let mut h = Home::new();
    suggested(&mut h);
    (h.s.raw)
        .exclude_capture(("repo", "github.com/x/private-aaa"), false)
        .unwrap();
    h.s.run();
    let _h = h.closed(|home| {
        let at = home.to_owned();
        let masked = format!("github.com/x/private-{}", crate::redact::MASK);
        on_checked(move || recorded_in(&at, "learned", &masked));
        let before = stored(home);
        let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
        let refused = acted(home, approve).unwrap_err();
        assert_eq!((refused.status, refused.code), (409, "rule_moved"));
        assert_eq!(
            stored(home),
            (before.0 + 1, before.1, before.2),
            "the event alone"
        );
    });
}

/// #1131 C1: the main page's rule action lets go of its locks before its receipt waits, which can
/// run the worker: config.lock, an exclusive dispatch hold and an exclusive raw.lock can each be
/// taken while the receipt proves the approval.
#[test]
fn a_rule_actions_receipt_holds_none_of_its_locks() {
    let mut h = Home::new();
    suggested(&mut h);
    let free = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = free.clone();
    let _h = h.closed(|home| {
        let at = home.to_owned();
        crate::claims::PROVING.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let wait = std::time::Duration::ZERO;
                let config = crate::settings::config_lock_within(&at, wait).unwrap();
                let dispatch = crate::dispatch::exclusive_within(&at, wait);
                let lock = std::fs::OpenOptions::new()
                    .write(true)
                    .open(at.join("raw.lock"));
                let swap = lock.unwrap().try_lock();
                seen.borrow_mut()
                    .extend([config.is_some(), dispatch.is_ok(), swap.is_ok()]);
            }))
        });
        let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
        assert_eq!(acted(home, approve).unwrap()["state"], "applied");
    });
    assert_eq!(
        *free.borrow(),
        [true, true, true],
        "config.lock, dispatch, raw.lock"
    );
}

/// #1131 item 9: the undo of a lost approval checks itself that no decision of its suggestion is
/// newer than the approval it undoes: a fence taken after another writer's undo and approval does
/// not see them, and the append still refuses, making and writing nothing.
#[test]
fn a_lost_approvals_undo_refuses_a_newer_decision_its_fence_did_not_see() {
    let (mut h, uid, _at, _segment, _saved) = lost_directive();
    let home = h.s.home.path().to_owned();
    let raw = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    let decision: i64 = raw
        .query_row(
            "SELECT MAX(op_seq) FROM ops WHERE type = 'my_work'
               AND json_extract(body, '$.job') = ?1 AND json_extract(body, '$.about') = 's1'",
            [RULE],
            |r| r.get(0),
        )
        .unwrap();
    drop(raw);
    decided_elsewhere(&home, "s1", json!({"action": "undo"}));
    let settings = crate::capture::Settings::load(&home).unwrap();
    let (c, quote) = crate::claims::directive_text(&settings, RULE_TEXT, None).unwrap();
    crate::claims::approve_append(&mut h.s.raw, &settings, (&c, &quote), "s1", None).unwrap();
    let fence = h.s.raw.rule_fence().unwrap();
    let subject = crate::raw::Subject {
        uid: Some(uid.as_str()),
        suggestion: Some("s1"),
        sources: None,
    };
    let made = std::cell::Cell::new(false);
    let ops = |_: &str, _: i64| {
        made.set(true);
        Ok(Vec::new())
    };
    let undone = (decision, "s1", uid.as_str());
    let got = (h.s.raw)
        .append_undo_of_lost_approval(undone, ops, (&fence, &subject))
        .unwrap();
    assert_eq!(got, None, "nothing written");
    assert!(!made.get(), "no ops made");
}

/// #1131 items 7 and 9: the undo of an approval whose directive a restore lost is checked against
/// the rule fence too: a first event of a session in a repository excluded at its read refuses it
/// as pending, with nothing written (Codex on the plan of #1131 C1).
#[test]
fn a_lost_approvals_undo_is_refused_by_an_event_since_its_read() {
    let (mut h, _uid, _at, _segment, _saved) = lost_directive();
    h.s.raw.exclude("github.com/o/secret", false).unwrap();
    h.s.run();
    let home = h.s.home.path().to_owned();
    crate::claims::LOST_UNDO.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            recorded_in(&home, "a newcomer", "github.com/o/secret");
        }))
    });
    let before = stored(h.s.home.path());
    let refused = h.act(serde_json::from_str(UNDO).unwrap()).unwrap_err();
    assert_eq!((refused.status, refused.code), (409, "rule_moved"));
    let after = stored(h.s.home.path());
    assert_eq!(after, (before.0 + 1, before.1, before.2), "the event alone");
}

/// #1131 C1: the main page's rule action asks for its token again once it holds config.lock: a
/// token replaced while it waited for the lock is refused, with nothing written.
#[test]
fn a_rule_action_whose_token_changed_while_it_waited_is_refused() {
    let mut h = Home::new();
    suggested(&mut h);
    let home = h.s.home.path().to_owned();
    let lock = crate::settings::config_lock_within(&home, std::time::Duration::ZERO);
    let lock = std::rc::Rc::new(std::cell::RefCell::new(lock.unwrap()));
    let revoked = std::rc::Rc::new(std::cell::Cell::new(false));
    let (release, revoke) = (lock.clone(), revoked.clone());
    crate::settings::CONFIG_BLOCKED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            revoke.set(true);
            release.borrow_mut().take();
        }))
    });
    let before = stored(&home);
    let body = serde_json::to_vec(&json!({"suggestion": "s1", "action": "reject"})).unwrap();
    let authorized = || !revoked.get();
    let saving = std::sync::Mutex::new(());
    let refused = crate::settings::claims::rule(&home, &saving, &authorized, &body).unwrap_err();
    assert_eq!(refused.status, 401);
    assert!(revoked.get(), "the lock was waited for");
    assert_eq!(stored(&home), before, "nothing written");
}

/// #1131 item 9: an undo with no anchor in knowledge.db still waits where the directive is not gone
/// for good: one masked by a rule before its purge, and one whose record a forget took before
/// its purge, both still a row; one at or past the device's top, whose seq the next record
/// takes; and an approval whose preference is in another batch, as approvals were written
/// before #1130. Each refuses as pending, and writes nothing.
#[test]
fn an_undo_with_no_anchor_waits_unless_the_directive_is_gone_for_good() {
    let cases = ["masked", "forgotten record", "at the top", "another batch"];
    for case in cases {
        let mut h = Home::new();
        h.s.raw.append(&crate::raw::test_event("before")).unwrap();
        suggested(&mut h);
        let uid = if case == "another batch" {
            let home = h.s.home.path().to_owned();
            let Ok(crate::claims::OwnerReceipt::Applied { uid }) =
                crate::claims::pref_add_recorded(&home, RULE_TEXT, None)
            else {
                panic!("the preference is not applied");
            };
            assert!(record(&home, "s1", json!({"action": "approve", "claim": uid})).unwrap());
            uid
        } else {
            let got = h.act(serde_json::from_str(APPROVE).unwrap()).unwrap();
            got["uid"].as_str().unwrap().to_owned()
        };
        let [at] = directive_seqs(&h)[..] else {
            panic!("{case}: one directive");
        };
        if case != "at the top" {
            h.s.raw.append(&crate::raw::test_event("after")).unwrap();
        }
        let device = h.s.raw.device().to_owned();
        let h = h.closed(|home| {
            match case {
                "masked" => {
                    std::fs::write(
                        home.join("config.toml"),
                        "[redaction]\nextra_rules = [{ id = \"tests\", regex = 'tests' }]\n",
                    )
                    .unwrap();
                }
                "forgotten record" => {
                    let target = crate::forget::Target::Record { device, seq: at };
                    crate::forget::start(home, &crate::forget::preview(home, target).unwrap())
                        .unwrap();
                }
                _ => {
                    rusqlite::Connection::open(home.join("raw.db"))
                        .unwrap()
                        .execute(
                            "DELETE FROM records WHERE device = ?1 AND seq = ?2",
                            rusqlite::params![device, at],
                        )
                        .unwrap();
                }
            }
            // knowledge.db made again from raw.db as it now stands: no preference derived.
            crate::worker::rebuild(home).unwrap();
        });
        h.s.run();
        assert_eq!(active_status(&h, &uid), None, "{case}: no anchor");
        let (_, before) = tops(&h);
        let refused = h.act(serde_json::from_str(UNDO).unwrap()).unwrap_err();
        assert_eq!(
            (refused.status, refused.code),
            (409, "rule_pending"),
            "{case}"
        );
        assert_eq!(
            ops_past(&h, before),
            Vec::<(String, Value)>::new(),
            "{case}"
        );
    }
}

/// The tops of `h`'s raw.db for its device: its last record's seq and its last op's.
fn tops(h: &Home) -> (i64, i64) {
    let c = rusqlite::Connection::open(h.s.home.path().join("raw.db")).unwrap();
    c.query_row(
        "SELECT (SELECT COALESCE(MAX(seq), 0) FROM records WHERE device = ?1),
                (SELECT COALESCE(MAX(op_seq), 0) FROM ops WHERE device = ?1)",
        [h.s.raw.device()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

/// What an approval recorded in `h` past `before` (`tops`): each directive's columns but its
/// device, seq and time; and each preference op `pref add` writes and each rule decision, as
/// `(type, batch, body)`, the device, seq and time they hold and the preference's uid named apart.
fn approval_since(h: &Home, (records, ops): (i64, i64)) -> (Vec<Value>, Vec<(String, i64, Value)>) {
    let device = h.s.raw.device().to_owned();
    let c = rusqlite::Connection::open(h.s.home.path().join("raw.db")).unwrap();
    let mut s = c
        .prepare(
            "SELECT type, kind, agent, session, repo, branch, head, gitdir, cwd, source, enc,
                    CAST(body AS TEXT)
             FROM records WHERE device = ?1 AND seq > ?2 AND kind = 'directive' ORDER BY seq",
        )
        .unwrap();
    let events = (s.query_map(rusqlite::params![device, records], |r| {
        let column = |i| r.get::<_, Option<String>>(i);
        Ok(json!([
            column(0)?,
            column(1)?,
            column(2)?,
            column(3)?,
            column(4)?,
            column(5)?,
            column(6)?,
            column(7)?,
            column(8)?,
            column(9)?,
            column(10)?,
            column(11)?
        ]))
    }))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap();
    let mut s = c
        .prepare(
            "SELECT type, batch, body FROM ops WHERE device = ?1 AND op_seq > ?2 ORDER BY op_seq",
        )
        .unwrap();
    let all = (s.query_map(rusqlite::params![device, ops], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    }))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap();
    let mut uid = None;
    let mut kept = Vec::new();
    for (kind, batch, body) in all {
        let mut v: Value = serde_json::from_str(&body).unwrap();
        if kind == "claim" && v["recipe"] == crate::claims::PREF_RECIPE {
            uid = crate::claims::op_uid(&v);
            for e in v["evidence"].as_array_mut().unwrap() {
                e["device"] = json!("D");
                e["seq"] = json!(0);
            }
        } else if kind == "my_work" && v["job"] == "rule" {
            assert_eq!(v["fields"]["claim"].as_str(), uid.as_deref(), "{v}");
            v["ts"] = json!(0);
            v["fields"]["claim"] = json!("U");
        } else {
            continue;
        }
        kept.push((kind, batch, v));
    }
    (events, kept)
}

/// #1083's R2b (PR 1): an approval records what `pref add` and its decision record apart (the
/// directive, its preference, the decision naming it), their device, seqs and times aside, and
/// holds its preference and its decision in one batch.
#[test]
fn an_approval_records_what_pref_add_and_its_decision_record_apart_in_one_batch() {
    let mut a = Home::new();
    suggested(&mut a);
    let before = tops(&a);
    let got = (a.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let (events, ops) = approval_since(&a, before);
    let mut b = Home::new();
    suggested(&mut b);
    let before = tops(&b);
    let home = b.s.home.path();
    let Ok(crate::claims::OwnerReceipt::Applied { uid }) =
        crate::claims::pref_add_recorded(home, RULE_TEXT, None)
    else {
        panic!("the preference is not applied");
    };
    assert!(record(home, "s1", json!({"action": "approve", "claim": uid})).unwrap());
    let (events_apart, ops_apart) = approval_since(&b, before);
    assert_eq!(events.len(), 1);
    assert_eq!(events, events_apart);
    let bodies = |ops: &[(String, i64, Value)]| -> Vec<(String, Value)> {
        (ops.iter())
            .map(|(t, _, b)| (t.clone(), b.clone()))
            .collect()
    };
    assert_eq!(bodies(&ops), bodies(&ops_apart));
    assert_eq!(ops.len(), 2);
    assert_eq!(
        ops[0].1, ops[1].1,
        "the preference and its decision in one batch"
    );
}

/// #1083's R2b (PR 2): the work-view's approval of a suggestion records what the main page's plain
/// Approve records (the directive, its preference, the decision naming it), their device, seqs and
/// times aside, its preference and its decision in one batch.
#[test]
fn the_work_views_approval_records_what_the_main_pages_approve_records() {
    let mut a = Home::new();
    suggested(&mut a);
    let before = tops(&a);
    let got = (a.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let (events, ops) = approval_since(&a, before);
    let mut b = Home::new();
    suggested(&mut b);
    let before = tops(&b);
    let home = b.s.home.path();
    let settings = crate::capture::Settings::load(home).unwrap();
    let (raw, k) = crate::search::b::stores(home).unwrap().unwrap();
    let found = crate::work_view::my_rules_on(home, crate::db::now_ms(), &settings, &raw, &k);
    let row = (found.unwrap().rows.into_iter()).find(|r| r.of["suggestion"] == "s1");
    let row = row.unwrap();
    drop((raw, k));
    let saving = std::sync::Mutex::new(());
    let take = || saving.lock().ok();
    let at = (row.reference.as_str(), row.version.as_str());
    let what = crate::work_view::Decision::Approve;
    let got = crate::work_view::decide(home, at, what, &take, &|| true).unwrap();
    assert!(
        matches!(got, crate::work_view::Switched::Applied(Some(_))),
        "{got:?}"
    );
    let (events_pages, ops_pages) = approval_since(&b, before);
    assert_eq!(events.len(), 1);
    assert_eq!(events, events_pages);
    let bodies = |ops: &[(String, i64, Value)]| -> Vec<(String, Value)> {
        (ops.iter())
            .map(|(t, _, b)| (t.clone(), b.clone()))
            .collect()
    };
    assert_eq!(bodies(&ops), bodies(&ops_pages));
    assert_eq!(ops_pages.len(), 2);
    assert_eq!(ops_pages[0].1, ops_pages[1].1, "one batch");
}

/// #1083's R2b (PR 1): an approval whose ops are refused once its event is checked, as a forget
/// committed in between refuses them, records nothing: no directive is left without its
/// preference, and the suggestion waits as it did.
#[test]
fn an_approval_refused_after_its_event_is_checked_leaves_no_directive() {
    let mut h = Home::new();
    suggested(&mut h);
    let before = tops(&h);
    crate::raw::OPS_REFUSED.set(true);
    let got = h.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}));
    crate::raw::OPS_REFUSED.set(false);
    let (events, ops) = approval_since(&h, before);
    assert_eq!((events.len(), ops.len()), (0, 0), "{got:?}");
    assert!(got.is_err(), "{got:?}");
    assert_eq!(h.waiting(), ["s1"]);
}

/// The op checkpoint of `consumer` for `device` in `home`'s knowledge.db, read on a handle of its
/// own.
fn checkpoint(home: &std::path::Path, device: &str, consumer: &str) -> i64 {
    let k = crate::knowledge::open(home).unwrap();
    let ops = crate::knowledge::checkpoint::OPS;
    crate::knowledge::checkpoint::get_in(&k, ops, consumer, device).unwrap()
}

/// `f` with `claims::applied`'s wait set to `wait`.
fn waiting<T>(wait: std::time::Duration, f: impl FnOnce() -> T) -> T {
    crate::claims::WAIT.with(|w| w.set(Some(wait)));
    let got = f();
    crate::claims::WAIT.with(|w| w.set(None));
    got
}

/// #1083's R2b (PR 1): an undo of an approval whose decision the my_work consumer applied while the
/// claims consumer has not kept its preference yet is refused, as pending, and records nothing: the
/// preference would be kept after it, active with its approval undone. Once the claims consumer
/// keeps it, the undo retracts it.
#[test]
fn an_undo_before_the_approvals_preference_is_kept_waits_for_it() {
    let mut h = Home::new();
    suggested(&mut h);
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let got = waiting(std::time::Duration::ZERO, || h.act(approve)).unwrap();
    assert_eq!(got["state"], "pending", "{got}");
    let my_work: Vec<Box<dyn crate::worker::Consumer>> =
        vec![Box::new(crate::consumer::my_work::MyWork)];
    crate::worker::run_consumers(h.s.home.path(), 0, my_work).unwrap();
    let before = tops(&h);
    let undo = json!({"suggestion": "s1", "action": "undo"});
    let refused = h.act(undo.clone()).unwrap_err();
    assert_eq!((refused.status, refused.code), (409, "rule_pending"));
    assert_eq!(tops(&h), before, "nothing recorded");
    h.s.run();
    let got = h.act(undo).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    assert_eq!(
        (h.yours(), h.waiting()),
        (Vec::<(String, bool)>::new(), vec!["s1".to_owned()])
    );
    assert_eq!(h.started_with(RULE_TEXT), [false; 3]);
}

/// #1083's R2b (PR 1): a retry of an approval whose decision the my_work consumer applies while the
/// retry reads, between what My Rules shows and what is not applied yet, reads one state of
/// knowledge.db: it is refused, and one directive and one preference stand.
#[test]
fn a_retry_while_its_approval_is_applied_records_no_second_preference() {
    let mut h = Home::new();
    suggested(&mut h);
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let before = tops(&h);
    let got = waiting(std::time::Duration::ZERO, || h.act(approve.clone())).unwrap();
    assert_eq!(got["state"], "pending", "{got}");
    let home = h.s.home.path().to_owned();
    let (device, decision) = (h.s.raw.device().to_owned(), before.1 + 2);
    crate::settings::claims::BETWEEN_READS.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::worker::run_once(&home).unwrap();
            let at = checkpoint(&home, &device, "my_work");
            assert!(
                at >= decision,
                "the decision is applied between the reads: {at}"
            );
        }));
    });
    let retried = h.act(approve).unwrap_err();
    assert_eq!((retried.status, retried.code), (409, "rule_pending"));
    h.s.run();
    let (events, ops) = approval_since(&h, before);
    assert_eq!((events.len(), ops.len()), (1, 2));
    assert_eq!(h.yours(), [(RULE_TEXT.to_owned(), true)]);
}

/// #1083's R2b (PR 1): an approval's receipt takes its decision as kept only from its own row, with
/// the subject and the fields it wrote, neither skipped nor forgotten.
#[test]
fn an_approvals_decision_is_kept_only_as_it_was_written() {
    let mut h = Home::new();
    suggested(&mut h);
    let before = tops(&h);
    let got = (h.act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))).unwrap();
    assert_eq!(got["state"], "applied", "{got}");
    let uid = got["uid"].as_str().unwrap();
    let device = h.s.raw.device().to_owned();
    let k = crate::knowledge::open(h.s.home.path()).unwrap();
    let seq: i64 = k
        .query_row(
            "SELECT op_seq FROM my_work WHERE device = ?1 AND job = 'rule' AND op_seq > ?2",
            rusqlite::params![device, before.1],
            |r| r.get(0),
        )
        .unwrap();
    let fields = json!({"action": "approve", "claim": uid});
    let kept = |about: &str, fields: &Value| {
        crate::my_work_ops::kept(&k, &device, seq, (RULE, about), fields).unwrap()
    };
    assert!(kept("s1", &fields));
    assert!(!kept("s2", &fields));
    assert!(!kept("s1", &json!({"action": "approve"})));
    k.execute(
        "UPDATE my_work SET skipped = 1 WHERE device = ?1 AND op_seq = ?2",
        rusqlite::params![device, seq],
    )
    .unwrap();
    assert!(!kept("s1", &fields), "skipped");
}

/// #1130's review: an approval's and an undo's receipts are applied only on the rows they wrote, as
/// written: a decision row the consumer kept unusable, or a retraction's row of another status or
/// another uid, leaves them pending.
#[test]
fn receipts_stay_pending_on_rows_not_as_written() {
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let tamper = |h: &Home, sql: &'static str| {
        let home = h.s.home.path().to_owned();
        crate::claims::PROVING.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let k = crate::knowledge::open(&home).unwrap();
                assert_eq!(k.execute(sql, []).unwrap(), 1, "{sql}");
            }));
        });
    };
    let mut h = Home::new();
    suggested(&mut h);
    tamper(&h, "UPDATE my_work SET skipped = 1 WHERE job = 'rule'");
    let got = h.act(approve.clone()).unwrap();
    assert_eq!(got["state"], "pending", "an approval: {got}");
    for sql in [
        "UPDATE corrections SET status = 'decided' WHERE status = 'retracted'",
        "UPDATE corrections SET uid = 'x' || uid WHERE status = 'retracted'",
    ] {
        let mut h = Home::new();
        suggested(&mut h);
        assert_eq!(h.act(approve.clone()).unwrap()["state"], "applied");
        tamper(&h, sql);
        let got = h
            .act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        assert_eq!(got["state"], "pending", "an undo, {sql}: {got}");
    }
}

/// #1083's R2b (PR 1): an approval's two waits share one deadline: one that the claims consumer's
/// wait spends leaves the my_work consumer's none, and the approval is pending.
#[test]
fn an_approvals_waits_share_one_deadline() {
    let mut h = Home::new();
    suggested(&mut h);
    let home = h.s.home.path().to_owned();
    let before = tops(&h);
    let (device, preference, decision) = (h.s.raw.device().to_owned(), before.1 + 1, before.1 + 2);
    let wait = std::time::Duration::from_millis(300);
    crate::claims::WAITING.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let claims: Vec<Box<dyn crate::worker::Consumer>> =
                vec![Box::new(crate::consumer::claims::Claims)];
            crate::worker::run_consumers(&home, 0, claims).unwrap();
            assert!(
                checkpoint(&home, &device, "claims") >= preference,
                "the first wait's"
            );
            assert!(
                checkpoint(&home, &device, "my_work") < decision,
                "the second wait's"
            );
            std::thread::sleep(wait * 2);
        }));
    });
    let approve = json!({"suggestion": "s1", "action": "approve", "scope": "global"});
    let got = waiting(wait, || h.act(approve)).unwrap();
    assert_eq!(got["state"], "pending", "{got}");
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
    crate::claims::pref_add_in(h.s.home.path(), "Keep the changelog short.", Some(&repo)).unwrap();
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
/// to retract, and the suggestion waits again. Before the forget's purge too, while knowledge.db
/// still holds the preference (#1083's R2b): a retraction of it would be refused.
#[test]
fn an_approve_whose_rule_was_forgotten_is_still_undone() {
    for purged in [true, false] {
        let mut h = Home::new();
        suggested(&mut h);
        let got = h
            .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
            .unwrap();
        let uid = got["uid"].as_str().unwrap().to_owned();
        let h = h.closed(|home| {
            let target = crate::forget::Target::Uid { uid };
            crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
            if purged {
                crate::forget::purge(home).unwrap();
            }
        });
        assert_eq!(h.yours(), Vec::<(String, bool)>::new());
        let got = h
            .act(json!({"suggestion": "s1", "action": "undo"}))
            .unwrap();
        assert_eq!(got["state"], "applied", "purged: {purged}");
        assert_eq!(h.waiting(), ["s1"], "purged: {purged}");
    }
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

/// A forgotten rule leaves the page at once, as agents stop getting it: knowledge.db holds it
/// until the forget's purge rebuilds the store, and the page listed it under the owner's rules
/// until then. It is under the removed ones, as a rule forgotten and purged is.
#[test]
fn a_forgotten_rule_leaves_the_page_before_the_purge() {
    let mut h = Home::new();
    suggested(&mut h);
    let got = h
        .act(json!({"suggestion": "s1", "action": "approve", "scope": "global"}))
        .unwrap();
    let uid = got["uid"].as_str().unwrap().to_owned();
    assert_eq!(h.yours().len(), 1);
    let h = h.closed(|home| {
        let target = crate::forget::Target::Uid { uid };
        crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
    });
    assert_eq!(h.yours(), Vec::<(String, bool)>::new());
    assert_eq!(h.page()["removed"][0]["id"], "s1");
}
