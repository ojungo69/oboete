//! My Work's job runner (docs/my-work-model.md A1 to A8): a due check that reads the stores and
//! sends nothing, at most one call a round in A3's place, the egress gate before each send (A5),
//! the daily bound (A7), and the waits and the give-up of a job every entry fails (A8). The jobs
//! say when they are due, what they send and what they keep (`Job`); the runner sends and writes.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::config::Summary;
use crate::curate::Phase;
use crate::my_work_ops::{MyWorkOp, Read, fitted};
use crate::provider::{AnswerCheck, ChainFailed, ChainResult, Gate};
use crate::raw::{OpKind, Raw, Removal};
use crate::redact::Rules;

/// A job due this long goes before a window (A3).
const OVERDUE_MS: i64 = 60 * 60 * 1000;

/// Refresh now (decision 70, parity row X14): how long a request the page leaves for the worker
/// stands. While it stands, the pass before the windows takes every subject due now.
pub(crate) const REFRESH_MS: i64 = 30 * 60 * 1000;

/// Where the page leaves its refresh request for the worker; the request's time is the file's.
pub(crate) fn refresh_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("state").join("my-work-refresh")
}

/// Where the worker marks the last request it finished, by the file's time. Only the page writes a
/// request and only the worker marks one done: neither removes what the other wrote, so a request
/// the page leaves while the worker finishes one stands.
fn refresh_done_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("state").join("my-work-refresh-done")
}

fn modified_ms(path: &std::path::Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(at.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// The refresh request standing at `now`, by its time (unix ms): none once `REFRESH_MS` old, once
/// marked done, or dated more than a minute ahead (a clock set back since, which would otherwise
/// wake the worker at every look until it caught up). A done mark dated that far ahead marks
/// nothing done, so that a request after the clock went back stands.
pub(crate) fn refresh_request(home: &std::path::Path, now: i64) -> Option<i64> {
    let ahead = |t: i64| t - now > 60_000;
    let at = modified_ms(&refresh_path(home))?;
    if now - at >= REFRESH_MS || ahead(at) {
        return None;
    }
    if modified_ms(&refresh_done_path(home)).is_some_and(|done| done >= at && !ahead(done)) {
        return None;
    }
    Some(at)
}
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The chain as the role `my_work` (A1): a job's span, prompt, schema, check and gate, to an
/// answer.
pub type Caller<'a> =
    dyn FnMut(&str, &str, &Value, &AnswerCheck, &Gate) -> Result<ChainResult> + 'a;

/// The judge (J1) as the runner asks it for a job (J3): a state and its questions, to its answer,
/// through the runner's egress gate; with its name in the ledger, each decision op's entry, and
/// its name in a decision's evidence (J7: where it sends).
pub(crate) struct Judging<'a> {
    pub entry: &'static str,
    pub label: &'static str,
    pub ask: &'a mut dyn FnMut(crate::judge::Ask) -> Result<crate::judge::Asked>,
}

/// My Work's pass of a curation round (A3): `true` asks only for a job due an hour or more.
pub type Pass<'a> = dyn FnMut(&mut Raw, &Connection, &Connection, bool) -> Result<Phase> + 'a;

/// What a job reads, all of it read-only.
pub(crate) struct Stores<'a> {
    pub raw: &'a Raw,
    pub k: &'a Connection,
    pub rules: &'a Rules,
    pub summary: &'a Summary,
    /// The exclusion list as the pass read it, which the gate holds every send to (A5, S6).
    pub reading: &'a crate::curate::Reading,
    /// #730: each job and subject a failed batch holds, which the job leaves out of its batches.
    pub held: &'a BTreeSet<(String, String)>,
}

/// No subject held (#730), as a `Stores` built outside a round has it.
#[cfg(test)]
pub(crate) static NOT_HELD: BTreeSet<(String, String)> = BTreeSet::new();

/// A job's subject that is due, and since when, unix ms (A3): a time to come is when it will be.
pub(crate) struct Due {
    pub about: String,
    pub since: i64,
}

/// What a job keeps of an answer that passed: its fields, by the job's names.
pub(crate) type Keep = dyn Fn(&Value) -> BTreeMap<String, Value>;

/// What a job sends for one subject, and how it takes the answer.
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
    /// The subjects one answer is kept for, `about` among them (L1's batch): each gets an op of
    /// its own with the call's whole `read`, its fields what `keep` gives under its name. Empty:
    /// `about` alone, its fields all `keep` gives.
    pub subjects: Vec<String>,
}

/// One of My Work's jobs (S1).
pub(crate) trait Job {
    /// The op's `job`, and the ledger span's first word.
    fn name(&self) -> &'static str;
    /// Its subjects due now: a read of the stores, no call.
    fn due(&self, s: &Stores, now: i64) -> Result<Vec<Due>>;
    /// What it sends for `about` now; none when nothing is left to send.
    fn ask(&self, s: &Stores, about: &str, now: i64) -> Result<Option<Ask>>;
    /// J3: what it decides with the judge, when the judge is on, before the round's call of the
    /// chain: the ops to write, none when it decides nothing now. A method of the job, not a step
    /// of the runner's, as the job's own read for the round (grouping's conversations and
    /// requests) says what is left to judge, read once a round and not again.
    fn judged(&self, _s: &Stores, _judge: &mut Judging, _now: i64) -> Result<Vec<MyWorkOp>> {
        Ok(Vec::new())
    }
    /// The list the job read under is no longer the store's (an exclusion, a forget, between a
    /// round's two passes): what it kept of its read for the round is dropped, and its next `due`
    /// or `ask` reads again. Every job says so itself: one that keeps a read and did not drop it
    /// would send, under the newer list, what it read under the older one.
    fn reread(&self);
}

/// A request past `[my_work] calls_a_day`, a retry's or a fallback's too (A7): nothing is sent,
/// and the job waits until the oldest call counted leaves the rolling day, unix ms.
#[derive(Debug)]
pub(crate) struct BoundReached(pub i64);

impl std::fmt::Display for BoundReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("My Work's calls a day are spent")
    }
}

impl std::error::Error for BoundReached {}

/// The jobs this binary runs, for one round.
/// A job's prompt: its instructions, then `data` between two fence lines named by `tag` and by
/// the data's digest, so the data cannot contain them (spec 3.3: recorded text is data).
pub(crate) fn fenced(tag: &str, instructions: &str, data: &str) -> String {
    let fence = format!("=== {tag} {} ===", &crate::curate::sha256_hex(data)[..16]);
    let sentence = crate::curate::FENCE_SENTENCE;
    format!("{instructions}\n{sentence}{fence}`.\n\n{fence}\n{data}{fence}")
}

/// A job's answer object (A10): every key of `properties` required, nothing more.
pub(crate) fn strict(properties: Value) -> Value {
    let required: Vec<String> = (properties.as_object().into_iter())
        .flat_map(|m| m.keys().cloned())
        .collect();
    json!({"type": "object", "additionalProperties": false,
        "required": required, "properties": properties})
}

pub(crate) fn jobs() -> Vec<Box<dyn Job>> {
    vec![
        Box::new(crate::my_work_group::Grouping::default()),
        Box::new(crate::my_work_progress::Progress::default()),
        Box::new(crate::my_work_place::Place::default()),
        Box::new(crate::my_work_story::Story::default()),
        Box::new(crate::my_work_rules::Suggestions::default()),
        Box::new(crate::my_work_goals::OwnerGoals::default()),
    ]
}

/// V1: the jobs whose state My Work's status line says; the goals and the rules jobs have lines of
/// their own (My Goals', My Rules').
pub(crate) fn line_jobs() -> Vec<&'static str> {
    (jobs().iter())
        .map(|j| j.name())
        .filter(|&n| ![crate::my_work_goals::NAME, crate::my_work_rules::NAME].contains(&n))
        .collect()
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
    /// J1: the judge and the month's paid cap, as the worker read them; none without `[judge]`.
    judge: Option<(&'a crate::config::Judge, f64)>,
    /// Decision 70: the refresh request standing as the round began, by its time, and where its
    /// end is marked.
    refresh: Option<(std::path::PathBuf, i64)>,
    /// The list the pass before this one read under, which the jobs' kept reads were made by.
    read_under: Option<crate::curate::Reading>,
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
            judge: None,
            refresh: None,
            read_under: None,
        }
    }

    /// Decision 70 (Refresh now): with a request standing in `home`, the pass before the windows
    /// takes every subject due now, not only those due an hour or more; a pass that finds none due
    /// now marks the request done.
    pub(crate) fn refresh(mut self, home: &std::path::Path) -> Self {
        self.refresh =
            refresh_request(home, crate::db::now_ms()).map(|at| (refresh_done_path(home), at));
        self
    }

    /// J3: the judge the jobs ask before the chain, with the month's paid cap.
    pub(crate) fn judge(mut self, judge: Option<(&'a crate::config::Judge, f64)>) -> Self {
        self.judge = judge;
        self
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
        // A5: a job reads once a round and keeps it, and a round has two passes with the windows'
        // calls between them. This pass's gate holds the list as it is now, so what a job kept
        // under an older one (before an exclusion, a forget) would pass it: the jobs read again.
        if self.read_under.as_ref().is_some_and(|was| *was != reading) {
            for job in &self.jobs {
                job.reread();
            }
        }
        self.read_under = Some(reading.clone());
        // #730: the subjects a failed batch holds, each by its own wait; read before the judge's
        // pass too, as a job's read for the round (placement's) is kept for the chain's (J5).
        let holds = crate::providers_db::my_work_held(db, now)?;
        let held_by: BTreeSet<(String, String)> = (holds.iter())
            .map(|(job, about, ..)| (job.clone(), about.clone()))
            .collect();
        // J3: with the judge on, the jobs decide what they can before the chain; a round that
        // decided ends there, and the next reads what the consumers applied of it.
        if let Some(phase) = self.judged(raw, k, db, (&reading, &held_by), now)? {
            return Ok(self.called(phase));
        }
        let stores = Stores {
            raw,
            k,
            rules: self.rules,
            summary: self.summary,
            reading: &reading,
            held: &held_by,
        };
        let mut due = Vec::new();
        for (i, job) in self.jobs.iter().enumerate() {
            for d in job.due(&stores, now)? {
                due.push((d.since, i, d.about));
            }
        }
        // V1 (#720, #724): a job's row says what it waits on. Its subjects due now, those an
        // overdue pass leaves too, are counted down as the loop passes them: a job none of whose
        // subjects is due now waits on nothing, nor does one whose last ends with no call while
        // none was held.
        let mut left = vec![0usize; self.jobs.len()];
        for d in due.iter().filter(|d| d.0 <= now) {
            left[d.1] += 1;
        }
        for (i, job) in self.jobs.iter().enumerate() {
            if left[i] == 0 {
                crate::providers_db::clear_my_work_round(db, job.name())?;
            }
        }
        // Each job's earliest hold this pass, the one `turns::sooner` waits for.
        let mut held: Vec<Option<i64>> = vec![None; self.jobs.len()];
        // A subject due later (a repository asked about minutes ago) is waited for, the worker
        // staying up for it as for a window (D10).
        let mut wait = Phase::Idle;
        // #730: a job whose batch is held waits on its earliest subject, though none is due.
        for (job, _, hold, until) in &holds {
            let Some(i) = self.jobs.iter().position(|j| j.name() == job) else {
                continue;
            };
            if held[i].is_none_or(|u| *until < u) {
                crate::providers_db::set_my_work_round(db, job, hold, *until)?;
                held[i] = Some(*until);
            }
            wait = crate::turns::sooner(wait, crate::turns::held(hold, *until, now));
        }
        // Decision 70: a refresh request stands until no subject is due now, the request this
        // round read marked done by its time, so that one the page left since stands.
        if !due.iter().any(|d| d.0 <= now)
            && let Some((done, at)) = self.refresh.take()
        {
            let at = std::time::UNIX_EPOCH
                + std::time::Duration::from_millis(u64::try_from(at).unwrap_or(0));
            let _ = std::fs::File::create(done).and_then(|f| f.set_modified(at));
        }
        let refreshing = self.refresh.is_some();
        due.retain(|&(since, ..)| {
            let later = since > now;
            if later {
                let until = Phase::Waiting {
                    until: since,
                    up: since - now <= crate::curate::STAY_UP_MS,
                };
                wait = crate::turns::sooner(wait, until);
            }
            !later && (!overdue || refreshing || now - since >= OVERDUE_MS)
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
            left[i] -= 1;
            let last = left[i] == 0 && held[i].is_none();
            let passed_over = || -> Result<()> {
                if last {
                    crate::providers_db::clear_my_work_round(db, name)?;
                }
                Ok(())
            };
            let stores = Stores {
                raw,
                k,
                rules: self.rules,
                summary: self.summary,
                reading: &reading,
                held: &held_by,
            };
            let Some(ask) = self.jobs[i].ask(&stores, &about, now)? else {
                passed_over()?;
                continue;
            };
            let span = format!("{name} {about}");
            let subjects = if ask.subjects.is_empty() {
                vec![about.clone()]
            } else {
                ask.subjects.clone()
            };
            // #730: a batch's subjects each wait as their batch does, whatever batch comes next.
            let batch = if ask.subjects.is_empty() {
                &[][..]
            } else {
                &subjects[..]
            };
            let op = |about: &str, fields, entry, ts, skipped| MyWorkOp {
                job: name.into(),
                about: about.to_owned(),
                entry,
                ts,
                read: ask.read.clone(),
                removed: ask.removed.clone(),
                fields,
                skipped,
                forgotten: None,
            };
            // What is kept of an answer at the least, a skip of each subject: a job whose labels
            // pass the op cap even so is not asked, as nothing paid for could be kept (S2).
            let skips: Option<Vec<(OpKind, Value)>> = (subjects.iter())
                .map(|s| {
                    let skip = fitted(op(s, BTreeMap::new(), String::new(), now, true))?;
                    Ok(skip.map(|v| (OpKind::MyWork, v)))
                })
                .collect::<Result<_>>()?;
            let Some(skips) = skips else {
                passed_over()?;
                continue;
            };
            // Nothing to send (each prompt the goals would show was an agent's): what it read is
            // kept as a skip with no call, so the job is not due again for it (M2).
            if ask.prompt.is_empty() {
                raw.append_ops_fenced(&skips, reading.fence)?;
                passed_over()?;
                continue;
            }
            let sent = crate::curate::sha256_hex(&format!("{}\n{}", self.chain, ask.prompt));
            let pending =
                crate::providers_db::my_work_pending_of(db, &span)?.filter(|p| p.prompt == sent);
            if let Some(p) = &pending {
                if p.attempts >= crate::curate::ATTEMPTS {
                    for s in batch {
                        crate::providers_db::clear_my_work_held(db, (name, s))?;
                    }
                    return given_up(raw, db, (name, &span), &skips, now);
                }
                if p.next_attempt_at > now {
                    if held[i].is_none_or(|until| p.next_attempt_at < until) {
                        crate::providers_db::set_my_work_round(
                            db,
                            name,
                            &p.hold,
                            p.next_attempt_at,
                        )?;
                        held[i] = Some(p.next_attempt_at);
                    }
                    wait = crate::turns::sooner(
                        wait,
                        crate::turns::held(&p.hold, p.next_attempt_at, now),
                    );
                    continue;
                }
            }
            let bound = self.calls_a_day;
            let answer = (self.call)(&span, &ask.prompt, &ask.schema, &*ask.check, &|| {
                // A7 at each request, the chain's retries and fallbacks too (Codex on #668): the
                // count holds this request's own reservation.
                let (calls, oldest) = crate::providers_db::my_work_calls_in_a_day(db)?;
                if calls > bound {
                    let until = oldest.map_or_else(crate::db::now_ms, |t| t + DAY_MS);
                    return Err(BoundReached(until).into());
                }
                let dispatch = raw.dispatch()?;
                reading.still(raw)?;
                crate::curate::rules_still(raw, self.rules)?;
                Ok(Some(dispatch))
            })
            .and_then(|res| {
                let kept = (ask.keep)(&res.output);
                let ts = crate::db::now_ms();
                let mut ops = Vec::new();
                for (subject, skip) in subjects.iter().zip(&skips) {
                    let fields = if ask.subjects.is_empty() {
                        kept.clone()
                    } else {
                        (kept.get(subject).and_then(Value::as_object).cloned())
                            .map(|o| o.into_iter().collect())
                            .unwrap_or_default()
                    };
                    let answered = op(subject, fields, res.provider.clone(), ts, false);
                    let answered = fitted(answered)?.map(|v| (OpKind::MyWork, v));
                    ops.push(answered.unwrap_or_else(|| skip.clone()));
                }
                // A subject keep gives beyond those the job waits on gets an op too, one that does
                // not fit none (the rules job's suggestions, #737): links and the story give
                // exactly theirs.
                if !ask.subjects.is_empty() {
                    for (subject, fields) in kept.iter().filter(|(s, _)| !subjects.contains(s)) {
                        let fields = (fields.as_object().cloned())
                            .map(|o| o.into_iter().collect())
                            .unwrap_or_default();
                        let answered = op(subject, fields, res.provider.clone(), ts, false);
                        ops.extend(fitted(answered)?.map(|v| (OpKind::MyWork, v)));
                    }
                }
                // A forget after the job read its records keeps none of the answer; the batch's
                // ops go in one append, all or none.
                raw.append_ops_fenced(&ops, reading.fence)?;
                Ok(())
            });
            let failed = match answer {
                Ok(()) => {
                    for s in batch {
                        crate::providers_db::clear_my_work_held(db, (name, s))?;
                    }
                    crate::providers_db::clear_my_work_pending(db, &span)?;
                    // #730: a job with a subject still held keeps saying it waits.
                    if held[i].is_none() {
                        crate::providers_db::clear_my_work_round(db, name)?;
                    }
                    return Ok(self.called(Phase::Covered));
                }
                // At the bound, nothing more went out: the job waits, its attempts as they were.
                Err(e) if e.is::<BoundReached>() => {
                    let until = e.downcast_ref::<BoundReached>().map_or(now, |b| b.0);
                    return Ok(self.called(Phase::Waiting { until, up: false }));
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
                for s in batch {
                    crate::providers_db::clear_my_work_held(db, (name, s))?;
                }
                return given_up(raw, db, (name, &span), &skips, now)
                    .map(|phase| self.called(phase));
            }
            for s in batch {
                crate::providers_db::set_my_work_held(db, (name, s), hold, next)?;
            }
            crate::providers_db::set_my_work_pending(db, &p)?;
            if held[i].is_none_or(|until| next < until) {
                crate::providers_db::set_my_work_round(db, name, hold, next)?;
            }
            // The runner wakes for the earliest of its waits, as the row names it.
            let held_now = crate::turns::held(hold, next, after);
            return Ok(self.called(crate::turns::sooner(wait, held_now)));
        }
        Ok(wait)
    }

    fn called(&mut self, phase: Phase) -> Phase {
        self.called = Some(phase);
        phase
    }

    /// J3: each job's decisions with the judge, when it is on (`[judge]` there, on, and its key
    /// read): the first job that decides gets its ops written, in one append, and the pass ends
    /// with the phase that says so; none when no job decides. The judge's calls are its own, under
    /// `[judge] daily_budget`, not A7's; the gate before each is the chain's otherwise (A5): forget
    /// ordered, and the exclusions and the rules as the pass read them; and the judge still on.
    fn judged(
        &self,
        raw: &mut Raw,
        k: &Connection,
        db: &Connection,
        (reading, held): (&crate::curate::Reading, &BTreeSet<(String, String)>),
        now: i64,
    ) -> Result<Option<Phase>> {
        let Some((judge, cap)) = self.judge.filter(|(j, _)| crate::judge::ready(j)) else {
            return Ok(None);
        };
        let rules = self.rules;
        for job in &self.jobs {
            let decided = {
                let raw = &*raw;
                let stores = Stores {
                    raw,
                    k,
                    rules,
                    summary: self.summary,
                    reading,
                    held,
                };
                let gate = || -> Result<Option<crate::dispatch::Guard>> {
                    let dispatch = raw.dispatch()?;
                    reading.still(raw)?;
                    crate::curate::rules_still(raw, rules)?;
                    // The switch as it is now: turned off since the round read it, nothing goes.
                    if !crate::judge::still_on(raw.home(), judge) {
                        return Err(crate::curate::ListChanged.into());
                    }
                    Ok(Some(dispatch))
                };
                let mut ask = |ask: crate::judge::Ask<'_>| {
                    crate::judge::ask(db, raw, rules, (judge, cap), ask, &gate)
                };
                let at = crate::judge::fixed(judge.destination);
                let mut judging = Judging {
                    entry: at.name,
                    label: at.label,
                    ask: &mut ask,
                };
                job.judged(&stores, &mut judging, now)
            };
            let ops = match decided {
                Ok(ops) => ops,
                // Nothing kept: the next pass reads the list again.
                Err(e) if e.is::<crate::curate::ListChanged>() => {
                    return Ok(Some(Phase::Waiting {
                        until: now,
                        up: true,
                    }));
                }
                Err(e) => return Err(e),
            };
            if ops.is_empty() {
                continue;
            }
            // And before anything is kept: a decision with no call, or a pick of none, needs no
            // request for the gate to stop.
            if !crate::judge::still_on(raw.home(), judge) {
                return Ok(Some(Phase::Waiting {
                    until: now,
                    up: true,
                }));
            }
            let mut kept = Vec::new();
            for op in ops {
                kept.extend(fitted(op)?.map(|v| (OpKind::MyWork, v)));
            }
            return match raw.append_ops_fenced(&kept, reading.fence) {
                Ok(_) => Ok(Some(Phase::Covered)),
                // A forget after the job read its records keeps none of it.
                Err(e) if e.is::<crate::curate::ListChanged>() => Ok(Some(Phase::Waiting {
                    until: now,
                    up: true,
                })),
                Err(e) => Err(e),
            };
        }
        Ok(None)
    }
}

/// Each job's call as the design's cost table counts it ("Cost on this PC's volume"): its
/// estimated tokens in and out.
const SIZES: [(&str, u32, u32); 7] = [
    ("group", 6_500, 300),
    ("review", 4_000, 1_000),
    ("links", 4_000, 300),
    ("place", 4_000, 300),
    ("story", 4_000, 1_700),
    ("rules", 2_000, 300),
    ("goals", 4_000, 2_000),
];
/// The days an estimate reads (A6).
const WEEK_MS: i64 = 7 * DAY_MS;
/// What each job's fixed instructions stay under, in estimated tokens (A4).
const INSTRUCTIONS: u32 = 1_500;
/// What an estimate counts a call with: its window of data (`[summary] window_tokens`) and each
/// entry's learned factor (`budget::factor`), as admission counts a call.
pub(crate) type Envelope<'a> = (u32, &'a dyn Fn(&str) -> Result<f64>);

/// What My Work's jobs would cost, from this home's last 7 days, before anything is sent (A6).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Estimate {
    /// Calls a day: typical, and at most (the daily bound).
    pub calls: (u32, u32),
    /// USD a day, typical and at most, at the first paid entry's prices; none when no entry is
    /// paid.
    pub usd: Option<(f64, f64)>,
    /// That entry's name and model.
    pub priced: Option<(String, String)>,
}

/// A6's estimate. Typical: each job's calls a day from the week's volumes (a repository's new
/// conversations of one half hour, one grouping call; a conversation's hours with prompts, one
/// review each, its links, placements, story and rules as the cost table derives them from the
/// reviews, the placements' Other pass once a day with prompts; a day with prompts, one goals
/// update), at most the bound, priced as the cost table sizes each job's
/// call. At most: the bound, each call the largest request A4 lets a job send (its instructions
/// and a window of data, `window_tokens`) and the entry's largest answer. Both count a call's input
/// as admission does, at the entry's learned `factor` (Codex on #671).
pub(crate) fn estimate(
    raw: &Raw,
    providers: &[crate::config::Provider],
    calls_a_day: u32,
    (window_tokens, factor): Envelope<'_>,
    now: i64,
) -> Result<Estimate> {
    let since = now - WEEK_MS;
    let halves: std::collections::HashSet<(Option<String>, i64)> =
        (raw.live_sessions()?.into_iter())
            .filter(|s| s.started >= since)
            .map(|s| (s.repo, s.started / (30 * 60_000)))
            .collect();
    let prompts = raw.prompt_times(since)?;
    let hours: std::collections::HashSet<(&(String, String), i64)> = prompts
        .iter()
        .map(|(ts, key)| (key, ts / 3_600_000))
        .collect();
    let days: std::collections::HashSet<i64> = prompts.iter().map(|(ts, _)| ts / DAY_MS).collect();
    let week = |n: usize| n as f64 / 7.0;
    let reviews = week(hours.len());
    let story = (reviews / 5.0).min(8.0);
    let days = week(days.len()).min(1.0);
    let per_job = [
        week(halves.len()),
        reviews,
        reviews / 5.0,
        reviews / 5.0 + days,
        story,
        story / 2.0,
        days,
    ];
    let typical: f64 = per_job.iter().sum();
    // Over the bound, every job waits alike (A7).
    let scale = if typical > f64::from(calls_a_day) {
        f64::from(calls_a_day) / typical
    } else {
        1.0
    };
    let calls = ((typical * scale).ceil() as u32, calls_a_day);
    let Some(p) = providers.iter().find(|p| p.limits().is_paid()) else {
        return Ok(Estimate {
            calls,
            usd: None,
            priced: None,
        });
    };
    let calibration = factor(p.name())?;
    let call = |input: u32, output: u32| {
        (p.limits()).usd(
            f64::from(input) * calibration * p.input_weight(crate::my_work::ROLE),
            f64::from(output),
        )
    };
    let usd: f64 = (per_job.iter().zip(SIZES))
        .map(|(n, (_, input, output))| n * scale * call(input, output))
        .sum();
    let table = SIZES.iter().map(|s| s.1).max().unwrap_or(0);
    let largest = table.max(INSTRUCTIONS + window_tokens);
    let most = f64::from(calls_a_day) * call(largest, crate::budget::largest_output(p));
    let model = match p {
        crate::config::Provider::Openai { model, .. } => model.clone(),
        crate::config::Provider::Cli { model, .. } => model.clone().unwrap_or_default(),
    };
    Ok(Estimate {
        calls,
        usd: Some((usd, most)),
        priced: Some((p.name().to_owned(), model)),
    })
}

/// A6's estimate for a home, over the chain as the worker builds it (an entry turned off is not
/// priced); none while nothing is recorded.
pub(crate) fn home_estimate(
    home: &std::path::Path,
    cfg: &crate::config::Config,
) -> Option<Result<Estimate>> {
    if !crate::raw::exists(home) {
        return None;
    }
    let chain: Vec<crate::config::Provider> = (cfg.providers.iter())
        .filter(|p| p.enabled() && !cfg.chain.turns_off(p.name()))
        .cloned()
        .collect();
    Some(crate::raw::open(home).and_then(|raw| {
        // The ledger read-only, as the page's ledger reads are (#680): a missing one, or an older
        // one without the columns a factor is learned from, is no learned factor yet.
        let db = crate::providers_db::read_only(home)?;
        let factor = |name: &str| {
            Ok(db
                .as_ref()
                .map_or(1.0, |db| crate::budget::factor(db, name).unwrap_or(1.0)))
        };
        let request: Envelope = (cfg.summary.window_tokens, &factor);
        estimate(
            &raw,
            &chain,
            cfg.my_work.calls_a_day,
            request,
            crate::db::now_ms(),
        )
    }))
}

/// Doctor's line on My Work (A6): the switch, and what turning it on would cost.
pub(crate) fn doctor_line(home: &std::path::Path, cfg: &crate::config::Config) -> String {
    let state = if cfg.my_work.curate && cfg.summary.curate {
        "on"
    } else if cfg.my_work.curate {
        "on, waiting for [summary] curate"
    } else {
        "off"
    };
    let e = match home_estimate(home, cfg) {
        Some(Ok(e)) => e,
        Some(Err(_)) => {
            return format!(
                "my work: {state} ([my_work] curate); no estimate: the store does not read"
            );
        }
        None => {
            return format!(
                "my work: {state} ([my_work] curate); no estimate: nothing is recorded yet"
            );
        }
    };
    // With no cap (decision 64) there is no most of calls or of USD to name.
    let capped = cfg.my_work.calls_a_day != crate::config::no_daily_cap();
    let usd = match (&e.usd, &e.priced) {
        (Some((typical, most)), Some((name, model))) if capped => format!(
            ", about USD {typical:.3} a day, at most USD {most:.2}, at {name}'s prices ({model})"
        ),
        (Some((typical, _)), Some((name, model))) => {
            format!(", about USD {typical:.3} a day, at {name}'s prices ({model})")
        }
        _ => ", no USD: no paid entry is in the chain".to_owned(),
    };
    let most = if capped {
        format!("at most {}", e.calls.1)
    } else {
        "no cap of calls a day".to_owned()
    };
    format!(
        "my work: {state} ([my_work] curate); calls a day with it on: about {}, {most}{usd}",
        e.calls.0
    )
}

/// V1: the status line of a page that shows `jobs`' ops, read with no call from config.toml,
/// providers.db and knowledge.db: off, or on and waiting for summaries; waiting at the daily
/// bound (A7), or on the hold the runner last left one of the jobs in (A8, #720: a budget, every
/// entry resting, the owner); else when the newest answered op of the jobs that this device wrote
/// and S6 shows was written, if one was.
pub(crate) fn status(home: &std::path::Path, jobs: &[&str], now: i64) -> Result<Value> {
    let cfg = crate::config::load(home)?;
    if !cfg.my_work.curate {
        return Ok(json!({"state": "off"}));
    }
    if !cfg.summary.curate {
        return Ok(json!({"state": "summaries_off"}));
    }
    if let Some(db) = crate::providers_db::read_only(home)? {
        // A ledger older than My Work holds nothing it waits on, as the page's other ledger reads
        // take it (#680).
        let (calls, oldest) = crate::providers_db::my_work_calls_in_a_day(&db).unwrap_or((0, None));
        if calls >= cfg.my_work.calls_a_day {
            return Ok(json!({"state": "waiting", "why": "bound",
                "until": oldest.map(|t| t + DAY_MS), "calls_a_day": cfg.my_work.calls_a_day}));
        }
        let held = (crate::providers_db::my_work_rounds(&db).unwrap_or_default())
            .into_iter()
            .filter(|(job, _, until)| *until > now && jobs.contains(&job.as_str()))
            .min_by_key(|(_, _, until)| *until);
        if let Some((_, hold, until)) = held {
            let why = match hold.as_str() {
                "budget" => "budget",
                "owner" => "owner",
                _ => "resting",
            };
            return Ok(json!({"state": "waiting", "why": why, "until": until}));
        }
    }
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok(json!({"state": "none"}));
    };
    let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live)?;
    let hidden = crate::my_work_ops::hidden(&raw, &k, &reading)?;
    let mut written = None;
    for job in jobs {
        let newest =
            crate::my_work_ops::shown(&k, (job, None), Some(raw.device()), &hidden, true, 1)?;
        written = written.max(newest.first().map(|o| o.op.ts));
    }
    Ok(match written {
        Some(at) => json!({"state": "written", "at": at}),
        None => json!({"state": "none"}),
    })
}

/// Refresh now (decision 70, parity row X14), the page's side: with no call, what the status line
/// would say when nothing can run (off, summaries off, or waiting at the daily bound or on a job's
/// hold, which a refresh does not lift); else how many subjects are due now, by every job's due
/// check as the runner makes it. With some, it leaves the request the worker takes at its next
/// round (`Runner::refresh`): `started` and their number; with none, `nothing_due`.
pub(crate) fn request_refresh(home: &std::path::Path, now: i64) -> Result<Value> {
    refresh_with(home, now, jobs(), &line_jobs())
}

/// `request_refresh` with these jobs, waiting as the status line of the jobs `line` names does.
fn refresh_with(
    home: &std::path::Path,
    now: i64,
    jobs: Vec<Box<dyn Job>>,
    line: &[&str],
) -> Result<Value> {
    let state = status(home, line, now)?;
    if matches!(
        state["state"].as_str(),
        Some("off" | "summaries_off" | "waiting")
    ) {
        return Ok(state);
    }
    let cfg = crate::config::load(home)?;
    let rules = crate::capture::Settings::load(home)?.rules;
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok(json!({"state": "nothing_due"}));
    };
    let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live)?;
    let held: BTreeSet<(String, String)> = match crate::providers_db::read_only(home)? {
        Some(db) => (crate::providers_db::my_work_held(&db, now)?.into_iter())
            .map(|(job, about, ..)| (job, about))
            .collect(),
        None => BTreeSet::new(),
    };
    let stores = Stores {
        raw: &raw,
        k: &k,
        rules: &rules,
        summary: &cfg.summary,
        reading: &reading,
        held: &held,
    };
    let mut due = 0;
    for job in &jobs {
        due += job
            .due(&stores, now)?
            .iter()
            .filter(|d| d.since <= now)
            .count();
    }
    if due == 0 {
        return Ok(json!({"state": "nothing_due"}));
    }
    let path = refresh_path(home);
    std::fs::create_dir_all(path.parent().unwrap_or(home))?;
    // One write at a time, so a request's time never goes back under a later one's (cubic on
    // #827). The request is the file's time, set on a handle that writes (Windows refuses a
    // read-only one).
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::File::create(&path)?.set_modified(std::time::SystemTime::now())?;
    Ok(json!({"state": "started", "due": due}))
}

/// A job every entry failed `ATTEMPTS` times is kept as a skip of what it read, one a subject, and
/// not asked again until what it would read changes (A8).
fn given_up(
    raw: &mut Raw,
    db: &Connection,
    (job, span): (&str, &str),
    skips: &[(OpKind, Value)],
    now: i64,
) -> Result<Phase> {
    raw.append_ops(skips)?;
    crate::providers_db::clear_my_work_pending(db, span)?;
    // #733: a job with another subject still held goes on saying it waits, on the earliest.
    let left = (crate::providers_db::my_work_held(db, now)?.into_iter())
        .filter(|(j, ..)| j == job)
        .min_by_key(|(.., until)| *until);
    match left {
        Some((_, _, hold, until)) => crate::providers_db::set_my_work_round(db, job, &hold, until)?,
        None => crate::providers_db::clear_my_work_round(db, job)?,
    }
    Ok(Phase::Covered)
}

#[cfg(test)]
mod tests;
