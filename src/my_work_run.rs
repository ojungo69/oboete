//! My Work's job runner (docs/my-work-model.md A1 to A8): a due check that reads the stores and
//! sends nothing, at most one call a round in A3's place, the egress gate before each send (A5),
//! the daily bound (A7), and the waits and the give-up of a job every entry fails (A8). The jobs
//! say when they are due, what they send and what they keep (`Job`); the runner sends and writes.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::config::Summary;
use crate::curate::Phase;
use crate::my_work_ops::{MyWorkOp, Read, fitted};
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Gate};
use crate::raw::{OpKind, Raw, Removal};
use crate::redact::Rules;

/// A job due this long goes before a window (A3).
const OVERDUE_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The chain as the role `my_work` (A1): a job's span, prompt, schema, check and gate, to an
/// answer.
pub type Caller<'a> =
    dyn FnMut(&str, &str, &Value, &AnswerCheck, &Gate) -> Result<ChainResult> + 'a;

/// My Work's pass of a curation round (A3): `true` asks only for a job due an hour or more.
pub type Pass<'a> = dyn FnMut(&mut Raw, &Connection, &Connection, bool) -> Result<Phase> + 'a;

/// What a job reads, all of it read-only.
// shortcut: no job reads every store until grouping (slice 3a-2), which drops this allow.
#[allow(dead_code)]
pub(crate) struct Stores<'a> {
    pub raw: &'a Raw,
    pub k: &'a Connection,
    pub rules: &'a Rules,
    pub summary: &'a Summary,
    /// The exclusion list as the pass read it, which the gate holds every send to (A5, S6).
    pub reading: &'a crate::curate::Reading,
}

/// A job's subject that is due, and since when, unix ms (A3): a time to come is when it will be.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Due {
    pub about: String,
    pub since: i64,
}

/// What a job keeps of an answer that passed: its fields, by the job's names.
pub(crate) type Keep = dyn Fn(&Value) -> BTreeMap<String, Value>;

/// What a job sends for one subject, and how it takes the answer.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Ask {
    /// Its fixed instructions, then the data between fence lines (A10), every piece of recorded
    /// text gated alone before it is cut (A5).
    pub prompt: String,
    pub schema: Value,
    /// What it shows a model (S2), kept in the op, a skip's too.
    pub read: Read,
    pub removed: Vec<Removal>,
    /// The answer's check (A10): the outcome that refuses it, none when it passes.
    pub check: Box<AnswerCheck<'static>>,
    /// The fields kept of an answer that passed, each gated again, one over its cap dropped (A5).
    pub keep: Box<Keep>,
}

/// One of My Work's jobs (S1).
pub(crate) trait Job {
    /// The op's `job`, and the ledger span's first word.
    fn name(&self) -> &'static str;
    /// Its subjects due now: a read of the stores, no call.
    fn due(&self, s: &Stores, now: i64) -> Result<Vec<Due>>;
    /// What it sends for `about` now; none when nothing is left to send.
    fn ask(&self, s: &Stores, about: &str, now: i64) -> Result<Option<Ask>>;
}

/// The jobs this binary runs, for one round: none until the first, grouping (slice 3a-2).
pub(crate) fn jobs() -> Vec<Box<dyn Job>> {
    Vec::new()
}

/// One round's runner: the switches and bound as the worker read them, the jobs, and the chain.
pub(crate) struct Runner<'a> {
    /// `[my_work] curate` (A6); the worker reaches a round only while `[summary] curate` is on
    /// (A1).
    on: bool,
    calls_a_day: u32,
    jobs: Vec<Box<dyn Job + 'a>>,
    call: Box<Caller<'a>>,
    rules: &'a Rules,
    summary: &'a Summary,
    /// Who is asked and within what caps: a job held under other ones is tried again now.
    chain: &'a str,
    /// The phase of the call this round made, if it made one: a round makes one at most.
    called: Option<Phase>,
}

impl<'a> Runner<'a> {
    pub(crate) fn new(
        (on, calls_a_day): (bool, u32),
        jobs: Vec<Box<dyn Job + 'a>>,
        call: Box<Caller<'a>>,
        (rules, summary, chain): (&'a Rules, &'a Summary, &'a str),
    ) -> Self {
        Self {
            on,
            calls_a_day,
            jobs,
            call,
            rules,
            summary,
            chain,
            called: None,
        }
    }

    /// One pass (A3): the job due the longest, of those due an hour or more when `overdue`; at
    /// most one call a round, whatever it answered.
    pub(crate) fn pass(
        &mut self,
        raw: &mut Raw,
        k: &Connection,
        db: &Connection,
        overdue: bool,
    ) -> Result<Phase> {
        if let Some(phase) = self.called {
            return Ok(phase);
        }
        if !self.on || self.jobs.is_empty() {
            return Ok(Phase::Idle);
        }
        let now = crate::db::now_ms();
        // Milestone 5 D1 rule 12: what a job shows is read once the consumers reach a forget.
        if crate::curate::lagging(raw, k)? {
            return Ok(Phase::Waiting {
                until: now,
                up: true,
            });
        }
        let reading = crate::curate::Reading::now(raw, crate::curate::Reads::Live)?;
        let stores = Stores {
            raw,
            k,
            rules: self.rules,
            summary: self.summary,
            reading: &reading,
        };
        let mut due = Vec::new();
        for (i, job) in self.jobs.iter().enumerate() {
            for d in job.due(&stores, now)? {
                due.push((d.since, i, d.about));
            }
        }
        // A subject due later (a repository asked about minutes ago) is waited for, the worker
        // staying up for it as for a window (D10).
        let mut wait = Phase::Idle;
        due.retain(|&(since, ..)| {
            let later = since > now;
            if later {
                let until = Phase::Waiting {
                    until: since,
                    up: since - now <= crate::curate::STAY_UP_MS,
                };
                wait = crate::turns::sooner(wait, until);
            }
            !later && (!overdue || now - since >= OVERDUE_MS)
        });
        if due.is_empty() {
            return Ok(wait);
        }
        // A7: at the bound every job waits, held for budget, until the oldest call counted leaves
        // the rolling day.
        let (calls, oldest) = crate::providers_db::my_work_calls_in_a_day(db)?;
        if calls >= self.calls_a_day {
            return Ok(Phase::Waiting {
                until: oldest.map_or(now, |t| t + DAY_MS),
                up: false,
            });
        }
        due.sort();
        for (_, i, about) in due {
            let name = self.jobs[i].name();
            let stores = Stores {
                raw,
                k,
                rules: self.rules,
                summary: self.summary,
                reading: &reading,
            };
            let Some(ask) = self.jobs[i].ask(&stores, &about, now)? else {
                continue;
            };
            let span = format!("{name} {about}");
            let skip = MyWorkOp {
                job: name.into(),
                about: about.clone(),
                entry: String::new(),
                ts: now,
                read: ask.read.clone(),
                removed: ask.removed.clone(),
                fields: BTreeMap::new(),
                skipped: true,
                forgotten: None,
            };
            // What is kept of an answer at the least, a skip: a job whose labels pass the op cap
            // even so is not asked, as nothing paid for could be kept (S2).
            let Some(skip) = fitted(skip)? else {
                continue;
            };
            let sent = crate::curate::sha256_hex(&format!("{}\n{}", self.chain, ask.prompt));
            let pending =
                crate::providers_db::my_work_pending_of(db, &span)?.filter(|p| p.prompt == sent);
            if let Some(p) = &pending {
                if p.attempts >= crate::curate::ATTEMPTS {
                    return given_up(raw, db, &span, skip);
                }
                if p.next_attempt_at > now {
                    wait = crate::turns::sooner(
                        wait,
                        crate::turns::held(&p.hold, p.next_attempt_at, now),
                    );
                    continue;
                }
            }
            let answer = (self.call)(&span, &ask.prompt, &ask.schema, &*ask.check, &|| {
                let dispatch = raw.dispatch()?;
                reading.still(raw)?;
                crate::curate::rules_still(raw, self.rules)?;
                Ok(Some(dispatch))
            })
            .and_then(|res| {
                let op = MyWorkOp {
                    job: name.into(),
                    about: about.clone(),
                    entry: res.provider,
                    ts: crate::db::now_ms(),
                    read: ask.read.clone(),
                    removed: ask.removed.clone(),
                    fields: (ask.keep)(&res.output),
                    skipped: false,
                    forgotten: None,
                };
                let op = fitted(op)?.unwrap_or_else(|| skip.clone());
                // A forget after the job read its records keeps none of the answer.
                raw.append_ops_fenced(&[(OpKind::MyWork, op)], reading.fence)?;
                Ok(())
            });
            let failed = match answer {
                Ok(()) => {
                    crate::providers_db::clear_my_work_pending(db, &span)?;
                    return Ok(self.called(Phase::Covered));
                }
                // Nothing went out: the next pass reads the list again.
                Err(e) if e.is::<crate::curate::ListChanged>() => {
                    return Ok(self.called(Phase::Waiting {
                        until: now,
                        up: true,
                    }));
                }
                Err(e) => e.downcast::<ChainFailed>()?.0,
            };
            // A slow call may set its reset after the pass started: judge the wait now.
            let after = crate::db::now_ms();
            let (hold, next, counted) = crate::curate::hold(&failed, after);
            let p = crate::providers_db::MyWorkPending {
                key: span.clone(),
                prompt: sent,
                reason: ChainFailed(failed).to_string(),
                hold: hold.into(),
                attempts: pending.map_or(0, |p| p.attempts) + i64::from(counted),
                next_attempt_at: next,
            };
            if p.attempts >= crate::curate::ATTEMPTS {
                return given_up(raw, db, &span, skip).map(|phase| self.called(phase));
            }
            crate::providers_db::set_my_work_pending(db, &p)?;
            return Ok(self.called(crate::turns::held(hold, next, after)));
        }
        Ok(wait)
    }

    fn called(&mut self, phase: Phase) -> Phase {
        self.called = Some(phase);
        phase
    }
}

/// A job every entry failed `ATTEMPTS` times is kept as a skip of what it read, and not asked
/// again until what it would read changes (A8).
fn given_up(raw: &mut Raw, db: &Connection, span: &str, skip: Value) -> Result<Phase> {
    raw.append_ops(&[(OpKind::MyWork, skip)])?;
    crate::providers_db::clear_my_work_pending(db, span)?;
    Ok(Phase::Covered)
}

#[cfg(test)]
mod tests {
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

        fn ask(&self, _: &Stores, about: &str, _: i64) -> Result<Option<Ask>> {
            Ok(Some(Ask {
                prompt: format!("Tell me about {about}."),
                schema: json!({"type": "object"}),
                read: Read {
                    spans: vec![(self.seq, self.seq)],
                    ops: vec![],
                },
                removed: vec![],
                check: Box::new(|v: &Value| (!v["text"].is_string()).then_some("shape")),
                keep: Box::new(|v: &Value| {
                    BTreeMap::from([("text".to_owned(), v["text"].clone())])
                }),
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
        let call = |span: &str,
                    _: &str,
                    _: &Value,
                    check: &AnswerCheck,
                    gate: &Gate|
         -> Result<ChainResult> {
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
        let call =
            |span: &str, _: &str, _: &Value, _: &AnswerCheck, _: &Gate| -> Result<ChainResult> {
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
}
