//! Milestone 4 Task 5 (D8): the embedding phase. The worker runs it before curation in each round
//! of `serve`. It reads the next batch of documents that have no vector (claims, then imported
//! documents, then raw records, the newest first), gates each text, and sends the batch on a
//! thread of its own, so a slow or failing embedder delays only the vectors (rows 55-1, 55-7).
//! Vectors are kept by the hash of the stored text, so a rebuild maps them back with no call
//! (spec 1.7), and a changed egress rule re-embeds nothing (A94).

use crate::curate::{Phase as Step, Reading, Reads};
use crate::embed::{Embedder, Failure};
use crate::raw::Raw;
use crate::resident::{Busy, Resident};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `provider_calls` name and role of a batch.
const ROLE: &str = "embed";
/// Documents read per poll: each poll sends at most one batch, its texts the shortest of these.
const PAGE: usize = 2 * crate::embed::BATCH;
/// `provider_state` name of the local model (Task 10): its rest after a load or run that failed,
/// kept in providers.db so a worker that exits idle does not load it again at once.
pub(crate) const LOCAL: &str = "local-embed";
/// D14: the worker loads the local model once this many documents wait for a vector, or one has
/// waited this long (ms by its time); a smaller, newer backlog is no work that keeps it up.
const LOAD_AT: usize = 20;
const LOAD_AFTER_MS: i64 = 10 * 60 * 1000;

/// One document the phase reads: its kind (`c` claim, `k` imported knowledge, `p` imported prompt,
/// `r` raw record), its key, and the metadata the index filters by.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub kind: &'static str,
    pub key: String,
    /// SHA-256 of the stored text, before the gate.
    pub sha: String,
    pub repo: String,
    pub ts: i64,
    pub session: String,
}

/// A batch to send: its documents, their texts as sent (gated, then cut), and the exclusion list
/// they were read under, with the rules and tombstones the sender holds the call to (D13).
pub struct Batch {
    pub embedder: String,
    pub docs: Vec<Doc>,
    pub texts: Vec<String>,
    pub reading: Reading,
    pub ruleset: String,
    pub tombstones: i64,
}

/// What became of a batch.
pub enum Sent {
    Vectors(Vec<Vec<f32>>),
    Failed(Failure),
    /// Nothing left the machine: the exclusions, rules or tombstones changed since the batch was
    /// read, or a local error such as raw.db not opening.
    Unsent(anyhow::Error),
}

#[cfg(test)]
thread_local! {
    static AFTER_SEND_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    /// What happens to the home while `still` waits for raw.db and the dispatch guard.
    static WHILE_STILL_WAITS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Rules loaded, raw.db opened, exclusions and tombstones checked against the batch's, raw.db
/// closed: what `send` and `send_local` do before the model runs. A call that leaves the machine
/// holds the dispatch guard, which a forget waits for.
fn still(home: &Path, batch: &Batch, leaves: bool) -> Result<Option<crate::dispatch::Guard>> {
    let raw = crate::raw::open(home)?;
    let dispatch = leaves.then(|| raw.dispatch()).transpose()?;
    #[cfg(test)]
    if let Some(meanwhile) = WHILE_STILL_WAITS.take() {
        meanwhile();
    }
    // After the waits: rules turned on while one held are the batch's no longer (owner decision
    // 43; Codex's security review of the switch).
    let rules = crate::redact::Rules::load(home)?;
    anyhow::ensure!(
        rules.version() == batch.ruleset,
        "the redaction rules changed since the batch was composed"
    );
    batch.reading.still(&raw)?;
    anyhow::ensure!(
        raw.tombstones()? == batch.tombstones,
        "the tombstones changed since the batch was composed"
    );
    Ok(dispatch)
}

/// The thread's body: `still`, then the call. What it took, in ms, beside what became of it.
pub fn send(home: &Path, batch: &Batch, embedder: &Embedder, timeout: Duration) -> (Sent, i64) {
    let started = Instant::now();
    let dispatch = match still(home, batch, true) {
        Ok(dispatch) => dispatch.expect("asked for"),
        Err(e) => return (Sent::Unsent(e), 0),
    };
    #[cfg(test)]
    AFTER_SEND_CHECK.with(|at| {
        if let Some(at) = at.borrow_mut().take() {
            at();
        }
    });
    let texts: Vec<&str> = batch.texts.iter().map(String::as_str).collect();
    let sent = match embedder.run_admitted(&texts, timeout, Some(dispatch)) {
        Ok(v) => Sent::Vectors(v),
        Err(f) => Sent::Failed(f),
    };
    (sent, started.elapsed().as_millis() as i64)
}

/// `send` on the local model (Task 10): nothing leaves the machine, so no dispatch guard holds a
/// forget back while it runs, which can take minutes; `write` keeps only the vectors of texts
/// still stored. Waits for the model's load and every text.
pub fn send_local(home: &Path, batch: &Batch, model: &Resident) -> (Sent, i64) {
    let started = Instant::now();
    if let Err(e) = still(home, batch, false) {
        return (Sent::Unsent(e), 0);
    }
    let sent = match model.embed(&batch.texts, None) {
        Ok(v) => Sent::Vectors(v),
        Err(busy) => Sent::Failed(Failure {
            status: None,
            retry_after_s: None,
            sent: false,
            message: match busy {
                Busy::Failed(why) => format!("local model: {why}"),
                other => format!("local model: {other:?}"),
            },
        }),
    };
    (sent, started.elapsed().as_millis() as i64)
}

/// A call on its thread.
struct InFlight {
    batch: Arc<Batch>,
    /// Its `provider_calls` row, counted since before it was sent; none on the local model.
    call: Option<i64>,
    thread: std::thread::JoinHandle<(Sent, i64)>,
    /// When the call's own timeout has passed (unix ms).
    until: i64,
}

/// A query vector another phase asked for (milestone 4 D9), on a thread of its own beside the
/// batch's.
struct Asked {
    key: String,
    /// The text as the asker built it, before the cut: the answer goes with it.
    text: String,
    embedder: String,
    /// Its `provider_calls` row, counted since before it was sent; none on the local model.
    call: Option<i64>,
    thread: std::thread::JoinHandle<(Sent, i64)>,
    until: i64,
}

/// An asked query vector that came back: under `key`, for `text`, from `embedder`.
pub struct Answer {
    pub key: String,
    pub text: String,
    pub embedder: String,
    pub vector: Vec<f32>,
}

/// Where a batch's vectors come from: Workers AI's call, or the local model this process holds.
enum Runner {
    WorkersAi(Embedder),
    Local(Arc<Resident>),
}

impl Runner {
    /// The batch on a thread of its own (`send` or `send_local`).
    fn spawn(
        self,
        home: &Path,
        batch: Arc<Batch>,
        timeout: Duration,
    ) -> std::thread::JoinHandle<(Sent, i64)> {
        let home = home.to_owned();
        std::thread::spawn(move || match self {
            Runner::WorkersAi(embedder) => send(&home, &batch, &embedder, timeout),
            Runner::Local(model) => send_local(&home, &batch, &model),
        })
    }
}

/// The requests of `daily_requests` kept for query vectors: batches stop this short (Global
/// Constraints).
pub(crate) const KEPT_FOR_QUERIES: u32 = 40;
/// Workers AI's free neurons a UTC day, bge-m3's neurons per million input tokens, and the price
/// past the allowance in USD per 1,000 neurons (Step 7).
const FREE_NEURONS: f64 = 10_000.0;
const NEURONS_PER_M: f64 = 1_075.0;
/// Tokens Cloudflare counts per token `budget::estimate` gives (Step 13, docs/milestone-4.md: 1.61
/// on b-import's observations; about 1.0 on short Japanese queries, which this over-counts).
const COUNTED_PER_ESTIMATED: f64 = 1.61;
const USD_PER_K_NEURONS: f64 = 0.011;

/// The phase over one home: at most one call in flight.
pub struct Phase {
    home: PathBuf,
    flight: Option<InFlight>,
    asked: Option<Asked>,
    /// The worker was asked to step aside: what is out is settled, nothing new is sent.
    held: bool,
    answered: Option<Answer>,
    unasked: Option<String>,
    db: Option<Connection>,
    /// A call's own timeout: `embed::BATCH_TIMEOUT`, shorter in tests.
    timeout: Duration,
    split: Option<Split>,
    /// The local model while documents wait for it (D14); dropped when none do.
    local: Option<Arc<Resident>>,
    /// The redaction rules whose `masked` marks were last judged (`unmask`): none before this
    /// worker's first poll.
    masked_under: Option<String>,
    #[cfg(test)]
    polls: usize,
}

/// A batch the model would not take (400, 413, 422), being split until each text it will not take
/// is alone (Step 6). A text alone that fails before anything is answered is held: a `held` mark,
/// so neither this worker nor the next sends it again or waits on it, until another document's
/// answer sends it once more, alone, in an answered split (`unhold`). Its mark gone, it is queued
/// as well: if that send fails for another reason, it goes again later as any document does.
#[derive(Default)]
struct Split {
    /// Halves still to send, before anything else.
    halves: Vec<Batch>,
    /// Whether one of its requests was answered, or an answer made it to send the held texts
    /// again: only then is a text it would not take alone that text's fault, not the embedder's.
    answered: bool,
    /// The texts it would not take alone after an answer, marked `refused` once it is over.
    lone: Vec<Doc>,
    /// Its requests that failed.
    fails: u32,
}

/// The failed requests of one split before one is answered: a batch of 100 reaches a text alone
/// within 8, so past this the embedder is failing, not a text.
const SPLIT_FAILS: u32 = 10;

impl Phase {
    pub fn new(home: &Path) -> Phase {
        Phase {
            home: home.to_owned(),
            flight: None,
            asked: None,
            held: false,
            answered: None,
            unasked: None,
            db: None,
            timeout: crate::embed::BATCH_TIMEOUT,
            split: None,
            local: None,
            masked_under: None,
            #[cfg(test)]
            polls: 0,
        }
    }

    /// Whether a call's thread has finished, a batch's or an asked query's, so a wait can end
    /// early.
    pub fn done(&self) -> bool {
        self.flight.as_ref().is_some_and(|f| f.thread.is_finished())
            || self.asked.as_ref().is_some_and(|a| a.thread.is_finished())
    }

    /// Whether a call is out or its answer not yet settled: the worker neither steps aside nor
    /// calls itself waiting until then (docs/resident.md R10, R12).
    pub fn busy(&self) -> bool {
        self.flight.is_some() || self.asked.is_some()
    }

    /// While `on`, a call that comes back is settled and no batch or query is sent: a worker
    /// asked to step aside must not start what the asking command would wait for (R12).
    pub fn hold(&mut self, on: bool) {
        self.held = on;
    }

    /// One step: a finished call's vectors written, or the next batch sent, or what it waits on.
    /// An asked query vector that came back is settled first and kept for `answer`; while one is
    /// out, the phase is not idle, so the worker stays up for it.
    pub fn poll(&mut self, raw: &Raw, k: &Connection) -> Result<Step> {
        #[cfg(test)]
        {
            self.polls += 1;
        }
        if self.asked.as_ref().is_some_and(|a| a.thread.is_finished()) {
            let a = self.asked.take().expect("checked above");
            self.answered = self.settled(a);
        }
        let step = self.batch(raw, k)?;
        let Some(a) = &self.asked else {
            return Ok(step);
        };
        // Asked again each second past its timeout, as a batch's call is.
        let until = a.until.max(crate::db::now_ms() + 1_000);
        Ok(match step {
            Step::Covered => Step::Covered,
            Step::Waiting { until: u, up: true } => Step::Waiting {
                until: u.min(until),
                up: true,
            },
            _ => Step::Waiting { until, up: true },
        })
    }

    /// Asks for `text`'s vector under `key` and returns at once (milestone 4 D9). The text, gated
    /// by the asker, is cut as a prompt is, counted in providers.db (role `query`) from the day's
    /// whole allowance as a search's query is, and sent on a thread of its own once its exclusions,
    /// rules and tombstones are found to be current still (`send`). False when nothing is asked: one is out,
    /// embedding is off or its settings do not load, the active vectors are another embedder's,
    /// the embedder rests, or a cap is spent.
    pub fn ask(
        &mut self,
        k: &Connection,
        key: &str,
        text: &str,
        reading: &Reading,
        rules: &crate::redact::Rules,
        tombstones: i64,
    ) -> Result<bool> {
        use crate::providers_db as pdb;
        if self.asked.is_some() || self.held {
            return Ok(false);
        }
        let Ok(config) = crate::config::load(&self.home) else {
            return Ok(false);
        };
        let runner = match config.embedding.provider.as_str() {
            // The local model answers only while the worker holds it for documents (D14): a query
            // alone loads nothing, and costs nothing to count.
            "local" => match &self.local {
                Some(model) if model.ready() => Runner::Local(Arc::clone(model)),
                _ => return Ok(false),
            },
            _ => match Embedder::from_config(&config.embedding) {
                Ok(Some(embedder)) => Runner::WorkersAi(embedder),
                _ => return Ok(false),
            },
        };
        let id = crate::embed::EMBEDDER;
        let active: Option<String> = k
            .query_row(
                "SELECT embedder FROM vec_generation WHERE state = 'active'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if active.as_deref() != Some(id) {
            return Ok(false);
        }
        let sent: String = text.chars().take(crate::embed::PROMPT_CHARS).collect();
        let call = match &runner {
            Runner::Local(_) => None,
            Runner::WorkersAi(_) => {
                let reserved = self.providers().and_then(|db| {
                    if pdb::state(db, crate::embed::CALLS)?.down_until > crate::db::now_ms() {
                        return Ok(None);
                    }
                    let cfg = &config.embedding;
                    Ok(reserve(
                        db,
                        "query",
                        "1 query",
                        &sent,
                        cfg.daily_requests,
                        cfg.monthly_usd,
                    )?
                    .ok())
                });
                match reserved {
                    Ok(Some(call)) => Some(call),
                    Ok(None) => return Ok(false),
                    Err(e) => {
                        eprintln!("oboete: no query embedding for now: {e:#}");
                        return Ok(false);
                    }
                }
            }
        };
        let batch = Arc::new(Batch {
            embedder: id.to_owned(),
            docs: Vec::new(),
            texts: vec![sent],
            reading: reading.clone(),
            ruleset: rules.version().to_owned(),
            tombstones,
        });
        self.asked = Some(Asked {
            key: key.to_owned(),
            text: text.to_owned(),
            embedder: id.to_owned(),
            call,
            thread: runner.spawn(&self.home, batch, self.timeout),
            until: crate::db::now_ms() + self.timeout.as_millis() as i64,
        });
        Ok(true)
    }

    /// The asked query vector that came back since the last call, if any.
    pub fn answer(&mut self) -> Option<Answer> {
        self.answered.take()
    }

    /// A key whose query never left: the asker can drop its cooldown and compose it again.
    pub fn unasked(&mut self) -> Option<String> {
        self.unasked.take()
    }

    /// A finished ask settled as `search::b::embedded` settles a query: counted, and never a
    /// rest; one not sent is no longer counted.
    fn settled(&mut self, a: Asked) -> Option<Answer> {
        use crate::providers_db as pdb;
        let (sent, ms) = a.thread.join().unwrap_or_else(|_| {
            (
                Sent::Unsent(anyhow::anyhow!("the call's thread panicked")),
                0,
            )
        });
        let (outcome, detail, billed) = match &sent {
            Sent::Unsent(e) => {
                eprintln!("oboete: query embedding not sent: {e:#}");
                let unreserved = (a.call).map_or(Ok(()), |call| {
                    self.providers().and_then(|db| pdb::unreserve(db, call))
                });
                if let Err(e) = unreserved {
                    eprintln!("oboete: a query embedding not sent stays counted: {e:#}");
                }
                self.unasked = Some(a.key);
                return None;
            }
            Sent::Vectors(_) => ("ok", "1 query".to_owned(), true),
            Sent::Failed(f) => ("error", f.message.clone(), f.billed()),
        };
        // The local model's queries are not counted: they cost nothing and leave nothing.
        if let Some(call) = a.call {
            let settled = self
                .providers()
                .and_then(|db| pdb::settle(db, call, outcome, ms, &detail, billed));
            if let Err(e) = settled {
                eprintln!("oboete: a query embedding is not settled: {e:#}");
            }
        } else if let Sent::Failed(f) = &sent {
            eprintln!("oboete: query embedding failed: {}", f.message);
        }
        let Sent::Vectors(mut vectors) = sent else {
            return None;
        };
        let vector = vectors.pop().filter(|v| v.len() == crate::embed::DIM)?;
        Some(Answer {
            key: a.key,
            text: a.text,
            embedder: a.embedder,
            vector,
        })
    }

    /// `poll`'s batches.
    fn batch(&mut self, raw: &Raw, k: &Connection) -> Result<Step> {
        // First: a knowledge.db the worker has just started takes its vectors before anything is
        // written to it, a call's answer included.
        carry_set_aside(&self.home, k, raw)?;
        if let Some(f) = &self.flight {
            if !f.thread.is_finished() {
                // A thread past its call's timeout is asked again each second, never at once.
                return Ok(Step::Waiting {
                    until: f.until.max(crate::db::now_ms() + 1_000),
                    up: true,
                });
            }
            let f = self.flight.take().expect("a call in flight");
            let (sent, ms) = f.thread.join().unwrap_or_else(|_| {
                (
                    Sent::Unsent(anyhow::anyhow!("the call's thread panicked")),
                    0,
                )
            });
            self.finish(raw, k, &f.batch, f.call, sent, ms)?;
            return Ok(Step::Covered);
        }
        if self.held {
            return Ok(Step::Idle);
        }
        // Read again each poll: a worker that stays up follows the owner's edits.
        let loaded = crate::config::load(&self.home)
            .and_then(|c| Ok((Embedder::from_config(&c.embedding)?, c.embedding)));
        let (embedder, cfg) = match loaded {
            Ok((embedder, cfg)) if embedder.is_some() || cfg.provider == "local" => (embedder, cfg),
            Ok(_) => {
                self.local = None;
                return Ok(Step::Idle);
            }
            Err(e) => {
                eprintln!("oboete: no embedding for now: {e:#}");
                return Ok(Step::Idle);
            }
        };
        let rules = match crate::redact::Rules::load(&self.home) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("oboete: no embedding for now: {e:#}");
                return Ok(Step::Idle);
            }
        };
        let reading = Reading::now(raw, Reads::Live)?;
        // Milestone 5 D1 rule 12: no text is sent while knowledge.db lags a forget.
        if crate::curate::lagging(raw, k)? {
            return Ok(Step::Idle);
        }
        cleared(k, crate::embed::EMBEDDER, &reading, &rules)?;
        if self.masked_under.as_deref() != Some(rules.version()) {
            unmask(k, crate::embed::EMBEDDER, rules.version())?;
            self.masked_under = Some(rules.version().to_owned());
        }
        let Some(embedder) = embedder else {
            return self.local_batch(raw, k, &reading, &rules);
        };
        self.local = None;
        let wait = match self.held_back(&cfg) {
            Ok(wait) => wait,
            // A providers.db that will not open or read holds back the vectors only.
            Err(e) => {
                eprintln!("oboete: no embedding for now: {e:#}");
                return Ok(Step::Idle);
            }
        };
        let half = match wait {
            None => self.next_half(raw, k, &embedder.id, &reading, &rules)?,
            Some(_) => None,
        };
        let batch = match half {
            Some(b) => b,
            // Cached vectors are mapped and documents passed over are marked whatever the rest
            // or the cap, each kind up to its first page with a text to send; only the call waits.
            None => {
                let Some(b) = pending(raw, k, &embedder.id, &reading, &rules, wait.is_some())?
                else {
                    return Ok(Step::Idle);
                };
                if let Some(w) = wait {
                    return Ok(w);
                }
                b
            }
        };
        // Counted before it is sent (Step 7); one that cannot be counted is not sent.
        let cap = cfg.daily_requests.saturating_sub(KEPT_FOR_QUERIES);
        let (span, texts) = (
            format!("{} documents", batch.docs.len()),
            batch.texts.join("\n"),
        );
        let reserved = self
            .providers()
            .and_then(|db| reserve(db, ROLE, &span, &texts, cap, cfg.monthly_usd));
        let call = match reserved {
            Ok(Ok(call)) => call,
            Ok(Err(until)) => {
                let up = until.saturating_sub(crate::db::now_ms()) <= crate::curate::STAY_UP_MS;
                return Ok(Step::Waiting { until, up });
            }
            Err(e) => {
                eprintln!("oboete: no embedding for now: {e:#}");
                return Ok(Step::Idle);
            }
        };
        Ok(self.fly(Runner::WorkersAi(embedder), batch, Some(call)))
    }

    /// The batch sent on its thread: the phase waits for it, and the worker stays up.
    fn fly(&mut self, runner: Runner, batch: Batch, call: Option<i64>) -> Step {
        let batch = Arc::new(batch);
        let thread = runner.spawn(&self.home, Arc::clone(&batch), self.timeout);
        let until = crate::db::now_ms() + self.timeout.as_millis() as i64;
        self.flight = Some(InFlight {
            batch,
            call,
            thread,
            until,
        });
        Step::Waiting { until, up: true }
    }

    /// `batch` on the local model (Task 10, D14): no cap, no count and no split. It loads once
    /// `LOAD_AT` documents wait or one has waited `LOAD_AFTER_MS`, and goes when none wait; a
    /// load or run that fails rests it in providers.db as a failed call rests Workers AI.
    fn local_batch(
        &mut self,
        raw: &Raw,
        k: &Connection,
        reading: &Reading,
        rules: &crate::redact::Rules,
    ) -> Result<Step> {
        use crate::providers_db as pdb;
        let now = crate::db::now_ms();
        let rest = match self.providers().and_then(|db| pdb::state(db, LOCAL)) {
            Ok(state) => state.down_until,
            Err(e) => {
                eprintln!("oboete: no local embedding for now: {e:#}");
                return Ok(Step::Idle);
            }
        };
        let embedder = crate::embed::EMBEDDER;
        if rest > now {
            self.local = None;
            // As while a Workers AI call waits, cached vectors are mapped and documents passed
            // over are marked; only the model waits (Codex on #426).
            if pending(raw, k, embedder, reading, rules, true)?.is_none() {
                return Ok(Step::Idle);
            }
            let up = rest - now <= crate::curate::STAY_UP_MS;
            return Ok(Step::Waiting { until: rest, up });
        }
        let loaded = self.local.as_ref().filter(|model| !model.gone()).cloned();
        // With no model yet, every kind is read, so its threshold counts the whole backlog.
        let mut seen = Seen::default();
        let counting = loaded.is_none().then_some(&mut seen);
        let Some(batch) = pending_seen(raw, k, embedder, reading, rules, false, counting)? else {
            self.local = None;
            return Ok(Step::Idle);
        };
        let model = match loaded {
            Some(model) => model,
            None => {
                let oldest = seen.oldest.unwrap_or(now);
                if seen.count < LOAD_AT && now - oldest < LOAD_AFTER_MS {
                    let until = oldest + LOAD_AFTER_MS;
                    return Ok(Step::Waiting { until, up: false });
                }
                let load = match crate::embed::local_model(&self.home) {
                    Ok(load) => load,
                    // Doctor names why (`embed::local_state`); the documents keep waiting.
                    Err(why) => {
                        eprintln!("oboete: no local embedding for now: {why}");
                        return Ok(Step::Idle);
                    }
                };
                let model = Arc::new(Resident::start(load, None));
                self.local = Some(Arc::clone(&model));
                model
            }
        };
        Ok(self.fly(Runner::Local(model), batch, None))
    }

    /// The split's next half, its documents read again (D8): one whose stored text changed or
    /// went since is left out (a fresh page brings it back), and the rest are sorted out and gated
    /// as a page's are, under the list read now, so a mask, a rule or an exclusion made while it
    /// waited holds.
    fn next_half(
        &mut self,
        raw: &Raw,
        k: &Connection,
        embedder: &str,
        reading: &Reading,
        rules: &crate::redact::Rules,
    ) -> Result<Option<Batch>> {
        let Some(split) = &mut self.split else {
            return Ok(None);
        };
        let tombstones = raw.tombstones()?;
        while let Some(half) = split.halves.pop() {
            let (mut docs, mut texts) = (Vec::new(), Vec::new());
            let tx = k.unchecked_transaction()?;
            let held = crate::claims::Pending::read(raw, &tx)?;
            for doc in half.docs {
                let Some(mut r) = current(Some(raw), &tx, doc.kind, &doc.key)? else {
                    continue;
                };
                if r.doc.sha != doc.sha {
                    continue;
                }
                if claim_pending(raw, &held, &tx, &r.doc)? {
                    continue;
                }
                if let Some((device, seq)) = doc.key.rsplit_once(':')
                    && matches!(doc.kind, "r" | "rp")
                {
                    r.labels = raw.event_labels(device, seq.parse()?)?;
                }
                if let Some((doc, text)) = sort_out(&tx, raw, reading, embedder, r, rules)? {
                    docs.push(doc);
                    texts.push(text);
                }
            }
            tx.commit()?;
            if !docs.is_empty() {
                return Ok(Some(Batch {
                    embedder: embedder.to_owned(),
                    docs,
                    texts,
                    reading: reading.clone(),
                    ruleset: rules.version().to_owned(),
                    tombstones,
                }));
            }
        }
        Ok(None)
    }

    fn providers(&mut self) -> Result<&Connection> {
        if self.db.is_none() {
            self.db = Some(crate::providers_db::open(&self.home)?);
        }
        Ok(self.db.as_ref().expect("opened above"))
    }

    /// Why no batch goes now, and until when: the embedder rests (a failure's cooldown, or the
    /// owner's hold), the day's requests are spent but those kept for queries, or the month's USD
    /// is (Step 7).
    fn held_back(&mut self, cfg: &crate::config::Embedding) -> Result<Option<Step>> {
        use crate::providers_db as pdb;
        let db = self.providers()?;
        let now = crate::db::now_ms();
        let rest = pdb::state(db, crate::embed::CALLS)?.down_until;
        let (calls, oldest) = pdb::calls_in_a_day(db, crate::embed::CALLS)?;
        let until = if rest > now {
            rest
        } else if calls >= cfg.daily_requests.saturating_sub(KEPT_FOR_QUERIES) {
            oldest.map_or(now + pdb::DAY_MS, pdb::out_of_the_day)
        } else if pdb::embed_usd_this_month(db)? >= cfg.monthly_usd {
            pdb::next_month()
        } else {
            return Ok(None);
        };
        Ok(Some(Step::Waiting {
            until,
            up: until.saturating_sub(now) <= crate::curate::STAY_UP_MS,
        }))
    }

    /// A finished call. providers.db first: the call settled (its outcome, and its estimated cost
    /// unless the embedder answered with an error status) and the rest it sets, only logged when
    /// that fails, as the call was counted before it was sent. Then knowledge.db: its vectors in
    /// one transaction, each document's only while its stored text is still the one sent (row
    /// 55-6), or the batch split and its lone texts refused.
    fn finish(
        &mut self,
        raw: &Raw,
        k: &Connection,
        batch: &Batch,
        call: Option<i64>,
        sent: Sent,
        ms: i64,
    ) -> Result<()> {
        use crate::providers_db as pdb;
        // The local model's batch is not counted, and its failures rest it, not Workers AI.
        let name = match call {
            Some(_) => crate::embed::CALLS,
            None => LOCAL,
        };
        let failure = match &sent {
            // Nothing left: the call is no longer counted, and no rest is set. A split's other
            // halves were read under the same list: the next poll reads them again.
            Sent::Unsent(e) => {
                eprintln!("oboete: embedding not sent: {e:#}");
                self.split = None;
                let unreserved = call.map_or(Ok(()), |call| {
                    self.providers().and_then(|db| pdb::unreserve(db, call))
                });
                if let Err(e) = unreserved {
                    eprintln!("oboete: an embedding call not sent stays counted: {e:#}");
                }
                return Ok(());
            }
            Sent::Vectors(_) => None,
            Sent::Failed(f) => Some(f),
        };
        // The embedder's state after it: unchanged (None), cleared (Some(None)) or rested.
        let mut rest = None;
        // A text that failed alone before anything was answered, to be marked `held`.
        let mut hold = Vec::new();
        match failure {
            None => {
                if let Some(split) = &mut self.split {
                    split.answered = true;
                }
                rest = Some(None);
            }
            Some(f) if call.is_some() && matches!(f.status, Some(400 | 413 | 422)) => {
                let split = self.split.get_or_insert_with(Split::default);
                split.fails += 1;
                if let [doc] = &batch.docs[..] {
                    if split.answered {
                        split.lone.push(doc.clone());
                    } else {
                        hold.push(doc.clone());
                    }
                } else {
                    let mid = batch.docs.len() / 2;
                    for (docs, texts) in [
                        (&batch.docs[..mid], &batch.texts[..mid]),
                        (&batch.docs[mid..], &batch.texts[mid..]),
                    ] {
                        split.halves.push(Batch {
                            embedder: batch.embedder.clone(),
                            docs: docs.to_vec(),
                            texts: texts.to_vec(),
                            reading: batch.reading.clone(),
                            ruleset: batch.ruleset.clone(),
                            tombstones: batch.tombstones,
                        });
                    }
                }
            }
            Some(f) => {
                if call.is_none() {
                    eprintln!("oboete: {}", f.message);
                    self.local = None;
                }
                rest = Some(Some(f));
            }
        }
        // A split that is over, or that failed past `SPLIT_FAILS` unanswered: answered, its lone
        // texts are refused; unanswered, the embedder's state takes the failure as any other's (a
        // 400 rests it only as the breaker's third in a row). What it held stays held.
        let mut refused = Vec::new();
        if let Some(split) = &self.split
            && (split.halves.is_empty() || (!split.answered && split.fails >= SPLIT_FAILS))
        {
            let split = self.split.take().expect("checked above");
            if split.answered {
                refused = split.lone;
            } else if failure.is_some() {
                rest = Some(failure);
            }
        }
        let (outcome, detail, billed) = match failure {
            None => ("ok", batch.docs.len().to_string(), true),
            Some(f) => ("error", f.message.clone(), f.billed()),
        };
        let settled = self.providers().and_then(|db| {
            if let Some(call) = call {
                pdb::settle(db, call, outcome, ms, &detail, billed)?;
            }
            if let Some(rest) = rest {
                let was = pdb::state(db, name)?;
                let next = rest.map_or_else(pdb::State::default, |f| {
                    crate::provider::next_state(was, &f.into())
                });
                if next != was {
                    pdb::set_state(db, name, next)?;
                }
            }
            Ok(())
        });
        if let Err(e) = settled {
            eprintln!("oboete: an embedding call is not settled: {e:#}");
        }
        if let Sent::Vectors(vecs) = &sent {
            write(raw, k, batch, vecs)?;
            // The embedder answers: each text held for that goes once more, alone, in an answered
            // split, so only a failure now is that text's fault.
            let held = unhold(k, &batch.embedder)?;
            if !held.is_empty() {
                let split = self.split.get_or_insert_with(Split::default);
                split.answered = true;
                split.halves.extend(held.into_iter().map(|doc| Batch {
                    embedder: batch.embedder.clone(),
                    docs: vec![doc],
                    texts: Vec::new(),
                    reading: batch.reading.clone(),
                    ruleset: batch.ruleset.clone(),
                    tombstones: batch.tombstones,
                }));
            }
        }
        for doc in &refused {
            if stored(raw, k, doc)? {
                mark(k, &batch.embedder, doc, "refused")?;
            }
        }
        for doc in &hold {
            if stored(raw, k, doc)? {
                mark(k, &batch.embedder, doc, "held")?;
            }
        }
        Ok(())
    }
}

pub(crate) struct DoctorFacts {
    pub(crate) generation: Result<Option<String>>,
    pub(crate) claims: Result<i64>,
    pub(crate) cards: Result<i64>,
    pub(crate) summaries: Result<i64>,
    pub(crate) imports: Result<i64>,
    pub(crate) records: Result<i64>,
    pub(crate) skipped: Result<Vec<(String, i64)>>,
}

/// Native SELECTs only: no schema, key read, reservation or provider call.
pub(crate) fn doctor_facts_in(k: &Connection, embedder: &str) -> DoctorFacts {
    let generation = k
        .query_row(
            "SELECT state FROM vec_generation WHERE embedder = ?1",
            [embedder],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into);
    let count = |sql: &str| -> Result<i64> { Ok(k.query_row(sql, [embedder], |r| r.get(0))?) };
    let claims = count(
        "SELECT count(*) FROM active a WHERE NOT EXISTS (SELECT 1 FROM vector_keys v
           WHERE v.embedder = ?1 AND v.kind = 'c' AND v.key = a.uid)",
    );
    // Parity row 3: a removed card or summary waits for no vector. A doctor can read a store the
    // worker has not given the column yet.
    let shown = |table: &str| crate::hide::shown(k, table, "x.").unwrap_or_default();
    let cards = count(&format!(
        "SELECT count(*) FROM cards x WHERE x.replaced_by IS NULL {} AND NOT EXISTS (
           SELECT 1 FROM vector_keys v WHERE v.embedder = ?1 AND v.kind = 'o'
             AND v.key = x.device || '.' || x.op_seq || '.' || x.n)",
        shown("cards")
    ));
    let summaries = count(&format!(
        "SELECT count(*) FROM turns x WHERE x.skipped = 0 {} AND NOT EXISTS (
           SELECT 1 FROM vector_keys v WHERE v.embedder = ?1 AND v.kind = 's'
             AND v.key = 'S' || x.device || '.' || x.op_seq)",
        shown("turns")
    ));
    let imports = count(
        "SELECT count(*) FROM imported i
         WHERE i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = i.uid)
           AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
             AND v.kind IN ('k', 'p') AND v.key = i.uid)",
    );
    let records = count(
        "SELECT count(*) FROM raw_docs d WHERE d.kind <> 'tool' AND NOT EXISTS (
           SELECT 1 FROM vector_keys v
           WHERE v.embedder = ?1 AND v.kind = 'r' AND v.key = d.device || ':' || d.seq)",
    );
    let skipped = (|| -> Result<Vec<(String, i64)>> {
        Ok(k.prepare(
            "SELECT CASE WHEN skipped LIKE 'masked:%' THEN 'empty' ELSE skipped END AS why,
                    count(*) FROM vector_keys WHERE embedder = ?1 AND skipped IS NOT NULL
             GROUP BY why ORDER BY why",
        )?
        .query_map([embedder], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
    })();
    DoctorFacts {
        generation,
        claims,
        cards,
        summaries,
        imports,
        records,
        skipped,
    }
}

/// Latest error in the native embedding accounting-name pool, without opening a store.
/// The CLI alone formats detail; public projections use only ts and a fixed role code.
pub(crate) struct LastEmbeddingError {
    pub(crate) ts: i64,
    pub(crate) role: String,
    detail: Option<String>,
}

pub(crate) fn last_embedding_error_in(db: &Connection) -> Result<Option<LastEmbeddingError>> {
    Ok(db
        .query_row(
            "SELECT ts, role, detail FROM provider_calls WHERE provider = ?1 AND outcome = 'error'
             ORDER BY id DESC LIMIT 1",
            [crate::embed::CALLS],
            |r| {
                Ok(LastEmbeddingError {
                    ts: r.get(0)?,
                    role: r.get(1)?,
                    detail: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// Doctor's lines (Step 11): the embedder and its generation, the documents still waiting per kind,
/// those passed over per reason, the requests and USD against the caps, a rest, the last error.
pub fn doctor_lines(home: &Path, k: &Connection) -> Result<Vec<String>> {
    use crate::providers_db as pdb;
    let config = crate::config::load(home)?.embedding;
    if config.provider == "none" {
        return Ok(vec!["embeddings: off".into()]);
    }
    let local = config.provider == "local";
    let embedder = crate::embed::EMBEDDER;
    crate::claims::schema(k)?;
    crate::consumer::imported::schema(k)?;
    crate::consumer::fts::schema(k)?;
    crate::cards::schema(k)?;
    crate::turns::schema(k)?;
    let facts = doctor_facts_in(k, embedder);
    let state = facts.generation?;
    let claims = facts.claims?;
    let cards = facts.cards?;
    let summaries = facts.summaries?;
    let imports = facts.imports?;
    let records = facts.records?;
    let mut lines = vec![format!(
        "embeddings: {embedder}{} ({}), waiting: {claims} claims, {cards} cards, {summaries} summaries, {imports} imported, {records} records",
        if local { " on this machine" } else { "" },
        state.as_deref().unwrap_or("nothing embedded yet")
    )];
    let skipped: Vec<String> = facts
        .skipped?
        .into_iter()
        .map(|(reason, count)| format!("{count} {reason}"))
        .collect();
    if !skipped.is_empty() {
        lines.push(format!("  passed over: {}", skipped.join(", ")));
    }
    if !home.join("providers.db").exists() {
        return Ok(lines);
    }
    // Requests and USD are Workers AI's; the local model has its own rest, after a load or run
    // that failed (the worker's log says why).
    if local {
        let rest = pdb::state(&pdb::open(home)?, LOCAL)?.down_until;
        if rest > crate::db::now_ms() {
            lines.push(format!(
                "  local model resting until {} after a load or run that failed (the worker's log says why)",
                crate::db::utc(rest)
            ));
        }
        return Ok(lines);
    }
    let db = pdb::open(home)?;
    let calls = crate::embed::CALLS;
    lines.push(format!(
        "  requests in the last day: {} of {} ({KEPT_FOR_QUERIES} kept for queries); USD this month: {:.2} of {:.2}",
        pdb::calls_in_a_day(&db, calls)?.0,
        config.daily_requests,
        pdb::embed_usd_this_month(&db)?,
        config.monthly_usd
    ));
    let rest = pdb::state(&db, calls)?.down_until;
    if rest > crate::db::now_ms() {
        lines.push(format!("  resting until {}", crate::db::utc(rest)));
    }
    if let Some(LastEmbeddingError { ts, role, detail }) = last_embedding_error_in(&db)? {
        lines.push(format!(
            "  last error ({role}, {}): {}",
            crate::db::utc(ts),
            detail
                .as_deref()
                .map(pdb::visible_detail)
                .unwrap_or_default()
        ));
    }
    Ok(lines)
}

/// Spec 1.7: the vectors of `from`, a knowledge.db set aside by a rebuild or quarantined, copied
/// into `k`'s cache, so the documents they were made for are mapped again with no call. Only the
/// vectors a key of `from` maps (milestone 5 D5): none a `forgotten` claim's or document's key
/// maps, nor one no key maps any more (an earlier text's: a forgotten claim's earlier text cannot
/// be told from the rest). Only blobs of one unit vector each; a file from before the keys carries
/// nothing. One transaction: a read that fails carries nothing.
// ponytail: within one knowledge.db the cache keeps every text ever embedded, the old texts of
// changed documents too (a rewind maps them back for free), and a rebuild or a quarantine drops
// those; sweep unreferenced rows if the live cache grows past what that is worth.
pub fn carry(k: &Connection, from: &Path, forgotten: &crate::forget::Forgotten) -> Result<usize> {
    let old = Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has: bool = old.query_row(
        "SELECT COUNT(*) = 2 FROM sqlite_master
         WHERE type = 'table' AND name IN ('vectors', 'vector_keys')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        return Ok(0);
    }
    // Beside the old file's keys: a read-only connection still writes its temp tables.
    old.execute_batch(
        "CREATE TEMP TABLE forgotten(uid TEXT PRIMARY KEY);
         CREATE TEMP TABLE forgotten_records(key TEXT PRIMARY KEY);",
    )?;
    {
        let mut add = old.prepare("INSERT OR IGNORE INTO temp.forgotten(uid) VALUES (?1)")?;
        for uid in &forgotten.uids {
            add.execute([uid])?;
        }
        let mut add =
            old.prepare("INSERT OR IGNORE INTO temp.forgotten_records(key) VALUES (?1)")?;
        for key in &forgotten.records {
            add.execute([key])?;
        }
    }
    let tx = k.unchecked_transaction()?;
    let mut carried = 0;
    {
        // A vector several keys map is read for each and inserted once. A key that kept no
        // vector (`src_sha` empty or null) maps none.
        let mut read = old.prepare(
            "SELECT v.embedder, v.src_sha, v.vec FROM vector_keys k
             JOIN vectors v ON v.embedder = k.embedder AND v.src_sha = k.src_sha
             WHERE k.src_sha <> ''
               AND NOT (k.kind IN ('c', 'k', 'p') AND k.key IN (SELECT uid FROM temp.forgotten))
               AND NOT (k.kind = 'r' AND k.key IN (SELECT key FROM temp.forgotten_records))",
        )?;
        let mut rows = read.query([])?;
        let mut write = tx
            .prepare("INSERT OR IGNORE INTO vectors(embedder, src_sha, vec) VALUES (?1, ?2, ?3)")?;
        while let Some(r) = rows.next()? {
            let vec: Vec<u8> = r.get(2)?;
            let (floats, rest) = vec.as_chunks::<4>();
            let norm = floats
                .iter()
                .map(|b| f64::from(f32::from_le_bytes(*b)).powi(2))
                .sum::<f64>()
                .sqrt();
            // `embed` keeps unit vectors only; the sum is NaN when one is not finite.
            let unit = (norm - 1.0).abs() < 1e-3;
            if floats.len() != crate::embed::DIM || !rest.is_empty() || !unit {
                continue;
            }
            carried +=
                write.execute(params![r.get::<_, String>(0)?, r.get::<_, String>(1)?, vec])?;
        }
    }
    tx.commit()?;
    Ok(carried)
}

/// After a restore, a damage quarantine or a rebuild that stopped (spec 1.7): the vectors of
/// every knowledge.db set aside since the one open now last took them, carried once, whose
/// checkpoint `carried` holds the time of the newest file it took them from. Every one, not the
/// newest only: a rebuild stopped twice leaves its vectors in the older file. One whose vectors
/// cannot be read carries nothing, and says so once. No forgotten uid's vector comes back (D5).
pub(crate) fn carry_set_aside(home: &Path, k: &Connection, raw: &Raw) -> Result<()> {
    use crate::knowledge::checkpoint;
    let done = checkpoint::get(k, "carried", "")?;
    // A listing or an entry that cannot be read is an error, not a copy passed over: the purge
    // removes every copy after this (Codex on #444).
    let mut aside: Vec<(i64, String)> = Vec::new();
    for entry in std::fs::read_dir(home)? {
        let Ok(name) = entry?.file_name().into_string() else {
            continue;
        };
        let at = ["knowledge.db.quarantined-", "knowledge.db.rebuilding-"]
            .iter()
            .find_map(|p| name.strip_prefix(p))
            .and_then(|at| at.parse().ok());
        if let Some(at) = at.filter(|at| *at > done) {
            aside.push((at, name));
        }
    }
    let forgotten = if aside.is_empty() {
        crate::forget::Forgotten::default()
    } else {
        let forgotten = crate::forget::Forgotten {
            uids: raw.forgotten_set()?,
            records: raw.forgotten_record_keys()?,
        };
        crate::forget::with_logged(home, forgotten, Some(raw.home_id()))?
    };
    for (_, name) in &aside {
        if let Err(e) = carry(k, &home.join(name), &forgotten) {
            eprintln!("oboete: no vectors carried from {name}: {e:#}");
        }
    }
    match aside.iter().map(|(at, _)| *at).max() {
        Some(newest) => checkpoint::set_in(k, checkpoint::SEQS, "carried", "", newest),
        None => Ok(()),
    }
}

/// A request counted before it is sent (Step 7): in one write transaction, a row that counts as
/// sent and billed as estimated until `providers_db::settle` says what became of it, its id; a
/// process that dies first leaves it counted, and one that checks meanwhile (the CLI, MCP, the
/// worker) sees it. Or, when it may not go, until when: the day's requests reached `cap` (until
/// the oldest leaves the day), the month's USD reached `monthly_usd` (the next month), or this
/// request's cost would pass it (the next UTC day, whose free neurons may make it cost nothing).
pub(crate) fn reserve(
    db: &Connection,
    role: &str,
    span: &str,
    texts: &str,
    cap: u32,
    monthly_usd: f64,
) -> Result<std::result::Result<i64, i64>> {
    use crate::providers_db as pdb;
    let tx = rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let now = crate::db::now_ms();
    let (calls, oldest) = pdb::calls_in_a_day(&tx, crate::embed::CALLS)?;
    if calls >= cap {
        return Ok(Err(oldest.map_or(now + pdb::DAY_MS, pdb::out_of_the_day)));
    }
    let est = crate::budget::estimate(texts);
    let usd = billed_usd(&tx, est)?;
    let spent = pdb::embed_usd_this_month(&tx)?;
    if spent >= monthly_usd {
        return Ok(Err(pdb::next_month()));
    }
    if spent + usd > monthly_usd {
        return Ok(Err(now - now.rem_euclid(pdb::DAY_MS) + pdb::DAY_MS));
    }
    pdb::record(
        &tx,
        &pdb::Call {
            provider: crate::embed::CALLS,
            role,
            span,
            outcome: "sent",
            ms: 0,
            detail: None,
            bytes_out: texts.len(),
            est_tokens: Some(est),
            usage: pdb::Usage::default(),
            usd: Some(usd),
        },
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Ok(id))
}

/// What a billed call of `tokens` costs, after the UTC day's billed embedding calls so far.
pub(crate) fn billed_usd(db: &Connection, tokens: u32) -> Result<f64> {
    let now = crate::db::now_ms();
    let day = now - now.rem_euclid(crate::providers_db::DAY_MS);
    let before: i64 = db.query_row(
        "SELECT COALESCE(SUM(est_tokens), 0) FROM provider_calls
         WHERE provider = ?1 AND ts >= ?2 AND usd IS NOT NULL",
        params![crate::embed::CALLS, day],
        |r| r.get(0),
    )?;
    Ok(usd(before, i64::from(tokens)))
}

/// What `tokens` more cost past the UTC day's free neurons, the day's billed calls having used
/// `before` tokens. ponytail: the allowance is the account's, shared with v1 and any other Workers
/// AI use, and this sees only this store's calls; counting the account's needs its usage API.
fn usd(before: i64, tokens: i64) -> f64 {
    let past =
        |t: i64| (t as f64 * COUNTED_PER_ESTIMATED * NEURONS_PER_M / 1e6 - FREE_NEURONS).max(0.0);
    (past(before + tokens) - past(before)) * USD_PER_K_NEURONS / 1_000.0
}

/// The `excluded` markers hold only under the list they were made under: a new list clears them,
/// so undoing an exclusion queues its documents again (D13). And under the rules: an imported
/// document's project is read as they read it, so a rule taken away can free one (CodeRabbit on
/// #489).
fn cleared(
    k: &Connection,
    embedder: &str,
    reading: &Reading,
    rules: &crate::redact::Rules,
) -> Result<()> {
    let list = serde_json::to_string(&(&*reading.exclusions, rules.version()))?;
    let made: Option<Option<String>> = k
        .query_row(
            "SELECT exclusions FROM vec_generation WHERE embedder = ?1",
            [embedder],
            |r| r.get(0),
        )
        .optional()?;
    if made
        .as_ref()
        .is_some_and(|m| m.as_deref() == Some(list.as_str()))
    {
        return Ok(());
    }
    let tx = k.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM vector_keys WHERE embedder = ?1 AND skipped = 'excluded'",
        [embedder],
    )?;
    // The first embedder is active from its start.
    tx.execute(
        "INSERT INTO vec_generation(embedder, state, exclusions) VALUES (?1, 'active', ?2)
         ON CONFLICT(embedder) DO UPDATE SET exclusions = excluded.exclusions",
        params![embedder, list],
    )?;
    tx.commit()?;
    Ok(())
}

/// The texts the rules blanked under other rules, their marks taken off so they are read again
/// under these (Codex on #429), as an `excluded` mark is judged again when the exclusion changes.
/// Read once per worker and once per change of the rules: it reads every key row.
fn unmask(k: &Connection, embedder: &str, ruleset: &str) -> Result<()> {
    k.execute(
        "DELETE FROM vector_keys WHERE embedder = ?1 AND skipped LIKE 'masked:%'
           AND skipped <> 'masked:' || ?2",
        params![embedder, ruleset],
    )?;
    Ok(())
}

/// A document read to embed: what it is, and its stored text.
struct Read {
    doc: Doc,
    /// Its text as stored; an imported document's body.
    text: String,
    /// Imported documents only: their kind and title, composed with the body once each is gated
    /// alone (`gated`).
    title: Option<(String, String)>,
    /// Records and summaries: its session as `Raw::event_labels` spells it, and a record's source.
    labels: Option<(String, String)>,
    /// Cards and summaries: the byte range of each value in `text`, gated alone as their readers
    /// gate it (K6).
    parts: Vec<std::ops::Range<usize>>,
}

/// The next batch of documents with no vector from `embedder`: claims, then cards and session
/// summaries (docs/tools.md V1), then imported documents, then raw records, the newest first. A
/// document passed over is marked (`excluded`, `source`, `empty`), and one whose stored text is
/// already embedded is mapped to that vector, so neither comes back; the rest of a page is sent
/// as one batch of its shortest texts. While a call `waiting` goes nowhere, the later kinds are
/// mapped and marked too.
fn pending(
    raw: &Raw,
    k: &Connection,
    embedder: &str,
    reading: &Reading,
    rules: &crate::redact::Rules,
    waiting: bool,
) -> Result<Option<Batch>> {
    pending_seen(raw, k, embedder, reading, rules, waiting, None)
}

/// What `pending` read past its batch, for the local model's threshold (D14): the documents of
/// each kind's first page with a text to send, and the oldest of them by its time.
#[derive(Default)]
struct Seen {
    count: usize,
    oldest: Option<i64>,
}

/// `pending`, which with `seen` reads each kind's first page with a text to send and counts it
/// there, the batch still the first kind's.
fn pending_seen(
    raw: &Raw,
    k: &Connection,
    embedder: &str,
    reading: &Reading,
    rules: &crate::redact::Rules,
    waiting: bool,
    mut seen: Option<&mut Seen>,
) -> Result<Option<Batch>> {
    let tombstones = raw.tombstones()?;
    crate::claims::schema(k)?;
    crate::consumer::imported::schema(k)?;
    crate::consumer::fts::schema(k)?;
    crate::cards::schema(k)?;
    crate::turns::schema(k)?;
    queue(k)?;
    let mut waits = None;
    for kind in ["c", "o", "s", "i", "r"] {
        loop {
            let page = read_page(raw, k, embedder, kind)?;
            if page.is_empty() {
                break;
            }
            let mut todo = Vec::new();
            let mut deferred = false;
            let tx = k.unchecked_transaction()?;
            let held = crate::claims::Pending::read(raw, &tx)?;
            for r in page {
                if claim_pending(raw, &held, &tx, &r.doc)? {
                    deferred = true;
                    continue;
                }
                todo.extend(sort_out(&tx, raw, reading, embedder, r, rules)?);
            }
            tx.commit()?;
            if todo.is_empty() {
                if deferred {
                    // No skip mark: the next consumer pass may leave the same claim body live.
                    break;
                }
                continue;
            }
            todo.sort_by_key(|(_, text)| text.chars().count());
            let pairs: Vec<(String, String)> = todo
                .iter()
                .enumerate()
                .map(|(i, (_, t))| (i.to_string(), t.clone()))
                .collect();
            let first = crate::embed::batches(&pairs)[0].len();
            if let Some(seen) = seen.as_deref_mut() {
                seen.count += todo.len();
                if let Some(oldest) = todo.iter().map(|(doc, _)| doc.ts).min() {
                    seen.oldest = Some(seen.oldest.map_or(oldest, |o| o.min(oldest)));
                }
            }
            let (docs, texts) = todo.into_iter().take(first).unzip();
            let batch = Batch {
                embedder: embedder.to_owned(),
                docs,
                texts,
                reading: reading.clone(),
                ruleset: rules.version().to_owned(),
                tombstones,
            };
            if !waiting && seen.is_none() {
                return Ok(Some(batch));
            }
            // ponytail: while a call waits, each kind is mapped up to its first page with a text
            // to send, not past it (`read_page` reads that page again until it is sent), so a
            // rebuild during a wait maps the rest of that kind when the wait ends. Page past it
            // with a cursor if that wait shows in search.
            waits.get_or_insert(batch);
            break;
        }
    }
    Ok(waits)
}

/// What becomes of `r`, a document read to embed: passed over and marked (D13, A92), mapped to the
/// vector its stored text already has, or its text to send, gated and cut (D8).
fn sort_out(
    k: &Connection,
    raw: &Raw,
    reading: &Reading,
    embedder: &str,
    r: Read,
    rules: &crate::redact::Rules,
) -> Result<Option<(Doc, String)>> {
    if let Some(why) = passed_over(raw, k, reading, &r, rules)? {
        mark(k, embedder, &r.doc, why)?;
        return Ok(None);
    }
    let sent = gated(&r, rules);
    if sent.trim().is_empty() || sent.trim() == "[REDACTED]" {
        // A text the rules blank is passed over under these rules only (`unmask`).
        let why = if r.text.trim().is_empty() {
            "empty".to_owned()
        } else {
            format!("masked:{}", rules.version())
        };
        mark(k, embedder, &r.doc, &why)?;
        return Ok(None);
    }
    if let Some(vec) = cached(k, embedder, &r.doc.sha)? {
        index(k, embedder, &r.doc, &vec)?;
        return Ok(None);
    }
    Ok(Some((r.doc, sent)))
}

/// The queue's trigger for a record: no tool output (#317, owner decision 42).
macro_rules! record_trigger {
    () => {
        "
    CREATE TRIGGER vector_todo_record AFTER INSERT ON raw_docs
      WHEN NEW.kind <> 'tool' AND NOT EXISTS (SELECT 1 FROM vector_keys
                       WHERE kind = 'r' AND key = NEW.device || ':' || NEW.seq)
    BEGIN
      INSERT OR IGNORE INTO vector_todo(family, key, ord)
        VALUES ('r', NEW.device || ':' || NEW.seq, NEW.seq);
    END;
"
    };
}

/// Step 13's queue: the imported documents and records with no key row, kept by triggers on the
/// tables that hold them and on `vector_keys`, so a poll reads a page of what waits and never the
/// whole store (over b-import's 178,370 documents each poll scanned them all, 0.7 s, waiting or
/// not). A new document is queued unless its key row exists (an import a second device holds), and
/// a key row that goes (`touched`, `cleared`) puts its document back; one keyed leaves the queue
/// when a read next reaches it (`queued`). Made
/// once for each knowledge.db, with what waits already: what its consumers wrote before the first
/// poll (a new home, a rebuild, a restore), or all of a file from before the queue. Claims stay a
/// read of `active`, a view, and few.
// ponytail: one embedder's queue (a new document's key row of any embedder keeps it out); the
// second embedder's generation (Task 10) needs a queue of its own.
const QUEUE: &str = concat!(
    "
    CREATE TABLE vector_todo(
      family TEXT NOT NULL, key TEXT NOT NULL, ord INTEGER NOT NULL,
      PRIMARY KEY (family, key)
    ) WITHOUT ROWID;
    CREATE INDEX vector_todo_ord ON vector_todo(family, ord);
    CREATE TRIGGER vector_todo_imported AFTER INSERT ON imported
      WHEN NOT EXISTS (SELECT 1 FROM vector_keys WHERE kind IN ('k', 'p') AND key = NEW.uid)
    BEGIN
      INSERT OR IGNORE INTO vector_todo(family, key, ord) VALUES ('i', NEW.uid, NEW.ts);
    END;
",
    record_trigger!(),
    "    CREATE TRIGGER vector_todo_unkeyed AFTER DELETE ON vector_keys WHEN OLD.kind <> 'c'
    BEGIN
      INSERT OR IGNORE INTO vector_todo(family, key, ord)
        SELECT 'i', OLD.key,
          COALESCE((SELECT ts FROM imported WHERE uid = OLD.key ORDER BY rowid DESC LIMIT 1), 0)
        WHERE OLD.kind IN ('k', 'p');
      INSERT OR IGNORE INTO vector_todo(family, key, ord)
        SELECT 'r', OLD.key, CAST(substr(OLD.key, instr(OLD.key, ':') + 1) AS INTEGER)
        WHERE OLD.kind = 'r';
    END;
    INSERT OR IGNORE INTO vector_todo(family, key, ord)
      SELECT 'i', i.uid, i.ts FROM imported i
      WHERE i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = i.uid)
        AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.kind IN ('k', 'p') AND v.key = i.uid);
    INSERT OR IGNORE INTO vector_todo(family, key, ord)
      SELECT 'r', d.device || ':' || d.seq, d.seq FROM raw_docs d
      WHERE d.kind <> 'tool' AND NOT EXISTS (SELECT 1 FROM vector_keys v
                        WHERE v.kind = 'r' AND v.key = d.device || ':' || d.seq);"
);

/// A home whose phase ran before tool output stayed out (#317): its trigger queued tool records and
/// its index may hold their vectors. The key rows go before the queue's: a deleted key row queues
/// its key again (`vector_todo_unkeyed`).
const UNTOOL: &str = concat!(
    "DROP TRIGGER vector_todo_record;",
    record_trigger!(),
    "
    DELETE FROM vec_index WHERE rowid IN (SELECT id FROM vector_keys WHERE kind = 'r'
      AND key IN (SELECT device || ':' || seq FROM raw_docs WHERE kind = 'tool'));
    DELETE FROM vector_keys WHERE kind = 'r'
      AND key IN (SELECT device || ':' || seq FROM raw_docs WHERE kind = 'tool');
    DELETE FROM vector_todo WHERE family = 'r'
      AND key IN (SELECT device || ':' || seq FROM raw_docs WHERE kind = 'tool');"
);

/// `QUEUE`, made in one transaction the first time a knowledge.db is polled; `UNTOOL` once for a
/// home whose record trigger is older.
fn queue(k: &Connection) -> Result<()> {
    let made: bool = k.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vector_todo')",
        [],
        |r| r.get(0),
    )?;
    if !made {
        let tx = k.unchecked_transaction()?;
        tx.execute_batch(QUEUE)?;
        tx.commit()?;
        return Ok(());
    }
    let trigger: Option<String> = k
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'vector_todo_record'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if trigger.is_some_and(|sql| !sql.contains("NEW.kind <> 'tool'")) {
        let tx = k.unchecked_transaction()?;
        tx.execute_batch(UNTOOL)?;
        tx.commit()?;
    }
    Ok(())
}

/// A page of `family`'s queued documents, the newest first, each as `read` finds it: a key whose
/// document is gone or has its key row leaves the queue, and the next page is read.
fn queued(
    k: &Connection,
    family: &str,
    mut read: impl FnMut(&str) -> Result<Option<Read>>,
) -> Result<Vec<Read>> {
    loop {
        let keys: Vec<String> = k
            .prepare_cached(
                "SELECT key FROM vector_todo WHERE family = ?1 ORDER BY ord DESC, key DESC LIMIT ?2",
            )?
            .query_map(params![family, PAGE as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let (mut out, mut gone) = (Vec::new(), Vec::new());
        for key in keys {
            match read(&key)? {
                Some(r) => out.push(r),
                None => gone.push(key),
            }
        }
        let tx = k.unchecked_transaction()?;
        for key in &gone {
            tx.execute(
                "DELETE FROM vector_todo WHERE family = ?1 AND key = ?2",
                params![family, key],
            )?;
        }
        tx.commit()?;
        if !out.is_empty() {
            return Ok(out);
        }
    }
}

/// A page of `kind`'s documents (`i` reads both imported kinds) with no key row for `embedder`,
/// the newest first: claims from `active`, current cards (`o`) from `cards`, summaries not skipped
/// (`s`) from `turns`, the rest from the queue.
fn read_page(raw: &Raw, k: &Connection, embedder: &str, kind: &str) -> Result<Vec<Read>> {
    let limit = PAGE as i64;
    Ok(match kind {
        "c" => k
            .prepare_cached(
                "SELECT a.uid, a.body, COALESCE(a.repo, ''), a.valid_from FROM active a
                 WHERE NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                                     AND v.kind = 'c' AND v.key = a.uid)
                 ORDER BY a.valid_from DESC LIMIT ?2",
            )?
            .query_map(params![embedder, limit], |r| {
                let text: String = r.get(1)?;
                Ok(Read {
                    doc: Doc {
                        kind: "c",
                        key: r.get(0)?,
                        sha: sha(&text),
                        repo: r.get(2)?,
                        ts: r.get(3)?,
                        session: String::new(),
                    },
                    text,
                    title: None,
                    labels: None,
                    parts: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
        "o" => k
            .prepare_cached(&format!(
                "SELECT {}, {CARD_DOC} FROM cards c WHERE replaced_by IS NULL AND hidden = 0
                   AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                     AND v.kind = 'o' AND v.key = c.device || '.' || c.op_seq || '.' || c.n)
                 ORDER BY ts DESC LIMIT ?2",
                crate::consumer::fts::CARD_COLUMNS
            ))?
            .query_map(params![embedder, limit], card_read)?
            .collect::<rusqlite::Result<_>>()?,
        "s" => k
            .prepare_cached(&format!(
                "SELECT {SUMMARY_DOC} FROM turns t WHERE skipped = 0 AND hidden = 0
                   AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                     AND v.kind = 's' AND v.key = 'S' || t.device || '.' || t.op_seq)
                 ORDER BY ts DESC LIMIT ?2"
            ))?
            .query_map(params![embedder, limit], summary_read)?
            .collect::<rusqlite::Result<_>>()?,
        "i" => queued(k, "i", |uid| {
            let mut read = k.prepare_cached(
                "SELECT i.kind, i.title, i.body, i.repo, i.ts, i.session FROM imported i
                 WHERE i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = ?2)
                   AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                     AND v.kind = CASE i.kind WHEN 'prompt' THEN 'p' ELSE 'k' END
                     AND v.key = i.uid)",
            )?;
            Ok(read
                .query_row(params![embedder, uid], |r| {
                    let (doc_kind, title, text): (String, String, String) =
                        (r.get(0)?, r.get(1)?, r.get(2)?);
                    Ok(Read {
                        doc: Doc {
                            kind: if doc_kind == "prompt" { "p" } else { "k" },
                            key: uid.to_owned(),
                            sha: sha(&composed(&doc_kind, &title, &text)),
                            repo: r.get(3)?,
                            ts: r.get(4)?,
                            session: r.get(5)?,
                        },
                        text,
                        title: Some((doc_kind, title)),
                        labels: None,
                        parts: Vec::new(),
                    })
                })
                .optional()?)
        })?,
        _ => queued(k, "r", |key| {
            let Some((device, seq)) = key
                .rsplit_once(':')
                .and_then(|(d, s)| Some((d, s.parse().ok()?)))
            else {
                return Ok(None);
            };
            let mut read = k.prepare_cached(
                "SELECT d.kind, d.ts, COALESCE(d.repo, ''), COALESCE(d.session, '')
                 FROM raw_docs d
                 WHERE d.device = ?2 AND d.seq = ?3 AND d.kind <> 'tool'
                   AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                                     AND v.kind = 'r' AND v.key = ?4)",
            )?;
            let row: Option<(String, i64, String, String)> = read
                .query_row(params![embedder, device, seq, key], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()?;
            let Some((record_kind, ts, repo, session)) = row else {
                return Ok(None);
            };
            // raw.db's text as it is now (#317).
            let Some((_, text)) = crate::consumer::fts::texts(raw, device, &[seq])?.pop() else {
                // docs/retention.md: an expired record has no text to send; its mark keeps it out
                // of the queue and the backlog.
                if raw.holds_expired(device, seq, seq)? {
                    k.execute(
                        "INSERT INTO vector_keys(embedder, kind, key, src_sha, skipped)
                         VALUES (?1, 'r', ?2, NULL, 'expired')
                         ON CONFLICT(kind, key, embedder) DO UPDATE SET src_sha = NULL,
                           skipped = 'expired'",
                        params![embedder, key],
                    )?;
                }
                return Ok(None);
            };
            Ok(Some(Read {
                doc: Doc {
                    kind: if record_kind == "prompt" { "rp" } else { "r" },
                    key: key.to_owned(),
                    sha: sha(&text),
                    repo,
                    ts,
                    session,
                },
                labels: raw.event_labels(device, seq)?,
                text,
                title: None,
                parts: Vec::new(),
            }))
        })?,
    })
}

/// What `card_read` reads after a card's `CARD_COLUMNS`: its key (the ID `cards::get` takes), its
/// repository, time and session.
const CARD_DOC: &str =
    "device || '.' || op_seq || '.' || n, COALESCE(repo, ''), ts, COALESCE(session, '')";

fn card_read(r: &rusqlite::Row) -> rusqlite::Result<Read> {
    let crate::consumer::fts::Joined { text, parts } = crate::consumer::fts::card_text(r, 0)?;
    Ok(Read {
        doc: Doc {
            kind: "o",
            key: r.get(7)?,
            sha: joined_sha(&text, &parts),
            repo: r.get(8)?,
            ts: r.get(9)?,
            session: r.get(10)?,
        },
        labels: None,
        text,
        title: None,
        parts,
    })
}

/// What `summary_read` reads of a summary: its fields, its key (the ID `turns::get` takes), its
/// repository and time, its session as `Raw::event_labels` spells it, and its session alone.
const SUMMARY_DOC: &str = "fields, 'S' || device || '.' || op_seq, COALESCE(repo, ''), ts,
    COALESCE(agent, '') || char(0) || COALESCE(session, ''), COALESCE(session, '')";

fn summary_read(r: &rusqlite::Row) -> rusqlite::Result<Read> {
    let crate::consumer::fts::Joined { text, parts } =
        crate::consumer::fts::turn_text(&r.get::<_, String>(0)?);
    Ok(Read {
        doc: Doc {
            kind: "s",
            key: r.get(1)?,
            sha: joined_sha(&text, &parts),
            repo: r.get(2)?,
            ts: r.get(3)?,
            session: r.get(5)?,
        },
        labels: Some((r.get(4)?, String::new())),
        text,
        title: None,
        parts,
    })
}

/// An imported document's text as v1 composed it (docs/pr-d.md): an observation's kind and title
/// over its body, a summary's or a prompt's body.
pub(crate) fn composed(kind: &str, title: &str, body: &str) -> String {
    composed_parts(kind, title, body).0
}

/// `composed`, with the byte ranges of the title and the body in it: one place for the format, so
/// the ranges `composed_out` gates alone cannot drift from it (OpenCodeReview on #312).
fn composed_parts(kind: &str, title: &str, body: &str) -> (String, Vec<std::ops::Range<usize>>) {
    match kind {
        "prompt" | "summary" => (body.to_owned(), Vec::new()),
        _ => {
            let head = format!("{kind}: ");
            let text = format!("{head}{title}\n{body}");
            let title_at = head.len()..head.len() + title.len();
            let body_at = title_at.end + 1..text.len();
            (text, vec![title_at, body_at])
        }
    }
}

/// `composed` for text that leaves this machine: gated whole, line by line, and in its title and
/// its body alone (`redact::outbound_joined`), as search gates an imported hit's title and body,
/// so a rule anchored to a title (`^...$`) holds once the kind is prefixed.
pub(crate) fn composed_out(kind: &str, title: &str, body: &str) -> String {
    let (text, parts) = composed_parts(kind, title, body);
    crate::redact::outbound_joined(&text, &parts)
}

pub(crate) fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// `sha` of a text gated value by value (`joined_with`), with where each value lies: values that
/// join to one text at other places gate differently, so they share no vector (Codex and
/// CodeRabbit on #429). Its `j` keeps it apart from every plain text's key, whatever that text
/// holds.
fn joined_sha(text: &str, parts: &[std::ops::Range<usize>]) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    for p in parts {
        h.update(format!("\0{}-{}", p.start, p.end));
    }
    format!("j{:x}", h.finalize())
}

/// A claim a pending correction or tombstone touches: its text may change on the next pass. A
/// forgotten one is not waited for: `passed_over` marks it (milestone 5 D5).
fn claim_pending(
    raw: &Raw,
    pending: &crate::claims::Pending,
    k: &Connection,
    doc: &Doc,
) -> Result<bool> {
    Ok(doc.kind == "c" && pending.touches(k, &doc.key)? && !raw.forgotten(&doc.key)?)
}

/// Why a document gets no vector without being sent: its repository or session is excluded
/// (D13), it is a record of an imported source (A92), or no reader shows it now.
fn passed_over(
    raw: &Raw,
    k: &Connection,
    reading: &Reading,
    r: &Read,
    rules: &crate::redact::Rules,
) -> Result<Option<&'static str>> {
    let list = &reading.exclusions;
    // A forgotten claim or document is sent nowhere (milestone 5 D5).
    if matches!(r.doc.kind, "c" | "k" | "p") && raw.forgotten(&r.doc.key)? {
        return Ok(Some("forgotten"));
    }
    match r.doc.kind {
        "c" => {
            let excluded = list.contains(&r.doc.repo)
                || crate::curate::quotes_excluded(raw, k, &reading.excluded, &r.doc.key)?;
            Ok(excluded.then_some("excluded"))
        }
        // Its project as stored and as the rules now read it, as a hook reads a label: one never
        // recorded under a label a rule masks is listed so (Codex on #489).
        "k" | "p" => {
            let now = crate::capture::label_with(&r.doc.repo, rules);
            let excluded = import_excluded(&r.doc.repo, list) || import_excluded(&now, list);
            Ok(excluded.then_some("excluded"))
        }
        "o" | "s" if r.text.trim().is_empty() => Ok(Some("empty")),
        "o" | "s" => {
            // A card or summary a removal hides (K4, T7) is sent nowhere, as a removed record is
            // not: asked of its one reader (K7).
            let shown = if r.doc.kind == "o" {
                crate::cards::get(k, raw, &r.doc.key, rules)?.is_some()
            } else {
                crate::turns::get(k, raw, &r.doc.key, rules)?.is_some()
            };
            if !shown {
                return Ok(Some("empty"));
            }
            let excluded =
                list.contains(&r.doc.repo) || made_from_excluded(raw, k, &reading.excluded, r)?;
            Ok(excluded.then_some("excluded"))
        }
        _ => {
            let Some((device, seq)) = r.doc.key.rsplit_once(':') else {
                return Ok(Some("empty"));
            };
            let live = crate::consumer::manifest::event(raw, device, seq.parse()?)?;
            if live.is_none_or(|e| crate::consumer::fts::text(&e.body) != r.text) {
                return Ok(Some("empty"));
            }
            Ok(match &r.labels {
                // A record the index holds and raw no longer does: its rewind comes.
                None => Some("empty"),
                Some((session, source)) => {
                    if reading.excluded.contains(session) {
                        Some("excluded")
                    } else if !crate::raw::is_live(source) {
                        Some("source")
                    } else {
                        None
                    }
                }
            })
        }
    }
}

/// Whether a card or a summary was made from a record of a session in `excluded` (D13,
/// docs/tools.md V2), as `curate::quotes_excluded` asks of a claim's quotes: a card's window and
/// goals (K4); a summary's own session, and the windows whose cards it read with their goals (T7).
/// A window's records of a session the list named when it was curated were kept back, unread, and
/// count too: the row keeps the range, not what was kept back, so such a card waits for the undo
/// as well.
fn made_from_excluded(
    raw: &Raw,
    k: &Connection,
    excluded: &std::collections::HashSet<String>,
    r: &Read,
) -> Result<bool> {
    if excluded.is_empty() {
        return Ok(false);
    }
    let not_a_key = || anyhow::anyhow!("not a key of its kind: {}", r.doc.key);
    let (device, spans, goals): (String, String, String) = if r.doc.kind == "o" {
        let (device, op_seq, n) = crate::cards::id_parts(&r.doc.key, "").ok_or_else(not_a_key)?;
        k.query_row(
            "SELECT device, json_array(json_array(from_seq, to_seq)), goals FROM cards
             WHERE device = ?1 AND op_seq = ?2 AND n = ?3",
            params![device, op_seq, n],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    } else {
        // A turn is its own session's records (T7): that session is all they are of.
        if r.labels
            .as_ref()
            .is_some_and(|(session, _)| excluded.contains(session))
        {
            return Ok(true);
        }
        let (device, op_seq) = crate::turns::id_parts(&r.doc.key, "").ok_or_else(not_a_key)?;
        k.query_row(
            "SELECT device, read, goals FROM turns WHERE device = ?1 AND op_seq = ?2",
            params![device, op_seq],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    };
    // As the readers read them (K4, T7): a list that does not parse is none.
    let spans: Vec<(i64, i64)> = serde_json::from_str(&spans).unwrap_or_default();
    let goals: Vec<i64> = serde_json::from_str(&goals).unwrap_or_default();
    let sessions = raw.sessions_over(&device, &spans, &goals)?;
    Ok(sessions.iter().any(|session| excluded.contains(session)))
}

/// Whether an imported document's repository is excluded: listed itself, or a claude-mem project
/// (and its worktree sessions, `claude-mem:<name>/…`) a listed repository reads, as search maps
/// them (`import::imported_repos`, D13).
pub(crate) fn import_excluded(repo: &str, list: &[String]) -> bool {
    let project = &crate::import::imported_repos(repo)[1];
    list.iter().any(|x| {
        x == repo
            || repo.starts_with("claude-mem:") && crate::import::imported_repos(x)[1] == *project
    })
}

/// The text sent for `r`: gated first (an imported document's fields each alone, `composed_out`),
/// then cut (12,000 characters; a prompt 1,000), so a secret across the cut is hidden whole (D8).
fn gated(r: &Read, rules: &crate::redact::Rules) -> String {
    let keep = match r.doc.kind {
        "p" | "rp" => crate::embed::PROMPT_CHARS,
        _ => crate::embed::MAX_CHARS,
    };
    match &r.title {
        Some((kind, title)) => {
            let (text, parts) = composed_parts(kind, title, &r.text);
            crate::redact::joined_with(&text, &parts, rules)
        }
        None if r.parts.is_empty() => crate::redact::lines_with(&r.text, rules),
        None => crate::redact::joined_with(&r.text, &r.parts, rules),
    }
    .chars()
    .take(keep)
    .collect()
}

/// The repository the index files a document under: an import's claude-mem project, its worktree
/// sessions' (`claude-mem:<name>/…`) included, so a search of one repository finds them all.
pub(crate) fn vec_repo<'a>(kind: &str, repo: &'a str) -> &'a str {
    match (kind, repo.strip_prefix("claude-mem:")) {
        ("k" | "p", Some(rest)) => rest
            .find('/')
            .map_or(repo, |i| &repo[.."claude-mem:".len() + i]),
        _ => repo,
    }
}

/// The index's kind of a document: a prompt record is a record.
fn index_kind(kind: &str) -> &str {
    if kind == "rp" { "r" } else { kind }
}

fn mark(k: &Connection, embedder: &str, doc: &Doc, why: &str) -> Result<()> {
    k.execute(
        "INSERT INTO vector_keys(embedder, kind, key, src_sha, skipped) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(embedder, kind, key) DO UPDATE SET src_sha = excluded.src_sha,
           skipped = excluded.skipped",
        params![embedder, index_kind(doc.kind), doc.key, doc.sha, why],
    )?;
    Ok(())
}

/// The texts held for an answer (`held` marks, Step 6), their marks taken off: each to be sent
/// again, alone. A mark keeps the index's kind, so a prompt record comes back as a record, which
/// `current` reads alike.
fn unhold(k: &Connection, embedder: &str) -> Result<Vec<Doc>> {
    let rows: Vec<(String, String, Option<String>)> = k
        .prepare_cached(
            "DELETE FROM vector_keys WHERE embedder = ?1 AND skipped = 'held'
             RETURNING kind, key, src_sha",
        )?
        .query_map([embedder], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .map(|(kind, key, sha)| Doc {
            kind: match kind.as_str() {
                "c" => "c",
                "o" => "o",
                "s" => "s",
                "k" => "k",
                "p" => "p",
                _ => "r",
            },
            key,
            sha: sha.unwrap_or_default(),
            repo: String::new(),
            ts: 0,
            session: String::new(),
        })
        .collect())
}

fn cached(k: &Connection, embedder: &str, sha: &str) -> Result<Option<Vec<u8>>> {
    Ok(k.query_row(
        "SELECT vec FROM vectors WHERE embedder = ?1 AND src_sha = ?2",
        params![embedder, sha],
        |r| r.get(0),
    )
    .optional()?)
}

/// `doc`'s key row mapped to the vector `vec` (fp32, little-endian), and its index row.
fn index(k: &Connection, embedder: &str, doc: &Doc, vec: &[u8]) -> Result<()> {
    let id: i64 = k.query_row(
        "INSERT INTO vector_keys(embedder, kind, key, src_sha, skipped) VALUES (?1, ?2, ?3, ?4, NULL)
         ON CONFLICT(embedder, kind, key) DO UPDATE SET src_sha = excluded.src_sha, skipped = NULL
         RETURNING id",
        params![embedder, index_kind(doc.kind), doc.key, doc.sha],
        |r| r.get(0),
    )?;
    let floats: Vec<f32> = vec
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    k.execute("DELETE FROM vec_index WHERE rowid = ?1", [id])?;
    k.execute(
        "INSERT INTO vec_index(rowid, embedder, kind, repo, ts, session, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, vec_bit(?7))",
        params![
            id,
            embedder,
            index_kind(doc.kind),
            vec_repo(doc.kind, &doc.repo),
            doc.ts,
            doc.session,
            crate::embed::bits(&floats)
        ],
    )?;
    Ok(())
}

/// The document `key` of `family` (`c` a claim, `o` a card, `s` a summary, `i` an import, `r` a
/// record) was written, replaced or removed, in the consumer's transaction that did it (row 46-2):
/// a key whose text is gone or differs loses its rows, so a poll gives it the vector of its text
/// now; the same text keeps its vector, under the document's repository and time now, but not an
/// `excluded` mark (D13), judged again. `raw` reads a record's text (#317); the others need none.
pub fn touched(raw: Option<&Raw>, k: &Connection, family: &str, key: &str) -> Result<()> {
    let kinds = match family {
        "i" => "'k', 'p'",
        "c" => "'c'",
        "o" => "'o'",
        "s" => "'s'",
        _ => "'r'",
    };
    let rows: Vec<(i64, String, Option<String>, Option<String>)> = k
        .prepare_cached(&format!(
            "SELECT id, kind, src_sha, skipped FROM vector_keys WHERE kind IN ({kinds}) AND key = ?1"
        ))?
        .query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let Some((_, kind, ..)) = rows.first() else {
        return Ok(());
    };
    let now = current(raw, k, kind, key)?.map(|r| r.doc);
    for (id, _, src_sha, skipped) in rows {
        match &now {
            // An `excluded` mark goes whatever the text: what excluded it may have changed.
            Some(d)
                if src_sha.as_ref() == Some(&d.sha) && skipped.as_deref() != Some("excluded") =>
            {
                if skipped.is_none() {
                    k.execute(
                        "UPDATE vec_index SET repo = ?2, ts = ?3, session = ?4 WHERE rowid = ?1",
                        params![id, vec_repo(d.kind, &d.repo), d.ts, d.session],
                    )?;
                }
            }
            _ => {
                k.execute("DELETE FROM vec_index WHERE rowid = ?1", [id])?;
                k.execute("DELETE FROM vector_keys WHERE id = ?1", [id])?;
            }
        }
    }
    Ok(())
}

/// A batch's vectors, cached and indexed, in one transaction: a document whose stored text is no
/// longer the one sent (a correction or a mask meanwhile) gets none (row 55-6).
fn write(raw: &Raw, k: &Connection, batch: &Batch, vecs: &[Vec<f32>]) -> Result<()> {
    let tx = k.unchecked_transaction()?;
    for (doc, vec) in batch.docs.iter().zip(vecs) {
        if !stored(raw, &tx, doc)? {
            continue;
        }
        let bytes: Vec<u8> = vec.iter().flat_map(|x| x.to_le_bytes()).collect();
        tx.execute(
            "INSERT OR IGNORE INTO vectors(embedder, src_sha, vec) VALUES (?1, ?2, ?3)",
            params![batch.embedder, doc.sha, bytes],
        )?;
        index(&tx, &batch.embedder, doc, &bytes)?;
    }
    tx.commit()?;
    Ok(())
}

/// Whether `doc`'s stored text is still the one it was read with.
fn stored(raw: &Raw, k: &Connection, doc: &Doc) -> Result<bool> {
    Ok(current(Some(raw), k, doc.kind, &doc.key)?.is_some_and(|r| r.doc.sha == doc.sha))
}

/// The document `key` of `kind` (`c`, `o`, `s`, `k` or `p` for an import, `r` or `rp` for a
/// record) as it is stored now, with its text, or None once it is gone (a card replaced, a summary
/// skipped). An imported uid's is its row with the highest rowid. A record's labels are left
/// unread, and its text is raw.db's (#317): `raw` is needed for a record the index still holds.
fn current(raw: Option<&Raw>, k: &Connection, kind: &str, key: &str) -> Result<Option<Read>> {
    let read = |doc: Doc, text: String| Read {
        doc,
        text,
        title: None,
        labels: None,
        parts: Vec::new(),
    };
    Ok(match kind {
        "c" => k
            .query_row(
                "SELECT body, COALESCE(repo, ''), valid_from FROM active WHERE uid = ?1",
                [key],
                |r| {
                    let text: String = r.get(0)?;
                    let doc = Doc {
                        kind: "c",
                        key: key.to_owned(),
                        sha: sha(&text),
                        repo: r.get(1)?,
                        ts: r.get(2)?,
                        session: String::new(),
                    };
                    Ok(read(doc, text))
                },
            )
            .optional()?,
        "k" | "p" => k
            .query_row(
                "SELECT kind, title, body, repo, ts, session FROM imported WHERE uid = ?1
                 ORDER BY rowid DESC LIMIT 1",
                [key],
                |r| {
                    let (doc_kind, title, text): (String, String, String) =
                        (r.get(0)?, r.get(1)?, r.get(2)?);
                    let doc = Doc {
                        kind: if doc_kind == "prompt" { "p" } else { "k" },
                        key: key.to_owned(),
                        sha: sha(&composed(&doc_kind, &title, &text)),
                        repo: r.get(3)?,
                        ts: r.get(4)?,
                        session: r.get(5)?,
                    };
                    Ok(Read {
                        title: Some((doc_kind, title)),
                        ..read(doc, text)
                    })
                },
            )
            .optional()?,
        "o" => {
            let Some((device, op_seq, n)) = crate::cards::id_parts(key, "") else {
                return Ok(None);
            };
            k.query_row(
                &format!(
                    "SELECT {}, {CARD_DOC} FROM cards
                     WHERE device = ?1 AND op_seq = ?2 AND n = ?3 AND replaced_by IS NULL
                       AND hidden = 0",
                    crate::consumer::fts::CARD_COLUMNS
                ),
                params![device, op_seq, n],
                card_read,
            )
            .optional()?
        }
        "s" => {
            let Some((device, op_seq)) = crate::turns::id_parts(key, "") else {
                return Ok(None);
            };
            k.query_row(
                &format!(
                    "SELECT {SUMMARY_DOC} FROM turns
                     WHERE device = ?1 AND op_seq = ?2 AND skipped = 0 AND hidden = 0"
                ),
                params![device, op_seq],
                summary_read,
            )
            .optional()?
        }
        _ => {
            let Some((device, seq)) = key.rsplit_once(':') else {
                return Ok(None);
            };
            let seq = seq.parse::<i64>().unwrap_or(-1);
            let row: Option<(String, i64, String, String)> = k
                .query_row(
                    "SELECT kind, ts, COALESCE(repo, ''), COALESCE(session, '') FROM raw_docs
                     WHERE device = ?1 AND seq = ?2 AND kind <> 'tool'",
                    params![device, seq],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((record_kind, ts, repo, session)) = row else {
                return Ok(None);
            };
            let raw = anyhow::Context::context(raw, "a record's text is read from raw.db")?;
            crate::consumer::fts::texts(raw, device, &[seq])?
                .pop()
                .map(|(_, text)| {
                    let doc = Doc {
                        kind: if record_kind == "prompt" { "rp" } else { "r" },
                        key: key.to_owned(),
                        sha: sha(&text),
                        repo,
                        ts,
                        session,
                    };
                    read(doc, text)
                })
        }
    })
}

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests;
