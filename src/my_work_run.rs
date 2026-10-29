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
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The chain as the role `my_work` (A1): a job's span, prompt, schema, check and gate, to an
/// answer.
pub type Caller<'a> =
    dyn FnMut(&str, &str, &Value, &AnswerCheck, &Gate) -> Result<ChainResult> + 'a;

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
        // #730: the subjects a failed batch holds, each by its own wait.
        let holds = crate::providers_db::my_work_held(db, now)?;
        let held_by: BTreeSet<(String, String)> = (holds.iter())
            .map(|(job, about, ..)| (job.clone(), about.clone()))
            .collect();
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
            f64::from(input) * calibration * p.input_weight(),
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
    let usd = match (&e.usd, &e.priced) {
        (Some((typical, most)), Some((name, model))) => format!(
            ", about USD {typical:.3} a day, at most USD {most:.2}, at {name}'s prices ({model})"
        ),
        _ => ", no USD: no paid entry is in the chain".to_owned(),
    };
    format!(
        "my work: {state} ([my_work] curate); calls a day with it on: about {}, at most {}{usd}",
        e.calls.0, e.calls.1
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
            entry("paid", paid),
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
        crate::providers_db::set_my_work_held(&db, ("links", "r9"), "resting", now + MINUTE)
            .unwrap();
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
            crate::providers_db::set_my_work_round(&db, "group", "budget", now + 60 * MINUTE)
                .unwrap();
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
}
