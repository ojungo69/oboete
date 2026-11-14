use super::*;
use crate::provider::{Fallback, Skip};
use crate::search::b::fixture::Store;
use serde_json::json;
use std::cell::RefCell;

const MINUTE: i64 = 60_000;

/// A job of `subjects`, each due since its time until an op about it is kept: it shows record
/// `seq` and keeps an answer's `text`.
struct Echo {
    subjects: Vec<(&'static str, i64)>,
    seq: i64,
}

impl Job for Echo {
    fn name(&self) -> &'static str {
        "group"
    }

    fn reread(&self) {}

    fn due(&self, s: &Stores, _: i64) -> Result<Vec<Due>> {
        let mut due = Vec::new();
        for (about, since) in &self.subjects {
            if crate::my_work_ops::newest(s.k, "group", about)?.is_none() {
                due.push(Due {
                    about: (*about).into(),
                    since: *since,
                });
            }
        }
        Ok(due)
    }

    /// A subject named `quiet…` has nothing to send, and one named `blank…` an empty prompt,
    /// as the goals job's when the text tests flag each prompt.
    fn ask(&self, _: &Stores, about: &str, _: i64) -> Result<Option<Ask>> {
        if about.starts_with("quiet") {
            return Ok(None);
        }
        Ok(Some(Ask {
            prompt: if about.starts_with("blank") {
                String::new()
            } else {
                format!("Tell me about {about}.")
            },
            schema: json!({"type": "object"}),
            read: Read {
                spans: vec![(self.seq, self.seq)],
                ops: vec![],
            },
            removed: vec![],
            check: Box::new(|v: &Value| (!v["text"].is_string()).then_some("shape")),
            subjects: Vec::new(),
            keep: Box::new(|v: &Value| BTreeMap::from([("text".to_owned(), v["text"].clone())])),
        }))
    }
}

/// One pass of a runner of `job` with `[my_work]` as `(curate, calls_a_day)`, the chain
/// answering as `answer` does: what it did. Each span asked goes to `asked`.
fn pass(
    s: &mut Store,
    job: Echo,
    my_work: (bool, u32),
    overdue: bool,
    answer: &dyn Fn(&Gate) -> Result<Value>,
    asked: &RefCell<Vec<String>>,
) -> Phase {
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let (rules, summary) = (Rules::load(s.home.path()).unwrap(), Summary::default());
    let call =
        |span: &str, _: &str, _: &Value, check: &AnswerCheck, gate: &Gate| -> Result<ChainResult> {
            asked.borrow_mut().push(span.to_owned());
            let output = answer(gate)?;
            assert_eq!(check(&output), None);
            Ok(ChainResult {
                provider: "fake".into(),
                output,
                tier: 1,
            })
        };
    let mut runner = Runner::new(
        my_work,
        vec![Box::new(job) as Box<dyn Job>],
        Box::new(call),
        (&rules, &summary, "chain"),
    );
    let phase = runner.pass(&mut s.raw, &k, &db, overdue).unwrap();
    s.run();
    phase
}

fn grouped(_: &Gate) -> Result<Value> {
    Ok(json!({"text": "Grouped."}))
}

fn failed(_: &Gate) -> Result<Value> {
    Err(ChainFailed(vec![Fallback {
        provider: "fake".into(),
        reason: "down".into(),
        skip: Skip::Failed,
        answer: None,
    }])
    .into())
}

fn kept(s: &Store, about: &str) -> Option<MyWorkOp> {
    let k = crate::knowledge::open(s.home.path()).unwrap();
    crate::my_work_ops::newest(&k, "group", about)
        .unwrap()
        .map(|kept| kept.op)
}

/// A1, A6: nothing is asked while `[my_work] curate` is off; on, the job's answer is kept as
/// its op, with the entry that wrote it and what it was shown, and the job is not due again.
#[test]
fn a_job_is_asked_only_while_my_work_is_on_and_its_answer_is_kept() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let echo = || Echo {
        subjects: vec![("r1", 0)],
        seq,
    };
    let asked = RefCell::new(Vec::new());
    assert_eq!(
        pass(&mut s, echo(), (false, 100), false, &grouped, &asked),
        Phase::Idle
    );
    assert!(asked.borrow().is_empty());
    let phase = pass(&mut s, echo(), (true, 100), false, &grouped, &asked);
    assert_eq!(
        (phase, asked.borrow().clone()),
        (Phase::Covered, vec!["group r1".to_owned()])
    );
    let op = kept(&s, "r1").unwrap();
    assert_eq!((op.entry.as_str(), op.skipped), ("fake", false));
    assert_eq!(op.fields["text"], json!("Grouped."));
    assert_eq!(op.read.spans, vec![(seq, seq)]);
    assert_eq!(
        pass(&mut s, echo(), (true, 100), false, &grouped, &asked),
        Phase::Idle
    );
    assert_eq!(asked.borrow().len(), 1);
}

/// A3: the job due the longest goes first, and a round makes one call at most.
#[test]
fn a_round_makes_one_call_at_most_the_job_due_longest_first() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let (rules, summary) = (Rules::default(), Summary::default());
    let asked = RefCell::new(Vec::new());
    let call = |span: &str, _: &str, _: &Value, _: &AnswerCheck, _: &Gate| -> Result<ChainResult> {
        asked.borrow_mut().push(span.to_owned());
        Ok(ChainResult {
            provider: "fake".into(),
            output: json!({"text": "Grouped."}),
            tier: 1,
        })
    };
    let echo = Echo {
        subjects: vec![("newer", 2), ("older", 1)],
        seq,
    };
    let mut runner = Runner::new(
        (true, 100),
        vec![Box::new(echo) as Box<dyn Job>],
        Box::new(call),
        (&rules, &summary, "chain"),
    );
    assert_eq!(
        runner.pass(&mut s.raw, &k, &db, false).unwrap(),
        Phase::Covered
    );
    assert_eq!(
        runner.pass(&mut s.raw, &k, &db, false).unwrap(),
        Phase::Covered
    );
    assert_eq!(*asked.borrow(), ["group older"]);
}

/// A3: the pass before a window takes only a job due an hour or more.
#[test]
fn only_a_job_due_an_hour_goes_before_a_window() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let now = crate::db::now_ms();
    let asked = RefCell::new(Vec::new());
    let echo = |since: i64| Echo {
        subjects: vec![("r1", since)],
        seq,
    };
    let early = pass(
        &mut s,
        echo(now - 59 * MINUTE),
        (true, 100),
        true,
        &grouped,
        &asked,
    );
    assert_eq!((early, asked.borrow().len()), (Phase::Idle, 0));
    let early = pass(
        &mut s,
        echo(now - 61 * MINUTE),
        (true, 100),
        true,
        &grouped,
        &asked,
    );
    assert_eq!((early, asked.borrow().len()), (Phase::Covered, 1));
}

/// A subject due later is waited for, the worker staying up for one within 30 minutes (D10),
/// and nothing is asked before.
#[test]
fn a_job_due_later_is_waited_for() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let now = crate::db::now_ms();
    let asked = RefCell::new(Vec::new());
    for (later, up) in [(10 * MINUTE, true), (40 * MINUTE, false)] {
        let echo = Echo {
            subjects: vec![("r1", now + later)],
            seq,
        };
        let phase = pass(&mut s, echo, (true, 100), false, &grouped, &asked);
        assert_eq!(
            phase,
            Phase::Waiting {
                until: now + later,
                up
            }
        );
    }
    assert!(asked.borrow().is_empty());
}

/// A7: at the bound every job waits, held for budget, until the oldest call counted leaves the
/// rolling day; under it, it is asked.
#[test]
fn at_the_bound_a_job_waits_for_the_oldest_call_to_leave_the_day() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let db = crate::providers_db::open(s.home.path()).unwrap();
    for _ in 0..2 {
        let call = crate::providers_db::Call {
            provider: "fake",
            role: crate::my_work::ROLE,
            span: "group r0",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: None,
            usage: Default::default(),
            usd: None,
        };
        crate::providers_db::record(&db, &call).unwrap();
    }
    let oldest = crate::db::now_ms() - 3 * 60 * MINUTE;
    db.execute(
        "UPDATE provider_calls SET ts = ?1 WHERE rowid = 1",
        [oldest],
    )
    .unwrap();
    let echo = || Echo {
        subjects: vec![("r1", 0)],
        seq,
    };
    let asked = RefCell::new(Vec::new());
    let phase = pass(&mut s, echo(), (true, 2), false, &grouped, &asked);
    assert_eq!(
        phase,
        Phase::Waiting {
            until: oldest + DAY_MS,
            up: false
        }
    );
    assert!(asked.borrow().is_empty());
    assert_eq!(
        pass(&mut s, echo(), (true, 3), false, &grouped, &asked),
        Phase::Covered
    );
}

/// A7 at each request (Codex on #668): a chain that falls through to another entry past the
/// bound sends nothing more; the job waits for the oldest call to leave the day, with no
/// attempt counted.
#[test]
fn a_fallback_past_the_bound_is_not_sent() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let home = s.home.path().to_owned();
    let sent = std::cell::Cell::new(0);
    // As the chain asks: each request reserved, then gated, the first failing over.
    let chain = |gate: &Gate| -> Result<Value> {
        let db = crate::providers_db::open(&home).unwrap();
        for outcome in ["error", "ok"] {
            let call = crate::providers_db::Call {
                provider: "fake",
                role: crate::my_work::ROLE,
                span: "group r1",
                outcome: "reserved",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: None,
                usage: Default::default(),
                usd: None,
            };
            crate::providers_db::record(&db, &call)?;
            // A request the gate refuses is not sent: its reservation is cancelled.
            let admitted = gate();
            if admitted.is_err() {
                db.execute("DELETE FROM provider_calls WHERE outcome = 'reserved'", [])?;
            }
            drop(admitted?);
            sent.set(sent.get() + 1);
            db.execute(
                "UPDATE provider_calls SET outcome = ?1 WHERE outcome = 'reserved'",
                [outcome],
            )?;
        }
        Ok(json!({"text": "Grouped."}))
    };
    let echo = Echo {
        subjects: vec![("r1", 0)],
        seq,
    };
    let asked = RefCell::new(Vec::new());
    let phase = pass(&mut s, echo, (true, 1), false, &chain, &asked);
    assert_eq!(sent.get(), 1, "the fallback was not sent");
    let db = crate::providers_db::open(&home).unwrap();
    let (calls, oldest) = crate::providers_db::my_work_calls_in_a_day(&db).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(
        phase,
        Phase::Waiting {
            until: oldest.unwrap() + DAY_MS,
            up: false
        }
    );
    assert!(kept(&s, "r1").is_none());
    let pending = crate::providers_db::my_work_pending_of(&db, "group r1").unwrap();
    assert!(pending.is_none(), "no attempt counted");
}

/// A8: a job every entry fails waits for its next attempt, is asked again then, and after
/// `ATTEMPTS` is kept as a skip of what it read, not asked again; a request that changed is
/// asked whatever an older one waits for.
#[test]
fn a_job_every_entry_fails_waits_then_is_kept_as_a_skip() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let echo = || Echo {
        subjects: vec![("r1", 0)],
        seq,
    };
    let asked = RefCell::new(Vec::new());
    let phase = pass(&mut s, echo(), (true, 100), false, &failed, &asked);
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    let waiting = crate::providers_db::my_work_pending_of(&db, "group r1")
        .unwrap()
        .unwrap();
    assert_eq!((waiting.attempts, waiting.hold.as_str()), (1, "time"));
    assert!(waiting.next_attempt_at > crate::db::now_ms());
    // Before its next attempt, nothing is sent.
    let phase = pass(&mut s, echo(), (true, 100), false, &failed, &asked);
    assert_eq!(
        phase,
        Phase::Waiting {
            until: waiting.next_attempt_at,
            up: true
        }
    );
    assert_eq!(asked.borrow().len(), 1);
    // A request that changed is not held by the wait of an older one.
    db.execute("UPDATE my_work_pending SET prompt = 'another'", [])
        .unwrap();
    pass(&mut s, echo(), (true, 100), false, &failed, &asked);
    let waiting = crate::providers_db::my_work_pending_of(&db, "group r1")
        .unwrap()
        .unwrap();
    assert_eq!((asked.borrow().len(), waiting.attempts), (2, 1));
    for attempts in [2, 3] {
        db.execute("UPDATE my_work_pending SET next_attempt_at = 0", [])
            .unwrap();
        let phase = pass(&mut s, echo(), (true, 100), false, &failed, &asked);
        assert_eq!(asked.borrow().len(), attempts as usize + 1);
        if attempts < crate::curate::ATTEMPTS {
            assert!(matches!(phase, Phase::Waiting { .. }));
        } else {
            assert_eq!(phase, Phase::Covered);
        }
    }
    let op = kept(&s, "r1").unwrap();
    assert_eq!(
        (op.skipped, op.entry.as_str(), op.fields.len()),
        (true, "", 0)
    );
    assert_eq!(op.read.spans, vec![(seq, seq)]);
    assert_eq!(
        crate::providers_db::my_work_pending_of(&db, "group r1").unwrap(),
        None
    );
    assert_eq!(
        pass(&mut s, echo(), (true, 100), false, &failed, &asked),
        Phase::Idle
    );
    assert_eq!(asked.borrow().len(), 4);
}

/// A5: an exclusion made, or redaction turned on, while a job waits for its call sends nothing
/// and keeps nothing; the next pass reads again.
#[test]
fn a_job_sends_and_keeps_nothing_when_the_list_or_the_rules_change_before_its_call() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let home = s.home.path().to_owned();
    let echo = || Echo {
        subjects: vec![("r1", 0)],
        seq,
    };
    let asked = RefCell::new(Vec::new());
    let excluding = |gate: &Gate| {
        crate::raw::open(&home)
            .unwrap()
            .exclude("elsewhere", false)
            .unwrap();
        gate()?;
        grouped(gate)
    };
    let phase = pass(&mut s, echo(), (true, 100), false, &excluding, &asked);
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert_eq!(kept(&s, "r1"), None);
    std::fs::write(home.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
    let unmasking = |gate: &Gate| {
        std::fs::write(home.join("config.toml"), "[redaction]\nenabled = true\n").unwrap();
        gate()?;
        grouped(gate)
    };
    let phase = pass(&mut s, echo(), (true, 100), false, &unmasking, &asked);
    assert!(
        matches!(phase, Phase::Waiting { up: true, .. }),
        "{phase:?}"
    );
    assert_eq!(kept(&s, "r1"), None);
    assert_eq!(asked.borrow().len(), 2);
}

/// A job that keeps what it read for its round, as the real ones do: it shows the session's
/// text unless the list it read under excludes a session.
#[derive(Default)]
struct Keeper {
    read: RefCell<Option<String>>,
}

impl Job for Keeper {
    fn name(&self) -> &'static str {
        "group"
    }

    fn due(&self, s: &Stores, now: i64) -> Result<Vec<Due>> {
        if self.read.borrow().is_none() {
            let shown = match s.reading.excluded.is_empty() {
                true => "Build the parser.",
                false => "",
            };
            *self.read.borrow_mut() = Some(shown.to_owned());
        }
        Ok(vec![Due {
            about: "r1".into(),
            since: now - MINUTE,
        }])
    }

    fn ask(&self, _: &Stores, _: &str, _: i64) -> Result<Option<Ask>> {
        Ok(Some(Ask {
            prompt: self.read.borrow().clone().unwrap_or_default(),
            schema: json!({"type": "object"}),
            read: Read {
                spans: vec![],
                ops: vec![],
            },
            removed: vec![],
            check: Box::new(|_: &Value| None),
            subjects: Vec::new(),
            keep: Box::new(|_: &Value| BTreeMap::new()),
        }))
    }

    fn reread(&self) {
        self.read.take();
    }
}

/// A5: a round's two passes are one runner's, and a job reads once a round. An exclusion made
/// between them (while the windows' calls are out) is not in what the job kept of its read
/// under the first pass's list, and the second pass's own list is the newer one, so its gate
/// had nothing to refuse: the job reads again under the list the pass sends by.
#[test]
fn a_job_reads_again_when_the_list_changed_between_a_rounds_passes() {
    let mut s = Store::new();
    s.said("s1", "r", 1_000, "Build the parser.");
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let (rules, summary) = (Rules::load(s.home.path()).unwrap(), Summary::default());
    let sent = RefCell::new(Vec::new());
    let call =
        |_: &str, prompt: &str, _: &Value, _: &AnswerCheck, gate: &Gate| -> Result<ChainResult> {
            gate()?;
            sent.borrow_mut().push(prompt.to_owned());
            Ok(ChainResult {
                provider: "fake".into(),
                output: json!({}),
                tier: 1,
            })
        };
    let mut runner = Runner::new(
        (true, 100),
        vec![Box::new(Keeper::default()) as Box<dyn Job>],
        Box::new(call),
        (&rules, &summary, "chain"),
    );
    // Before the windows: due a minute, so not yet asked, and its read is kept.
    let phase = runner.pass(&mut s.raw, &k, &db, true).unwrap();
    assert!(matches!(phase, Phase::Idle), "{phase:?}");
    s.exclude("r");
    runner.pass(&mut s.raw, &k, &db, false).unwrap();
    assert_eq!(*sent.borrow(), Vec::<String>::new());
}

fn entry(name: &str, limits: crate::config::Limits) -> crate::config::Provider {
    crate::config::Provider::Openai {
        enabled: true,
        name: name.into(),
        base_url: "http://127.0.0.1:9".into(),
        api: Default::default(),
        key_file: None,
        model: "m".into(),
        daily_budget: Some(10),
        timeout_s: 1,
        retry_429: false,
        extra: Default::default(),
        headers: Default::default(),
        limits,
        subscription: false,
    }
}

/// A6: the estimate counts the week's volumes as the cost table does, at most the bound, and
/// prices them at the chain's first paid entry, the bound at the largest call; with no paid
/// entry, no USD.
#[test]
fn the_estimate_counts_the_weeks_volumes_and_prices_the_first_paid_entry() {
    let mut s = Store::new();
    let now = crate::db::now_ms();
    let hour = 3_600_000;
    // Each of the last 7 days: two new conversations of one repository in two half hours
    // (14 grouping calls), with 3 and 2 hours of prompts (35 reviews); a week before, one more.
    for d in 0..7 {
        let day = now - d * DAY_MS;
        for h in [5, 4, 3] {
            s.said(&format!("a{d}"), "r", day - h * hour, "Build the parser.");
        }
        for h in [3, 2] {
            s.said(&format!("b{d}"), "r", day - h * hour, "Add its tests.");
        }
    }
    s.said("old", "r", now - 9 * DAY_MS, "Long ago.");
    let paid = crate::config::Limits {
        usd_per_mtok_in: 1.0,
        usd_per_mtok_out: 5.0,
        max_output_tokens: 4_000,
        ..Default::default()
    };
    let chain = [
        entry("free", crate::config::Limits::default()),
        entry("paid", paid.clone()),
    ];
    // A day: 2 groupings, 5 reviews, 1 links, 2 placements (one an Other pass), 1 story, 0.5
    // rules, 1 goals.
    let flat = |_: &str| -> Result<f64> { Ok(1.0) };
    let e = estimate(&s.raw, &chain, 100, (5_000, &flat), now).unwrap();
    assert_eq!(e.calls, (13, 100));
    assert_eq!(e.priced, Some(("paid".to_owned(), "m".to_owned())));
    let (typical, most) = e.usd.unwrap();
    let usd = |calls: f64, input: f64, output: f64| calls * (input + 5.0 * output) / 1e6;
    let expected = usd(2.0, 6_500.0, 300.0)
        + usd(5.0, 4_000.0, 1_000.0)
        + usd(1.0, 4_000.0, 300.0)
        + usd(2.0, 4_000.0, 300.0)
        + usd(1.0, 4_000.0, 1_700.0)
        + usd(0.5, 2_000.0, 300.0)
        + usd(1.0, 4_000.0, 2_000.0);
    assert!((typical - expected).abs() < 1e-9, "{typical} {expected}");
    assert!((most - usd(100.0, 6_500.0, 4_000.0)).abs() < 1e-9, "{most}");
    // A larger window and an entry that counts twice the estimate (Codex on #671): each input
    // at the factor, the largest request its instructions and the window.
    let twice = |_: &str| -> Result<f64> { Ok(2.0) };
    let (typical, most) = (estimate(&s.raw, &chain, 100, (8_000, &twice), now).unwrap())
        .usd
        .unwrap();
    let doubled = usd(2.0, 13_000.0, 300.0)
        + usd(5.0, 8_000.0, 1_000.0)
        + usd(1.0, 8_000.0, 300.0)
        + usd(2.0, 8_000.0, 300.0)
        + usd(1.0, 8_000.0, 1_700.0)
        + usd(0.5, 4_000.0, 300.0)
        + usd(1.0, 8_000.0, 2_000.0);
    assert!((typical - doubled).abs() < 1e-9, "{typical} {doubled}");
    assert!(
        (most - usd(100.0, 19_000.0, 4_000.0)).abs() < 1e-9,
        "{most}"
    );
    // Over the bound, every job waits alike.
    let bound = estimate(&s.raw, &chain, 5, (5_000, &flat), now).unwrap();
    assert_eq!(bound.calls, (5, 5));
    let (typical, _) = bound.usd.unwrap();
    assert!((typical - expected * 5.0 / 12.5).abs() < 1e-9, "{typical}");
    let free = estimate(&s.raw, &chain[..1], 100, (5_000, &flat), now).unwrap();
    assert_eq!((free.calls, free.usd, free.priced), ((13, 100), None, None));
    // Decision 86: on a Messages entry each input is priced as an hour's cache write, twice the
    // input price, the most a My Work call's input may bill.
    let mut messages = entry("paid", paid);
    if let crate::config::Provider::Openai { api, .. } = &mut messages {
        *api = crate::config::Api::Anthropic;
    }
    let (typical, _) = (estimate(&s.raw, &[messages], 100, (5_000, &flat), now).unwrap())
        .usd
        .unwrap();
    assert!((typical - doubled).abs() < 1e-9, "{typical} {doubled}");
}

/// A6: doctor names the switch and what turning it on would cost.
#[test]
fn doctor_names_the_switch_and_what_turning_it_on_costs() {
    let mut s = Store::new();
    s.said("s1", "r", crate::db::now_ms(), "Build the parser.");
    let home = s.home.path().to_owned();
    std::fs::write(home.join("config.toml"), "providers = []\n").unwrap();
    let cfg = crate::config::load_chain(&home).unwrap();
    let line = doctor_line(&home, &cfg);
    assert!(
        line.starts_with("my work: off ([my_work] curate)"),
        "{line}"
    );
    assert!(
        line.contains("calls a day with it on: about 1, at most 100"),
        "{line}"
    );
    assert!(line.ends_with("no paid entry is in the chain"), "{line}");
    let fresh = tempfile::tempdir().unwrap();
    let line = doctor_line(fresh.path(), &cfg);
    assert!(
        line.ends_with("no estimate: nothing is recorded yet"),
        "{line}"
    );
    // An entry turned off is not priced, as the worker leaves it out of the chain.
    std::fs::write(
            home.join("config.toml"),
            "[[providers]]\nkind = \"openai\"\nname = \"p\"\nenabled = false\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\nlimits = { usd_per_mtok_in = 1.0 }\n",
        )
        .unwrap();
    let cfg = crate::config::load(&home).unwrap();
    let line = doctor_line(&home, &cfg);
    assert!(line.ends_with("no paid entry is in the chain"), "{line}");
    let entry = "[[providers]]\nkind = \"openai\"\nname = \"p\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\nlimits = { usd_per_mtok_in = 1.0 }\n";
    let doctor = |text: String| {
        std::fs::write(home.join("config.toml"), text).unwrap();
        doctor_line(&home, &crate::config::load(&home).unwrap())
    };
    let line = doctor(format!("[chain]\noff = [\"p\"]\n{entry}"));
    assert!(line.ends_with("no paid entry is in the chain"), "{line}");
    let line = doctor(format!("[my_work]\ncurate = true\n{entry}"));
    assert!(
        line.starts_with("my work: on, waiting for [summary] curate"),
        "{line}"
    );
    assert!(line.ends_with("at p's prices (m)"), "{line}");
    let line = doctor(format!(
        "[my_work]\ncurate = true\n[summary]\ncurate = true\n{entry}"
    ));
    assert!(line.starts_with("my work: on ([my_work] curate)"), "{line}");
    // At most: 100 calls of the largest request, the instructions and the configured window,
    // at the factor the entry's answers taught (Codex on #671): 100 x 6,500 at the default,
    // 100 x 9,500 with a window of 8,000, twice that once five answers counted twice.
    assert!(line.contains("at most USD 0.65"), "{line}");
    let window = format!("[summary]\nwindow_tokens = 8000\n{entry}");
    let line = doctor(window.clone());
    assert!(line.contains("at most USD 0.95"), "{line}");
    let db = crate::providers_db::open(&home).unwrap();
    for _ in 0..5 {
        let usage = crate::providers_db::Usage {
            prompt: Some(2_000),
            ..Default::default()
        };
        let call = crate::providers_db::Call {
            provider: "p",
            role: "curator",
            span: "s",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: Some(1_000),
            usage,
            usd: None,
        };
        crate::providers_db::record(&db, &call).unwrap();
    }
    let line = doctor(window);
    assert!(line.contains("at most USD 1.90"), "{line}");
    // With no cap (decision 64), no most of calls or of USD: 4294967295 is not a most.
    let no_cap = "[my_work]\ncalls_a_day = 4294967295\n";
    let line = doctor(format!("{no_cap}{entry}"));
    assert!(
        line.contains("about 1, no cap of calls a day, about USD "),
        "{line}"
    );
    assert!(!line.contains("at most"), "{line}");
    assert!(line.ends_with("at p's prices (m)"), "{line}");
    let line = doctor(format!("providers = []\n{no_cap}"));
    assert!(
        line.ends_with("about 1, no cap of calls a day, no USD: no paid entry is in the chain"),
        "{line}"
    );
}

/// V1: the status line says off, on and waiting for summaries, waiting at the daily bound or
/// for a held request of the page's own jobs, or when they last wrote, with no call.
#[test]
fn the_status_line_says_off_waiting_or_written_with_no_call() {
    let s = Store::new();
    let p = s.home.path();
    let status = |jobs: &[&str]| status(p, jobs, crate::db::now_ms()).unwrap();
    let config = |text: &str| std::fs::write(p.join("config.toml"), text).unwrap();
    assert_eq!(status(&["goals"]), json!({"state": "off"}));
    config("[my_work]\ncurate = true\n");
    assert_eq!(status(&["goals"]), json!({"state": "summaries_off"}));
    config("[summary]\ncurate = true\n[my_work]\ncurate = true\ncalls_a_day = 2\n");
    assert_eq!(status(&["goals"]), json!({"state": "none"}));
    let op = MyWorkOp {
        job: "goals".into(),
        about: "2026-10-27".into(),
        entry: "fake".into(),
        ts: 1_000,
        read: Read::default(),
        removed: vec![],
        fields: BTreeMap::new(),
        skipped: false,
        forgotten: None,
    };
    let mut raw = crate::raw::open(p).unwrap();
    raw.append_ops(&[(OpKind::MyWork, serde_json::to_value(&op).unwrap())])
        .unwrap();
    s.run();
    assert_eq!(status(&["goals"]), json!({"state": "written", "at": 1_000}));
    assert_eq!(status(&["group", "review"]), json!({"state": "none"}));
    let review = MyWorkOp {
        job: "review".into(),
        about: "r1".into(),
        ts: 2_000,
        ..op
    };
    raw.append_ops(&[(OpKind::MyWork, serde_json::to_value(review).unwrap())])
        .unwrap();
    s.run();
    // The newest of the page's jobs, in whatever order they are named.
    for jobs in [["goals", "review"], ["review", "goals"]] {
        assert_eq!(status(&jobs), json!({"state": "written", "at": 2_000}));
    }
    let db = crate::providers_db::open(p).unwrap();
    let later = crate::db::now_ms() + 60_000;
    let hold = |hold: &str, until: i64| {
        crate::providers_db::set_my_work_round(&db, "goals", hold, until).unwrap();
    };
    hold("budget", later);
    let waiting = json!({"state": "waiting", "why": "budget", "until": later});
    assert_eq!(status(&["goals"]), waiting);
    for (held, why) in [("time", "resting"), ("owner", "owner")] {
        hold(held, later);
        assert_eq!(status(&["goals"])["why"], why, "{held}");
    }
    // Another page's job is not held, and a time passed holds nothing.
    let review = json!({"state": "written", "at": 2_000});
    assert_eq!(status(&["group", "review"]), review);
    hold("budget", crate::db::now_ms() - 60_000);
    assert_eq!(status(&["goals"]), json!({"state": "written", "at": 1_000}));
    for _ in 0..2 {
        let call = crate::providers_db::Call {
            provider: "fake",
            role: "my_work",
            span: "group r1",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 0,
            est_tokens: None,
            usage: crate::providers_db::Usage::default(),
            usd: None,
        };
        crate::providers_db::record(&db, &call).unwrap();
    }
    let bound = status(&["goals"]);
    assert_eq!(
        (bound["state"].as_str(), bound["why"].as_str()),
        (Some("waiting"), Some("bound"))
    );
    assert_eq!(bound["calls_a_day"], 2);
    assert!(bound["until"].as_i64().unwrap() > crate::db::now_ms());
}

/// V1 (#720): the status line says what the runner last decided. A request held for a budget
/// says waiting; once the round finds nothing due for the job, it does not, though the held
/// request's row is still there; a later request answered makes it written.
#[test]
fn the_status_line_follows_what_the_runner_last_decided() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let config = "[summary]\ncurate = true\n[my_work]\ncurate = true\n";
    std::fs::write(s.home.path().join("config.toml"), config).unwrap();
    let until = crate::db::now_ms() + 3_600_000;
    let budget = |_: &Gate| -> Result<Value> {
        Err(ChainFailed(vec![Fallback {
            provider: "fake".into(),
            reason: "the monthly cap".into(),
            skip: Skip::Budget(until),
            answer: None,
        }])
        .into())
    };
    let asked = RefCell::new(Vec::new());
    let echo = |subjects: Vec<(&'static str, i64)>| Echo { subjects, seq };
    let line = |s: &Store| status(s.home.path(), &["group"], crate::db::now_ms()).unwrap();
    pass(
        &mut s,
        echo(vec![("r1", 0)]),
        (true, 100),
        false,
        &budget,
        &asked,
    );
    let waiting = json!({"state": "waiting", "why": "budget", "until": until});
    assert_eq!(line(&s), waiting);
    // Held still, the next round finds the same request held and says so again.
    pass(
        &mut s,
        echo(vec![("r1", 0)]),
        (true, 100),
        false,
        &budget,
        &asked,
    );
    assert_eq!((line(&s), asked.borrow().len()), (waiting.clone(), 1));
    // Its subject gone, nothing is due: the held request's row stays, but the line waits on
    // nothing.
    pass(&mut s, echo(vec![]), (true, 100), false, &grouped, &asked);
    let db = crate::providers_db::open(s.home.path()).unwrap();
    assert!(
        crate::providers_db::my_work_pending_of(&db, "group r1")
            .unwrap()
            .is_some()
    );
    assert_eq!(line(&s), json!({"state": "none"}));
    // Another subject held, then a later one answered: written.
    pass(
        &mut s,
        echo(vec![("r2", 0)]),
        (true, 100),
        false,
        &budget,
        &asked,
    );
    assert_eq!(line(&s)["state"], "waiting");
    pass(
        &mut s,
        echo(vec![("r3", 0)]),
        (true, 100),
        false,
        &grouped,
        &asked,
    );
    assert_eq!(line(&s)["state"], "written");
    // The held request found again: waiting again.
    let r1 = echo(vec![("r1", 0)]);
    pass(&mut s, r1, (true, 100), false, &grouped, &asked);
    assert_eq!(line(&s), waiting);
    // Given up, a request holds the job no longer.
    let skip = json!({"job": "group", "about": "r1", "entry": "", "ts": 0, "read": {},
            "removed": [], "fields": {}, "skipped": true, "forgotten": null});
    given_up(
        &mut s.raw,
        &db,
        ("group", "group r1"),
        &[(OpKind::MyWork, skip.clone())],
        crate::db::now_ms(),
    )
    .unwrap();
    assert!(crate::providers_db::my_work_rounds(&db).unwrap().is_empty());
    // #733: given up while another subject of the job is held, the line stays on that hold.
    let now = crate::db::now_ms();
    let until = now + 30 * MINUTE;
    crate::providers_db::set_my_work_held(&db, ("group", "r2"), "budget", until).unwrap();
    crate::providers_db::set_my_work_held(&db, ("links", "r9"), "resting", now + MINUTE).unwrap();
    given_up(
        &mut s.raw,
        &db,
        ("group", "group r1"),
        &[(OpKind::MyWork, skip)],
        now,
    )
    .unwrap();
    assert_eq!(
        crate::providers_db::my_work_rounds(&db).unwrap(),
        [("group".to_owned(), "budget".to_owned(), until)]
    );
}

/// A store whose `[my_work]` and `[summary]` curate, with record `seq` said.
fn curating() -> (Store, i64) {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let config = "[summary]\ncurate = true\n[my_work]\ncurate = true\n";
    std::fs::write(s.home.path().join("config.toml"), config).unwrap();
    (s, seq)
}

/// #724: two subjects of one job held until different times, the later passed last, leave
/// the line on the earlier, the one the runner waits for.
#[test]
fn the_line_names_a_jobs_earliest_hold() {
    let (mut s, seq) = curating();
    let now = crate::db::now_ms();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    for (about, next) in [("r1", now + 10 * MINUTE), ("r2", now + 30 * MINUTE)] {
        let prompt = crate::curate::sha256_hex(&format!("chain\nTell me about {about}."));
        let p = crate::providers_db::MyWorkPending {
            key: format!("group {about}"),
            prompt,
            reason: "the monthly cap".into(),
            hold: "budget".into(),
            attempts: 1,
            next_attempt_at: next,
        };
        crate::providers_db::set_my_work_pending(&db, &p).unwrap();
    }
    let asked = RefCell::new(Vec::new());
    let job = Echo {
        subjects: vec![("r1", 0), ("r2", 1)],
        seq,
    };
    pass(&mut s, job, (true, 100), false, &grouped, &asked);
    assert!(asked.borrow().is_empty());
    let line = status(s.home.path(), &["group"], crate::db::now_ms()).unwrap();
    assert_eq!(
        line,
        json!({"state": "waiting", "why": "budget", "until": now + 10 * MINUTE})
    );
    // A later subject asked and failing, its wait longer, leaves the earlier hold too.
    let (mut s, seq) = curating();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let prompt = crate::curate::sha256_hex("chain\nTell me about r1.");
    let p = crate::providers_db::MyWorkPending {
        key: "group r1".into(),
        prompt,
        reason: "the monthly cap".into(),
        hold: "budget".into(),
        attempts: 1,
        next_attempt_at: now + MINUTE,
    };
    crate::providers_db::set_my_work_pending(&db, &p).unwrap();
    let job = Echo {
        subjects: vec![("r1", 0), ("r2", 1)],
        seq,
    };
    let phase = pass(&mut s, job, (true, 100), false, &failed, &asked);
    assert_eq!(asked.borrow().clone(), vec!["group r2".to_owned()]);
    assert!(
        matches!(phase, Phase::Waiting { until, .. } if until == now + MINUTE),
        "the runner wakes for it: {phase:?}"
    );
    let line = status(s.home.path(), &["group"], crate::db::now_ms()).unwrap();
    assert_eq!(
        line,
        json!({"state": "waiting", "why": "budget", "until": now + MINUTE})
    );
}

/// #724: a job whose subjects are all due later, or all end with no call, waits on nothing,
/// whatever row an earlier pass left; one with a subject due now that an overdue pass did not
/// reach keeps its row.
#[test]
fn a_job_that_holds_nothing_now_leaves_no_row() {
    let (mut s, seq) = curating();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let asked = RefCell::new(Vec::new());
    let rows = || crate::providers_db::my_work_rounds(&db).unwrap().len();
    let mut case = |subjects: Vec<(&'static str, i64)>, overdue: bool| {
        let now = crate::db::now_ms();
        crate::providers_db::set_my_work_round(&db, "group", "budget", now + 60 * MINUTE).unwrap();
        let job = Echo { subjects, seq };
        pass(&mut s, job, (true, 100), overdue, &grouped, &asked);
        rows()
    };
    let later = crate::db::now_ms() + 30 * MINUTE;
    assert_eq!(case(vec![("r1", later)], false), 0, "due later");
    assert_eq!(case(vec![("quiet1", 0)], false), 0, "nothing to send");
    assert_eq!(case(vec![("blank1", 0)], false), 0, "an empty prompt");
    let fresh = crate::db::now_ms() - MINUTE;
    assert_eq!(
        case(vec![("quiet2", 0), ("r9", fresh)], true),
        1,
        "a subject due now the overdue pass did not reach"
    );
    assert!(asked.borrow().is_empty());
}

/// One pass as `pass` makes it, with the refresh request the home holds (decision 70), and
/// `meanwhile` run after the round read it.
fn pass_refreshing(
    s: &mut Store,
    job: Echo,
    overdue: bool,
    asked: &RefCell<Vec<String>>,
    meanwhile: &dyn Fn(&std::path::Path),
) -> Phase {
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let (rules, summary) = (Rules::load(s.home.path()).unwrap(), Summary::default());
    let call = |span: &str, _: &str, _: &Value, _: &AnswerCheck, _: &Gate| -> Result<ChainResult> {
        asked.borrow_mut().push(span.to_owned());
        Ok(ChainResult {
            provider: "fake".into(),
            output: json!({"text": "Grouped."}),
            tier: 1,
        })
    };
    let mut runner = Runner::new(
        (true, 100),
        vec![Box::new(job) as Box<dyn Job>],
        Box::new(call),
        (&rules, &summary, "chain"),
    )
    .refresh(s.home.path());
    meanwhile(s.home.path());
    let phase = runner.pass(&mut s.raw, &k, &db, overdue).unwrap();
    s.run();
    phase
}

/// The page's request, as `request_refresh` leaves it, `ago` before now.
fn requested(home: &std::path::Path, ago: i64) {
    let path = refresh_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let at = std::time::SystemTime::now()
        - std::time::Duration::from_millis(u64::try_from(ago).unwrap());
    file.set_modified(at).unwrap();
}

/// Decision 70 (Refresh now): with a request standing, the pass before the windows takes a
/// subject due minutes ago, as it takes one due an hour; once none is due now, a pass marks the
/// request done, and a request the page left meanwhile stands.
#[test]
fn a_refresh_request_takes_a_job_due_now_before_a_window() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let now = crate::db::now_ms();
    let asked = RefCell::new(Vec::new());
    let echo = || Echo {
        subjects: vec![("r1", now - 5 * MINUTE)],
        seq,
    };
    let none = |_: &std::path::Path| ();
    let early = pass_refreshing(&mut s, echo(), true, &asked, &none);
    assert_eq!(
        (early, asked.borrow().len()),
        (Phase::Idle, 0),
        "no request"
    );
    requested(s.home.path(), 2_000);
    let early = pass_refreshing(&mut s, echo(), true, &asked, &none);
    assert_eq!(
        (early, asked.borrow().len()),
        (Phase::Covered, 1),
        "a request"
    );
    let home = s.home.path().to_path_buf();
    let standing = || refresh_request(&home, crate::db::now_ms());
    assert!(standing().is_some(), "one call a round");
    // The round finds none due now and marks the request it read done; the page's newer one,
    // left after the round read the first, stands.
    let left = |home: &std::path::Path| requested(home, 0);
    let done = pass_refreshing(&mut s, echo(), true, &asked, &left);
    assert_eq!((done, asked.borrow().len()), (Phase::Idle, 1));
    assert!(standing().is_some(), "the newer request stands");
    let done = pass_refreshing(&mut s, echo(), true, &asked, &none);
    assert_eq!((done, asked.borrow().len()), (Phase::Idle, 1));
    assert!(standing().is_none(), "nothing due now: done");
}

/// Decision 70: a request dated more than a minute ahead, after the clock was set back, stands
/// not, so that it does not wake the worker at every look.
#[test]
fn a_request_from_ahead_of_the_clock_stands_not() {
    let home = tempfile::tempdir().unwrap();
    let path = refresh_path(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(3_600);
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(ahead).unwrap();
    assert_eq!(refresh_request(home.path(), crate::db::now_ms()), None);
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        refresh_request(home.path(), crate::db::now_ms()).is_some(),
        "a few seconds ahead"
    );
    // A request finished at noon, the clock set back an hour, then a new request: it stands.
    let done = std::fs::File::create(refresh_done_path(home.path())).unwrap();
    done.set_modified(ahead).unwrap();
    file.set_modified(std::time::SystemTime::now()).unwrap();
    assert!(
        refresh_request(home.path(), crate::db::now_ms()).is_some(),
        "a done mark ahead"
    );
}

/// Decision 70: a request `REFRESH_MS` old stands no more and changes nothing.
#[test]
fn a_stale_refresh_request_changes_nothing() {
    let mut s = Store::new();
    let seq = s.said("s1", "r", 1_000, "Build the parser.");
    let now = crate::db::now_ms();
    let asked = RefCell::new(Vec::new());
    requested(s.home.path(), REFRESH_MS + MINUTE);
    let job = Echo {
        subjects: vec![("r1", now - 5 * MINUTE)],
        seq,
    };
    let early = pass_refreshing(&mut s, job, true, &asked, &|_| ());
    assert_eq!((early, asked.borrow().len()), (Phase::Idle, 0));
    assert_eq!(refresh_request(s.home.path(), crate::db::now_ms()), None);
}

/// Decision 70, the page's side: off, waiting on a hold, nothing due and some due, each with
/// the request left only for the last.
#[test]
fn a_refresh_is_requested_only_when_something_is_due_and_can_run() {
    let request = |s: &Store, subjects: Vec<(&'static str, i64)>, seq: i64| {
        let jobs = vec![Box::new(Echo { subjects, seq }) as Box<dyn Job>];
        refresh_with(s.home.path(), crate::db::now_ms(), jobs, &["group"]).unwrap()
    };
    let now = crate::db::now_ms();
    let mut off = Store::new();
    let seq = off.said("s1", "r", 1_000, "Build the parser.");
    assert_eq!(
        request(&off, vec![("r1", now)], seq),
        json!({"state": "off"})
    );
    assert!(!refresh_path(off.home.path()).exists());

    let (s, seq) = curating();
    let later = crate::db::now_ms() + 30 * MINUTE;
    assert_eq!(
        request(&s, vec![("r1", later)], seq),
        json!({"state": "nothing_due"})
    );
    assert!(!refresh_path(s.home.path()).exists());
    let db = crate::providers_db::open(s.home.path()).unwrap();
    let until = crate::db::now_ms() + 60 * MINUTE;
    crate::providers_db::set_my_work_round(&db, "group", "budget", until).unwrap();
    let line = request(&s, vec![("r1", now - MINUTE)], seq);
    assert_eq!(line["state"], "waiting", "{line}");
    assert!(!refresh_path(s.home.path()).exists());
    crate::providers_db::clear_my_work_round(&db, "group").unwrap();
    // A hold of a job My Work's line does not speak for (My Rules' or My Goals') stops none.
    crate::providers_db::set_my_work_round(&db, "rules", "budget", until).unwrap();
    assert_eq!(
        request(
            &s,
            vec![("r1", now - MINUTE), ("r2", now - 2 * MINUTE)],
            seq
        ),
        json!({"state": "started", "due": 2})
    );
    assert!(refresh_request(s.home.path(), crate::db::now_ms()).is_some());
    drop(off);
}
