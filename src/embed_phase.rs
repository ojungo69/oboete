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
}

/// Rules loaded, raw.db opened, exclusions and tombstones checked against the batch's, raw.db
/// closed: what `send` and `send_local` do before the model runs. A call that leaves the machine
/// holds the dispatch guard, which a forget waits for.
fn still(home: &Path, batch: &Batch, leaves: bool) -> Result<Option<crate::dispatch::Guard>> {
    let rules = crate::redact::Rules::load(home)?;
    anyhow::ensure!(
        rules.version() == batch.ruleset,
        "the redaction rules changed since the batch was composed"
    );
    let raw = crate::raw::open(home)?;
    let dispatch = leaves.then(|| raw.dispatch()).transpose()?;
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
        carry_set_aside(&self.home, k)?;
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
        cleared(k, crate::embed::EMBEDDER, &reading)?;
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
                if claim_pending(&held, &tx, &r.doc)? {
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
    let cards = count(
        "SELECT count(*) FROM cards c WHERE c.replaced_by IS NULL AND NOT EXISTS (
           SELECT 1 FROM vector_keys v WHERE v.embedder = ?1 AND v.kind = 'o'
             AND v.key = c.device || '.' || c.op_seq || '.' || c.n)",
    );
    let summaries = count(
        "SELECT count(*) FROM turns t WHERE t.skipped = 0 AND NOT EXISTS (
           SELECT 1 FROM vector_keys v WHERE v.embedder = ?1 AND v.kind = 's'
             AND v.key = 'S' || t.device || '.' || t.op_seq)",
    );
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
            "SELECT skipped, count(*) FROM vector_keys WHERE embedder = ?1 AND skipped IS NOT NULL
             GROUP BY skipped ORDER BY skipped",
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
/// into `k`'s cache, so the documents they were made for are mapped again with no call. Only
/// blobs of one unit vector each; a file from before the cache carries nothing. One
/// transaction: a read that fails carries nothing.
// ponytail: the cache keeps every text ever embedded, the old texts of changed documents too (a
// rewind maps them back for free); sweep unreferenced rows if it grows past what that is worth.
pub fn carry(k: &Connection, from: &Path) -> Result<usize> {
    let old = Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has: bool = old.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vectors')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        return Ok(0);
    }
    let tx = k.unchecked_transaction()?;
    let mut carried = 0;
    {
        let mut read = old.prepare("SELECT embedder, src_sha, vec FROM vectors")?;
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
/// cannot be read carries nothing, and says so once.
fn carry_set_aside(home: &Path, k: &Connection) -> Result<()> {
    use crate::knowledge::checkpoint;
    let done = checkpoint::get(k, "carried", "")?;
    let aside: Vec<(i64, String)> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let at = ["knowledge.db.quarantined-", "knowledge.db.rebuilding-"]
                .iter()
                .find_map(|p| name.strip_prefix(p))?
                .parse()
                .ok()?;
            Some((at, name))
        })
        .filter(|(at, _)| *at > done)
        .collect();
    for (_, name) in &aside {
        if let Err(e) = carry(k, &home.join(name)) {
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
/// so undoing an exclusion queues its documents again (D13).
fn cleared(k: &Connection, embedder: &str, reading: &Reading) -> Result<()> {
    let list = serde_json::to_string(&*reading.exclusions)?;
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
                if claim_pending(&held, &tx, &r.doc)? {
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
        mark(k, embedder, &r.doc, "empty")?;
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
                "SELECT {}, {CARD_DOC} FROM cards c WHERE replaced_by IS NULL
                   AND NOT EXISTS (SELECT 1 FROM vector_keys v WHERE v.embedder = ?1
                     AND v.kind = 'o' AND v.key = c.device || '.' || c.op_seq || '.' || c.n)
                 ORDER BY ts DESC LIMIT ?2",
                crate::consumer::fts::CARD_COLUMNS
            ))?
            .query_map(params![embedder, limit], card_read)?
            .collect::<rusqlite::Result<_>>()?,
        "s" => k
            .prepare_cached(&format!(
                "SELECT {SUMMARY_DOC} FROM turns t WHERE skipped = 0
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
/// CodeRabbit on #429).
fn joined_sha(text: &str, parts: &[std::ops::Range<usize>]) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    for p in parts {
        h.update(format!("\0{}-{}", p.start, p.end));
    }
    format!("{:x}", h.finalize())
}

/// A claim a pending correction or tombstone touches: its text may change on the next pass.
fn claim_pending(pending: &crate::claims::Pending, k: &Connection, doc: &Doc) -> Result<bool> {
    Ok(doc.kind == "c" && pending.touches(k, &doc.key)?)
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
    match r.doc.kind {
        "c" => {
            let excluded = list.contains(&r.doc.repo)
                || crate::curate::quotes_excluded(raw, k, &reading.excluded, &r.doc.key)?;
            Ok(excluded.then_some("excluded"))
        }
        "k" | "p" => Ok(import_excluded(&r.doc.repo, list).then_some("excluded")),
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
/// (and its worktree sessions, `claude-mem:<name>/…`) named by a listed repository's last part,
/// as search maps them (D13).
pub(crate) fn import_excluded(repo: &str, list: &[String]) -> bool {
    if list.iter().any(|x| x == repo) {
        return true;
    }
    let Some(name) = repo.strip_prefix("claude-mem:") else {
        return false;
    };
    let project = crate::import::repo(name.split('/').next().unwrap_or(name));
    list.iter()
        .any(|x| *x == project || crate::import::repo(x.rsplit('/').next().unwrap_or(x)) == project)
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
fn vec_repo<'a>(kind: &str, repo: &'a str) -> &'a str {
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
                     WHERE device = ?1 AND op_seq = ?2 AND n = ?3 AND replaced_by IS NULL",
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
                     WHERE device = ?1 AND op_seq = ?2 AND skipped = 0"
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
pub(crate) mod fixture {
    use super::*;
    use crate::embed::stub::Stub;
    use crate::search::b::fixture::Store;

    /// A home whose `[embedding]` points at `stub`, with a key file.
    pub(crate) fn config(s: &Store, stub: &Stub) {
        config_at(s, &stub.url);
    }

    pub(crate) fn config_at(s: &Store, url: &str) {
        let key = s.home.path().join("key.md");
        std::fs::write(&key, "workers ai\nk\n").unwrap();
        let text = format!(
            "[embedding]\nprovider = \"workers-ai\"\naccount_id = \"a\"\nkey_file = '{}'\nurl = \"{}\"\n",
            key.display(),
            url
        );
        std::fs::write(s.home.path().join("config.toml"), text).unwrap();
    }

    /// Deterministic embedder answers without a loopback listener.
    pub(crate) fn vectors(s: &Store) {
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::load(s.home.path()).unwrap();
        let id = crate::embed::EMBEDDER;
        cleared(&k, id, &reading).unwrap();
        while let Some(batch) = pending(&s.raw, &k, id, &reading, &rules, false).unwrap() {
            let vectors: Vec<_> = batch
                .texts
                .iter()
                .map(|t| crate::embed::stub::vector(id, t))
                .collect();
            write(&s.raw, &k, &batch, &vectors).unwrap();
        }
    }

    pub(crate) fn answer(phase: &mut Phase, key: &str, text: &str) {
        phase.answered = Some(Answer {
            key: key.to_owned(),
            text: text.to_owned(),
            embedder: crate::embed::EMBEDDER.to_owned(),
            vector: crate::embed::stub::vector(crate::embed::EMBEDDER, text),
        });
    }

    pub(crate) fn hold_query(
        phase: &mut Phase,
        key: &str,
        text: &str,
        sent: Sent,
    ) -> std::sync::mpsc::Sender<()> {
        let call = reserve(
            phase.providers().unwrap(),
            "query",
            "1 query",
            text,
            200,
            1.0,
        )
        .unwrap()
        .unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        phase.asked = Some(Asked {
            key: key.to_owned(),
            text: text.to_owned(),
            embedder: crate::embed::EMBEDDER.to_owned(),
            call: Some(call),
            thread: std::thread::spawn(move || {
                wait.recv().unwrap();
                (sent, 0)
            }),
            until: crate::db::now_ms() + 10_000,
        });
        release
    }

    pub(crate) fn query_text(phase: &Phase) -> Option<&str> {
        phase.asked.as_ref().map(|a| a.text.as_str())
    }

    /// The phase polled until it has nothing left, each call waited for.
    pub(crate) fn embed_all(s: &Store) {
        let k = crate::knowledge::open(s.home.path()).unwrap();
        until_idle(&s.raw, &k, &mut Phase::new(s.home.path()));
    }

    pub(crate) fn until_idle(raw: &Raw, k: &Connection, phase: &mut Phase) {
        for _ in 0..100 {
            match phase.poll(raw, k).unwrap() {
                Step::Idle => return,
                Step::Waiting { .. } if phase.flight.is_some() || phase.asked.is_some() => {
                    while !phase.done() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                Step::Waiting { until, .. } => panic!("the phase waits until {until}"),
                Step::Covered => {}
            }
        }
        panic!("the phase never went idle");
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::embed::stub::Stub;
    use crate::search::b::fixture::Store;

    const R: &str = "github.com/o/r";

    /// A current native record's registration waits for a batch already through its final
    /// check to send. Hooks still append, and registration completes before model headers.
    #[test]
    fn a_native_forget_orders_with_dispatch_without_waiting_for_embedding_headers() {
        use std::sync::mpsc::{RecvTimeoutError, sync_channel};
        const CANARY: &str = "native-dispatch-forget-canary-901";
        let mut s = Store::new();
        let stub = Stub::start();
        config(&s, &stub);
        let response = stub.hold();
        let mut event = crate::raw::test_event(&serde_json::json!({"prompt": CANARY}).to_string());
        event.source = "transcript".into();
        event.kind = "prompt".into();
        event.repo = Some(R.into());
        let identity = crate::raw::ImportIdentity {
            origin: crate::forget::origin("synthetic-dispatch", "native-901"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = s
            .raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                std::slice::from_ref(&identity),
                "",
                None,
            )
            .unwrap()[0];
        // Imported raw is held for recuration, but an ordinary claim anchored in it is an
        // active embedding document. Forget still targets the genuine native raw record.
        derive(&mut s, seq, CANARY, CANARY);
        s.run();
        let home = s.home.path().to_owned();
        let preview = crate::forget::preview(
            &home,
            crate::forget::Target::Record {
                device: s.raw.device().into(),
                seq,
            },
        )
        .unwrap();
        assert_eq!(
            preview.count(),
            1,
            "the fixture is a genuinely eligible native record"
        );
        let request = crate::forget::previous_transcript_request(&s.raw, seq, &identity);
        let k = crate::knowledge::open(&home).unwrap();
        let loaded = crate::config::load(&home).unwrap();
        let embedder = Embedder::from_config(&loaded.embedding).unwrap().unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::load(&home).unwrap();
        let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
            .unwrap()
            .unwrap();
        assert!(batch.texts.iter().any(|t| t.contains(CANARY)));
        let (checked_at, checked) = sync_channel(1);
        let (transmit, begin) = sync_channel(1);
        let sending_home = home.clone();
        let sender = std::thread::spawn(move || {
            AFTER_SEND_CHECK.with(|at| {
                *at.borrow_mut() = Some(Box::new(move || {
                    checked_at.send(()).unwrap();
                    begin.recv_timeout(Duration::from_secs(3)).unwrap();
                }));
            });
            send(&sending_home, &batch, &embedder, Duration::from_secs(3))
        });
        checked.recv_timeout(Duration::from_secs(3)).unwrap();
        let (started_at, started) = sync_channel(1);
        let (registered_at, registered) = sync_channel(1);
        let registering_home = home.clone();
        let forget = std::thread::spawn(move || {
            started_at.send(()).unwrap();
            let result = crate::raw::open(&registering_home)
                .and_then(|mut raw| raw.forget_apply(&[request]));
            registered_at.send(result.is_ok()).unwrap();
            result
        });
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        // The dispatch hold is independent of raw's writer transaction.
        crate::raw::open(&home)
            .unwrap()
            .append(&crate::raw::test_event(
                "an unrelated hook while native dispatch is paused",
            ))
            .unwrap();
        let early = match registered.recv_timeout(Duration::from_millis(100)) {
            Ok(success) => {
                assert!(success);
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(e) => panic!("forget thread ended: {e}"),
        };
        transmit.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while stub.requests() == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(stub.requests(), 1);
        let before_headers = early || registered.recv_timeout(Duration::from_secs(1)).unwrap();
        let result = forget.join().unwrap().unwrap();
        assert_eq!(result, 1);
        assert_eq!(
            stub.answered(),
            0,
            "the model response headers are still withheld"
        );
        drop(response);
        assert!(matches!(sender.join().unwrap().0, Sent::Vectors(_)));
        assert!(
            !early,
            "native forget committed after the send check but before transmission"
        );
        assert!(
            before_headers,
            "registration waited for the embedding response"
        );
    }

    /// A decided decision of `R` that quotes a new record (`quote`, whole), with its own `body`.
    fn claim(s: &mut Store, quote: &str, body: &str) -> String {
        let seq = s.said("s", R, 1_000, quote);
        derive(s, seq, quote, body)
    }

    /// A claim op that quotes record `seq` (`quote`, whole) with `body`: a new claim, or a
    /// re-derivation of the one that quotes the same.
    fn derive(s: &mut Store, seq: i64, quote: &str, body: &str) -> String {
        use crate::claims::{ClaimOp, Evidence};
        let evidence = Evidence {
            device: s.raw.device().to_owned(),
            seq,
            offset: 0,
            length: quote.len() as i64,
            sentence: 0,
            quote: quote.into(),
            claim_at: None,
        };
        let uid = crate::claims::uid("decision", &evidence);
        let op = ClaimOp {
            id: "c1".into(),
            kind: "decision".into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: body.into(),
            evidence: vec![evidence],
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
        };
        let op = serde_json::to_value(op).unwrap();
        let ops = [(crate::raw::OpKind::Claim, op)];
        s.raw.append_ops(&ops).unwrap();
        uid
    }

    /// The day's embedding requests spent, as earlier calls would have.
    fn spend_the_day(home: &Path) {
        use crate::providers_db as pdb;
        let db = pdb::open(home).unwrap();
        for _ in 0..200 {
            let call = pdb::Call {
                provider: crate::embed::CALLS,
                role: ROLE,
                span: "",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: Some(1),
                usage: pdb::Usage::default(),
                usd: None,
            };
            pdb::record(&db, &call).unwrap();
        }
    }

    fn keys(s: &Store) -> Vec<(String, String, Option<String>)> {
        let k = crate::knowledge::open(s.home.path()).unwrap();
        k.prepare("SELECT kind, key, skipped FROM vector_keys ORDER BY kind, key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn skipped(s: &Store, key: &str) -> Option<String> {
        keys(s)
            .into_iter()
            .find(|(_, k, _)| k == key)
            .and_then(|(.., why)| why)
    }

    /// Row 30-1 (D13): nothing of an excluded repository reaches the embedder: its claims, a claim
    /// elsewhere that quotes a session that touched it, its claude-mem project and that project's
    /// worktree sessions, and that session's records in any repository. The rest is sent.
    #[test]
    fn an_excluded_repositorys_documents_reach_no_embedder() {
        const X: &str = "github.com/o/secret";
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let there = s.said("sx", X, 1_000, "Secret words there.");
        let here = s.said("sx", R, 2_000, "Secret words said here.");
        let secret = [
            derive(&mut s, there, "Secret words there.", "Secret words there."),
            derive(
                &mut s,
                here,
                "Secret words said here.",
                "Secret words said here.",
            ),
            s.imported(
                "o1",
                "secret",
                3_000,
                "Secret notes",
                "Secret project notes.",
            ),
            s.imported(
                "o2",
                "secret/wt",
                3_000,
                "Secret tree",
                "Secret worktree notes.",
            ),
            s.key(there),
            s.key(here),
        ];
        let open = s.said("so", R, 4_000, "Open words.");
        derive(&mut s, open, "Open words.", "Open words.");
        s.imported("o3", "r", 4_000, "Open notes", "Open project notes.");
        s.raw.exclude(X, false).unwrap();
        s.run();
        embed_all(&s);
        let sent = stub.texts().concat();
        assert!(sent.iter().any(|t| t == "Open words."), "{sent:?}");
        assert!(sent.iter().all(|t| !t.contains("Secret")), "{sent:?}");
        for key in &secret {
            assert_eq!(skipped(&s, key).as_deref(), Some("excluded"), "{key}");
        }
    }

    /// Tools slice 3 (V1, V3): cards and summaries are embedded after the claims, a skipped summary
    /// is not, and each vector follows its row: a recuration's replaced card loses its vector and
    /// the new card gets one, a rewind takes them off and maps the card it brings back with no
    /// call, and a rebuild sends nothing.
    #[test]
    fn a_cards_and_a_summarys_vectors_follow_their_rows() {
        use crate::consumer::{cards::Cards, turns::Turns};
        use crate::worker::Consumer;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let quote = "Parser caches stay in Redis.";
        let seq = s.said("s", R, 1_000, quote);
        derive(&mut s, seq, quote, quote);
        let first = s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "bugfix", "title": "Azimuth"}]),
            false,
        );
        let turn = |skipped: bool| {
            serde_json::json!({"agent": "claude", "session": "s", "repo": R, "ts": 2_000,
                "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
                "fields": if skipped { serde_json::json!({}) }
                          else { serde_json::json!({"request": "Fix the parser."}) },
                "skipped": skipped})
        };
        let before_turns = s.raw.max_op_seq().unwrap();
        let summary = s.turn(turn(false));
        s.turn(turn(true));
        s.run();
        embed_all(&s);
        let device = s.raw.device().to_owned();
        let kept = |s: &Store, kind: &str| -> Vec<String> {
            keys(s)
                .into_iter()
                .filter(|(k, _, why)| k == kind && why.is_none())
                .map(|(_, key, _)| key)
                .collect()
        };
        assert_eq!(stub.texts()[0], [quote], "the claims' batch first");
        assert!(
            stub.texts()
                .concat()
                .iter()
                .any(|t| t.starts_with("Azimuth"))
        );
        assert_eq!(kept(&s, "o"), [format!("{device}.{}", first[0])]);
        let summary_key = format!("S{device}.{}", &summary[1..]);
        assert_eq!(kept(&s, "s"), std::slice::from_ref(&summary_key));
        // A recuration replaces the card.
        let before = s.raw.max_op_seq().unwrap();
        let second = s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "feature", "title": "Nebula"}]),
            true,
        );
        s.run();
        embed_all(&s);
        assert_eq!(kept(&s, "o"), [format!("{device}.{}", second[0])]);
        // A restore that lost the recuration: the card it replaced is current again, mapped to the
        // vector it had, and a lost summary's vector goes.
        let sent = stub.requests();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        Cards.rewind(&k, &device, before).unwrap();
        Turns.rewind(&k, &device, before_turns).unwrap();
        assert!(kept(&s, "o").is_empty() && kept(&s, "s").is_empty());
        embed_all(&s);
        assert_eq!(kept(&s, "o"), [format!("{device}.{}", first[0])]);
        assert_eq!(stub.requests(), sent);
        // A rebuild, with no store open as `oboete rebuild` runs, makes the rows again from the op
        // log, and their vectors from the ones kept.
        drop(k);
        let Store { home, raw } = s;
        drop(raw);
        crate::worker::rebuild(home.path()).unwrap();
        let s = Store {
            raw: crate::raw::open(home.path()).unwrap(),
            home,
        };
        embed_all(&s);
        assert_eq!(kept(&s, "o"), [format!("{device}.{}", second[0])]);
        assert_eq!(kept(&s, "s"), [summary_key]);
        assert_eq!(stub.requests(), sent);
    }

    /// Tools slice 3 (V2, D13): an excluded repository's card and summary, those of a session
    /// that touched it, and those made from one of its records (a window across sessions, a goal,
    /// a window a summary read) are not sent; each is marked `excluded`. The rest is sent.
    #[test]
    fn an_excluded_repositorys_cards_and_summaries_reach_no_embedder() {
        const X: &str = "github.com/o/secret";
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let there = s.said("sx", X, 1_000, "Secret words there.");
        let elsewhere = s.said("sx", R, 2_000, "Words said here.");
        let open = s.said("so", R, 3_000, "Open words.");
        let card = |s: &mut Store, seq, title: &str| {
            s.cards(
                seq,
                seq,
                serde_json::json!([{"type": "bugfix", "title": title}]),
                false,
            )[0]
            .clone()
        };
        let turn = |s: &mut Store, session: &str, repo: &str, seq, request: &str| {
            s.turn(
                serde_json::json!({"agent": "claude", "session": session, "repo": repo,
                "ts": 4_000, "from": seq, "through": seq, "read": [], "goals": [],
                "removed": [], "fields": {"request": request}, "skipped": false}),
            )
        };
        let device = s.raw.device().to_owned();
        // K4's records: a window over two sessions has no session or repository label of its own,
        // and a goal is a record its curator was shown.
        let mixed = s.cards(
            elsewhere,
            open,
            serde_json::json!([{"type": "bugfix", "title": "Secret card across sessions"}]),
            false,
        )[0]
        .clone();
        let goal = s
            .raw
            .append_ops(&[(
                crate::raw::OpKind::Window,
                serde_json::json!({"outcome": "curated", "summary": "", "from_seq": open,
                "to_seq": open, "from_offset": null, "to_offset": null, "elided": [],
                "removed": [], "goals": [there], "recurate": false,
                "observations": [{"type": "bugfix", "title": "Secret card of a goal"}]}),
            )])
            .unwrap()[0];
        // T7's: the windows whose cards a summary read, and their goals.
        let read = |s: &mut Store, read: serde_json::Value, goals: serde_json::Value, request| {
            s.turn(
                serde_json::json!({"agent": "claude", "session": "so", "repo": R,
                "ts": 4_000, "from": open, "through": open, "read": read, "goals": goals,
                "removed": [], "fields": {"request": request}, "skipped": false}),
            )
        };
        let read_there = read(
            &mut s,
            serde_json::json!([[there, there]]),
            serde_json::json!([]),
            "Secret summary of what it read",
        );
        let goal_there = read(
            &mut s,
            serde_json::json!([]),
            serde_json::json!([there]),
            "Secret summary of a goal",
        );
        let not_sent = [
            format!("{device}.{}", card(&mut s, there, "Secret card there")),
            format!("{device}.{}", card(&mut s, elsewhere, "Secret card here")),
            format!("{device}.{mixed}"),
            format!("{device}.{goal}.0"),
            format!(
                "S{device}.{}",
                &turn(&mut s, "sx", X, there, "Secret summary")[1..]
            ),
            format!(
                "S{device}.{}",
                &turn(&mut s, "sx", R, elsewhere, "Secret summary here")[1..]
            ),
            format!("S{device}.{}", &read_there[1..]),
            format!("S{device}.{}", &goal_there[1..]),
        ];
        card(&mut s, open, "Open card");
        turn(&mut s, "so", R, open, "Open summary");
        s.raw.exclude(X, false).unwrap();
        s.run();
        embed_all(&s);
        let sent = stub.texts().concat();
        assert!(sent.iter().any(|t| t.starts_with("Open card")), "{sent:?}");
        assert!(sent.iter().any(|t| t == "Open summary"), "{sent:?}");
        assert!(sent.iter().all(|t| !t.contains("Secret")), "{sent:?}");
        for key in &not_sent {
            assert_eq!(skipped(&s, key).as_deref(), Some("excluded"), "{key}");
        }
    }

    /// Tools slice 3 (V2, K4, T7): a card and a summary a removal hides before they are embedded
    /// are not sent; each is marked `empty`, as a removed record is.
    #[test]
    fn a_card_and_a_summary_a_removal_hides_reach_no_embedder() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let gone = s.said("s", R, 1_000, "Gone words.");
        let card = s.cards(
            gone,
            gone,
            serde_json::json!([{"type": "bugfix", "title": "Hidden card"}]),
            false,
        )[0]
        .clone();
        let summary = s.turn(
            serde_json::json!({"agent": "claude", "session": "s", "repo": R,
            "ts": 2_000, "from": gone, "through": gone, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Hidden summary"}, "skipped": false}),
        );
        let device = s.raw.device().to_owned();
        s.raw
            .append_tombstone(crate::raw::Target::Record {
                device: device.clone(),
                seq: gone,
            })
            .unwrap();
        s.run();
        embed_all(&s);
        let sent = stub.texts().concat();
        assert!(sent.iter().all(|t| !t.contains("Hidden")), "{sent:?}");
        for key in [
            format!("{device}.{card}"),
            format!("S{device}.{}", &summary[1..]),
        ] {
            assert_eq!(skipped(&s, &key).as_deref(), Some("empty"), "{key}");
        }
    }

    /// Tools slice 3 (V1, K6; Codex): each value of a card or a summary is gated alone, as its
    /// reader gates it, so a rule on a whole value that spans its lines holds in the text sent.
    #[test]
    fn a_rule_on_a_whole_value_of_a_card_or_summary_holds_in_the_text_sent() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let path = s.home.path().join("config.toml");
        let rule =
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^ACME\\ncode=[0-9]{6}$' }]\n";
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + rule).unwrap();
        let seq = s.said("s", R, 1_000, "Open words.");
        s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "bugfix", "title": "Card title",
                "facts": ["ACME\ncode=123456"]}]),
            false,
        );
        s.turn(
            serde_json::json!({"agent": "claude", "session": "s", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Summary request", "learned": "ACME\ncode=654321"},
            "skipped": false}),
        );
        s.run();
        embed_all(&s);
        let sent = stub.texts().concat();
        assert!(sent.iter().any(|t| t.starts_with("Card title")), "{sent:?}");
        assert!(
            sent.iter().any(|t| t.starts_with("Summary request")),
            "{sent:?}"
        );
        assert!(
            sent.iter()
                .all(|t| !t.contains("123456") && !t.contains("654321")),
            "{sent:?}"
        );
    }

    /// Codex and CodeRabbit on #429: a card's or a summary's vector is reused only by a row whose
    /// values join to the same text at the same places. Values split differently gate differently
    /// (`joined_with`), so the second row's own gated text is sent, not mapped to the first's.
    #[test]
    fn values_joined_to_one_text_at_other_places_share_no_vector() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let path = s.home.path().join("config.toml");
        let rule =
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^ACME\\ncode=[0-9]{6}$' }]\n";
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + rule).unwrap();
        let summary = |s: &mut Store, session: &str, fields: serde_json::Value| {
            let seq = s.said(session, R, 1_000, "Open words.");
            s.turn(
                serde_json::json!({"agent": "claude", "session": session, "repo": R,
                "ts": 2_000, "from": seq, "through": seq, "read": [], "goals": [],
                "removed": [], "fields": fields, "skipped": false}),
            );
        };
        let card = |s: &mut Store, session: &str, facts: serde_json::Value| {
            let seq = s.said(session, R, 1_000, "Open words.");
            s.cards(
                seq,
                seq,
                serde_json::json!([{"type": "bugfix", "title": "Card", "facts": facts}]),
                false,
            );
        };
        // Neither value is the rule's whole: the code goes out with the rest.
        summary(
            &mut s,
            "a",
            serde_json::json!({"request": "Prefix\nACME\ncode=123456"}),
        );
        card(&mut s, "b", serde_json::json!(["ACME", "code=654321"]));
        s.run();
        embed_all(&s);
        let first = stub.texts().concat();
        assert!(first.iter().any(|t| t.contains("123456")), "{first:?}");
        assert!(first.iter().any(|t| t.contains("654321")), "{first:?}");
        let before = stub.requests();
        // The same texts, one value the rule's whole in each.
        summary(
            &mut s,
            "c",
            serde_json::json!({"request": "Prefix", "learned": "ACME\ncode=123456"}),
        );
        card(&mut s, "d", serde_json::json!(["ACME\ncode=654321"]));
        s.run();
        embed_all(&s);
        let later = stub.texts()[before..].concat();
        assert!(later.iter().any(|t| t.starts_with("Prefix")), "{later:?}");
        assert!(later.iter().any(|t| t.starts_with("Card")), "{later:?}");
        assert!(
            later
                .iter()
                .all(|t| !t.contains("123456") && !t.contains("654321")),
            "{later:?}"
        );
    }

    /// Tools slice 3 (Codex): cards held for an answer (Step 6) come back as cards, each sent once
    /// more alone after another document's answer, not read again as one batch.
    #[test]
    fn held_cards_are_sent_again_each_alone() {
        use crate::providers_db as pdb;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let seq = s.said("s", R, 1_000, "Open words.");
        s.run();
        embed_all(&s);
        let db = pdb::open(s.home.path()).unwrap();
        let mut held = Vec::new();
        for title in ["First card", "Second card"] {
            let card = s.cards(
                seq,
                seq,
                serde_json::json!([{"type": "bugfix", "title": title}]),
                false,
            )[0]
            .clone();
            stub.fail_next(400, None);
            s.run();
            embed_all(&s);
            let key = format!("{}.{card}", s.raw.device());
            assert_eq!(skipped(&s, &key).as_deref(), Some("held"));
            // A 400 alone is one failure and no rest: cleared, so the next card is alone too.
            pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
            held.push(key);
        }
        let sent = stub.requests();
        s.said("s", R, 2_000, "Later words.");
        s.run();
        embed_all(&s);
        let texts = &stub.texts()[sent..];
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(texts.iter().any(|t| *t == ["First card"]), "{texts:?}");
        assert!(texts.iter().any(|t| *t == ["Second card"]), "{texts:?}");
        for key in &held {
            assert!(
                keys(&s).iter().any(|(_, k, why)| k == key && why.is_none()),
                "{key}"
            );
        }
    }

    /// D13: a batch read under one exclusion list is not sent once the list changed: `send` makes
    /// no request and the phase keeps nothing of it, its count included; the next poll reads again
    /// and sends what may go.
    #[test]
    fn a_batch_read_before_an_exclusion_is_never_sent() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Open words.");
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let config = crate::config::load(home).unwrap();
        let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::load(home).unwrap();
        let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
            .unwrap()
            .unwrap();
        let db = crate::providers_db::open(home).unwrap();
        let call = reserve(&db, ROLE, "1 documents", "Open words.", 160, 1.0)
            .unwrap()
            .unwrap();
        s.raw.exclude("github.com/o/elsewhere", false).unwrap();
        let (sent, _) = send(home, &batch, &embedder, Duration::from_secs(5));
        assert!(matches!(&sent, Sent::Unsent(e) if e.is::<crate::curate::ListChanged>()));
        Phase::new(home)
            .finish(&s.raw, &k, &batch, Some(call), sent, 0)
            .unwrap();
        assert_eq!(stub.requests(), 0);
        let rows: i64 = db
            .query_row(
                "SELECT (SELECT count(*) FROM provider_calls) + (SELECT count(*) FROM provider_state)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
        embed_all(&s);
        assert_eq!(stub.requests(), 1);
    }

    /// Spec 6.4: the sending thread checks the rules the batch was composed under.
    #[test]
    fn a_paused_send_rejects_changed_or_invalid_rules() {
        for invalid in [false, true] {
            let mut s = Store::new();
            config_at(&s, "http://127.0.0.1:1/run/bge-m3");
            s.said("s", R, 1_000, "opaque_canary");
            s.run();
            let home = s.home.path();
            let k = crate::knowledge::open(home).unwrap();
            let config = crate::config::load(home).unwrap();
            let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
            let reading = Reading::now(&s.raw, Reads::Live).unwrap();
            let rules = crate::redact::Rules::load(home).unwrap();
            let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
                .unwrap()
                .unwrap();
            let gate = std::sync::Barrier::new(2);
            let (sent, ms) = std::thread::scope(|scope| {
                let sending = scope.spawn(|| {
                    gate.wait();
                    send(home, &batch, &embedder, Duration::from_millis(100))
                });
                let path = home.join("config.toml");
                let regex = if invalid { "(" } else { "^opaque_canary$" };
                let rules = format!(
                    "[redaction]\nextra_rules = [{{ id = \"canary\", regex = '{regex}' }}]\n"
                );
                std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + &rules).unwrap();
                gate.wait();
                sending.join().unwrap()
            });
            assert!(
                matches!(sent, Sent::Unsent(_)),
                "invalid={invalid}: a stale batch reached the embedder"
            );
            assert_eq!(ms, 0);
        }
    }

    /// A tombstone written while the sender waits invalidates document and query text alike.
    #[test]
    fn a_paused_send_rejects_a_new_tombstone() {
        for query in [false, true] {
            let mut s = Store::new();
            config_at(&s, "http://127.0.0.1:1/run/bge-m3");
            let seq = s.said("s", R, 1_000, "opaque_canary");
            s.run();
            let home = s.home.path().to_owned();
            let k = crate::knowledge::open(&home).unwrap();
            let config = crate::config::load(&home).unwrap();
            let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
            let reading = Reading::now(&s.raw, Reads::Live).unwrap();
            let rules = crate::redact::Rules::load(&home).unwrap();
            let mut batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
                .unwrap()
                .unwrap();
            if query {
                batch.docs.clear();
            }
            let gate = std::sync::Barrier::new(2);
            let (sent, ms) = std::thread::scope(|scope| {
                let sending = scope.spawn(|| {
                    gate.wait();
                    send(&home, &batch, &embedder, Duration::from_millis(100))
                });
                s.raw
                    .append_tombstone(crate::raw::Target::Record {
                        device: s.raw.device().to_owned(),
                        seq,
                    })
                    .unwrap();
                gate.wait();
                sending.join().unwrap()
            });
            assert!(
                matches!(sent, Sent::Unsent(_)),
                "query={query}: tombstoned text reached the embedder"
            );
            assert_eq!(ms, 0);
        }
    }

    /// A tombstone after consumer drain also keeps stale claim and raw indexes from egress.
    #[test]
    fn pending_tombstones_keep_raw_and_claim_text_out_of_batches() {
        for whole in [true, false] {
            let mut s = Store::new();
            let text = "opaque_canary parser";
            let seq = s.said("s", R, 1_000, text);
            s.claim(seq, text, ("decision", "decided", "user"), &[]);
            s.run();
            let device = s.raw.device().to_owned();
            let target = if whole {
                crate::raw::Target::Record { device, seq }
            } else {
                let e = crate::consumer::manifest::event(&s.raw, &device, seq)
                    .unwrap()
                    .unwrap();
                crate::raw::Target::Range {
                    device,
                    seq,
                    offset: e.body.find("opaque_canary").unwrap() as i64,
                    length: "opaque_canary".len() as i64,
                }
            };
            s.raw.append_tombstone(target).unwrap();
            let k = crate::knowledge::open(s.home.path()).unwrap();
            let reading = Reading::now(&s.raw, Reads::Live).unwrap();
            let rules = crate::redact::Rules::default();
            let batch =
                pending(&s.raw, &k, crate::embed::EMBEDDER, &reading, &rules, false).unwrap();
            assert!(
                batch
                    .as_ref()
                    .is_none_or(|b| b.texts.iter().all(|t| !t.contains("opaque_canary"))),
                "whole={whole}: pending tombstoned text was composed"
            );
        }
    }

    /// A pending status change holds a claim only until the consumer applies it.
    #[test]
    fn a_pending_status_correction_does_not_permanently_skip_a_claim() {
        let mut s = Store::new();
        let text = "Parser errors go to stderr.";
        let seq = s.said("s", R, 1_000, text);
        let uid = s.claim(seq, text, ("decision", "decided", "user"), &[]);
        s.run();
        let correction = serde_json::json!({
            "uid": uid, "anchor": {"device": s.raw.device(), "seq": seq}, "status": "done"
        });
        s.raw
            .append_ops(&[(crate::raw::OpKind::Correction, correction)])
            .unwrap();
        vectors(&s);
        s.run();
        vectors(&s);
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let embedded: bool = k.query_row(
            "SELECT EXISTS(SELECT 1 FROM vector_keys WHERE kind = 'c' AND key = ?1 AND skipped IS NULL)",
            [&uid], |r| r.get(0),
        ).unwrap();
        assert!(
            embedded,
            "the applied status change left the claim permanently skipped"
        );
    }

    /// D13: an exclusion undone queues its documents again: the marks made under the old list go.
    #[test]
    fn undoing_an_exclusion_queues_its_documents() {
        const X: &str = "github.com/o/secret";
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let seq = s.said("sx", X, 1_000, "Secret words there.");
        s.raw.exclude(X, false).unwrap();
        s.run();
        embed_all(&s);
        assert_eq!(stub.requests(), 0);
        assert_eq!(skipped(&s, &s.key(seq)).as_deref(), Some("excluded"));
        s.raw.exclude(X, true).unwrap();
        s.run();
        embed_all(&s);
        assert_eq!(stub.texts().concat(), ["Secret words there."]);
        assert!(
            keys(&s)
                .iter()
                .any(|(_, k, why)| *k == s.key(seq) && why.is_none())
        );
    }

    /// Step 13: a poll reads a page of `vector_todo`, never the whole store. The queue holds the
    /// imported documents and records with no key row, those a home held before its first poll
    /// included, and nothing once they are embedded; a new record, a text changed and an exclusion
    /// undone each put their document back.
    #[test]
    fn the_queue_holds_only_what_waits_for_a_vector() {
        const X: &str = "github.com/o/secret";
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.imported("o1", "r", 1_000, "Deploy notes", "Notes.");
        let seq = s.said("s", R, 2_000, "Words to mask.");
        let secret = s.said("sx", X, 3_000, "Secret words there.");
        s.raw.exclude(X, false).unwrap();
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let queued = || -> Vec<String> {
            k.prepare("SELECT family || ' ' || key FROM vector_todo ORDER BY family, key")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        embed_all(&s);
        assert!(queued().is_empty(), "{:?}", queued());
        let more = s.said("s", R, 4_000, "More words.");
        s.run();
        assert_eq!(queued(), [format!("r {}", s.key(more))]);
        embed_all(&s);
        assert!(queued().is_empty(), "{:?}", queued());
        let body = serde_json::json!({ "prompt": "Words to mask." }).to_string();
        s.raw
            .append_tombstone(crate::raw::Target::Range {
                device: s.raw.device().to_owned(),
                seq,
                offset: body.find("mask").unwrap() as i64,
                length: 4,
            })
            .unwrap();
        s.run();
        assert_eq!(queued(), [format!("r {}", s.key(seq))]);
        embed_all(&s);
        s.raw.exclude(X, true).unwrap();
        s.run();
        embed_all(&s);
        assert!(queued().is_empty(), "{:?}", queued());
        assert!(
            stub.texts()
                .concat()
                .iter()
                .any(|t| t == "Secret words there.")
        );
        assert!(
            keys(&s)
                .iter()
                .any(|(_, key, why)| *key == s.key(secret) && why.is_none())
        );
    }

    /// One poll's call, waited for, then what the next poll says.
    fn call(s: &Store, k: &Connection, phase: &mut Phase) -> Step {
        assert!(matches!(
            phase.poll(&s.raw, k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, k).unwrap(), Step::Covered);
        phase.poll(&s.raw, k).unwrap()
    }

    /// Rows 55-5 and 7.6 (Step 6): a 429, a 500 and a call past its timeout each rest the
    /// embedder, and the documents wait for a later call. A text the model will not take alone is
    /// `refused` once another part of its batch was answered, and the rest of the batch goes; with
    /// no part answered, the embedder rests instead. Each request counts in the day's.
    #[test]
    fn a_refused_or_timed_out_embedding_waits_and_loses_nothing() {
        use crate::providers_db as pdb;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let words = s.said("s", R, 1_000, "Words to embed.");
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let db = pdb::open(home).unwrap();
        let mut phase = Phase::new(home);
        phase.timeout = Duration::from_millis(300);
        let rested = |step: Step, at_least: i64| {
            let Step::Waiting { until, up } = step else {
                panic!("{step:?}")
            };
            assert!(until >= crate::db::now_ms() + at_least, "{step:?}");
            assert!(up);
            assert_eq!(skipped(&s, &s.key(words)), None);
            assert!(!keys(&s).iter().any(|(_, key, _)| *key == s.key(words)));
            pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
        };
        stub.fail_next(429, Some(2));
        rested(call(&s, &k, &mut phase), 40_000);
        stub.fail_next(500, None);
        rested(call(&s, &k, &mut phase), 500_000);
        let held = stub.hold();
        rested(call(&s, &k, &mut phase), 500_000);
        drop(held);
        while stub.answered() < stub.requests() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // A 404 sets no rest at first (the breaker counts it), and the next answer clears the count.
        stub.fail_next(404, None);
        assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
        assert_eq!(pdb::state(&db, crate::embed::CALLS).unwrap().fails, 1);
        assert_eq!(call(&s, &k, &mut phase), Step::Idle);
        assert_eq!(
            pdb::state(&db, crate::embed::CALLS).unwrap(),
            pdb::State::default()
        );
        assert!(
            keys(&s)
                .iter()
                .any(|(_, key, why)| *key == s.key(words) && why.is_none())
        );
        assert_eq!(pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0, 5);

        // Split down to the text alone, which fails before the other half is answered: held, then
        // sent once more alone after that answer, and refused; the rest embedded.
        stub.refuse("Poison words.");
        let poison = s.said("s", R, 2_000, "Poison words.");
        let fine = s.said("s", R, 3_000, "Fine words.");
        s.run();
        until_idle(&s.raw, &k, &mut phase);
        assert_eq!(stub.requests(), 9);
        assert_eq!(skipped(&s, &s.key(poison)).as_deref(), Some("refused"));
        assert!(
            keys(&s)
                .iter()
                .any(|(_, key, why)| *key == s.key(fine) && why.is_none())
        );
        assert_eq!(
            pdb::state(&db, crate::embed::CALLS).unwrap(),
            pdb::State::default()
        );

        // Alone from the start with nothing answered: not refused, as the embedder may be what
        // fails, and not rested on or in the way of the rest: held, a mark, so not sent again by
        // this worker or the next, until another document's answer; then sent once more alone and
        // refused.
        let alone = s.said("s", R, 4_000, "Poison words.");
        s.run();
        let sent = stub.requests();
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(stub.requests(), sent + 1);
        assert_eq!(skipped(&s, &s.key(alone)).as_deref(), Some("held"));
        // Counted as the breaker counts a 400 (one, no rest), so failing alone again and again
        // with nothing answered would rest it.
        assert_eq!(
            pdb::state(&db, crate::embed::CALLS).unwrap(),
            pdb::State {
                fails: 1,
                ..pdb::State::default()
            }
        );
        let mut phase = Phase::new(s.home.path());
        phase.timeout = Duration::from_millis(300);
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(stub.requests(), sent + 1);
        let later = s.said("s", R, 5_000, "Later words.");
        s.run();
        until_idle(&s.raw, &k, &mut phase);
        assert_eq!(stub.requests(), sent + 3);
        assert_eq!(stub.texts()[sent + 1], ["Later words."]);
        assert_eq!(skipped(&s, &s.key(alone)).as_deref(), Some("refused"));
        assert!(
            keys(&s)
                .iter()
                .any(|(_, key, why)| *key == s.key(later) && why.is_none())
        );
        assert_eq!(
            pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0 as usize,
            stub.requests()
        );
    }

    /// Steps 4 and 6: a text whose stored text changed while its split went on is not refused:
    /// the mark would hold the old text's hash, and the new text would never be read again.
    #[test]
    fn a_text_changed_during_its_split_is_not_refused() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        stub.refuse("Poison words.");
        let poison = s.said("s", R, 1_000, "Poison words.");
        s.said("s", R, 2_000, "Fine words.");
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        // The batch, then the poison alone: both refused, the other half still to go.
        for _ in 0..2 {
            assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
        }
        let body = serde_json::json!({ "prompt": "Poison words." }).to_string();
        s.raw
            .append_tombstone(crate::raw::Target::Range {
                device: s.raw.device().to_owned(),
                seq: poison,
                offset: body.find("words").unwrap() as i64,
                length: 5,
            })
            .unwrap();
        s.run();
        until_idle(&s.raw, &k, &mut phase);
        assert!(stub.texts().concat().iter().any(|t| t == "Poison *****."));
        assert!(
            keys(&s)
                .iter()
                .any(|(_, key, why)| *key == s.key(poison) && why.is_none())
        );
    }

    /// Step 6 (D8): a split's half that waited out a rest is read again before it goes: a text
    /// masked meanwhile never leaves as it was, and goes as it is now.
    #[test]
    fn a_split_half_is_read_again_before_it_is_sent() {
        use crate::providers_db as pdb;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        stub.refuse("Poison words.");
        let poison = s.said("s", R, 1_000, "Poison words.");
        s.said("s", R, 2_000, "Fine words, longer.");
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        let answered = |phase: &mut Phase| {
            assert!(matches!(
                phase.poll(&s.raw, &k).unwrap(),
                Step::Waiting { .. }
            ));
            while !phase.done() {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        };
        // The batch is refused and split; its other half meets a 429: the poison's half waits.
        answered(&mut phase);
        stub.fail_next(429, Some(60));
        answered(&mut phase);
        assert_eq!(stub.requests(), 2);
        let body = serde_json::json!({ "prompt": "Poison words." }).to_string();
        s.raw
            .append_tombstone(crate::raw::Target::Range {
                device: s.raw.device().to_owned(),
                seq: poison,
                offset: body.find("words").unwrap() as i64,
                length: 5,
            })
            .unwrap();
        s.run();
        let db = pdb::open(s.home.path()).unwrap();
        pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
        until_idle(&s.raw, &k, &mut phase);
        let after = stub.texts()[2..].concat();
        assert!(!after.iter().any(|t| t == "Poison words."), "{after:?}");
        assert!(after.iter().any(|t| t == "Poison *****."), "{after:?}");
    }

    /// Step 6: a split that nothing answers stops after `SPLIT_FAILS` requests: the embedder is
    /// failing, not a text, so nothing is refused; the texts that failed alone are held for an
    /// answer.
    #[test]
    fn a_split_nothing_answers_stops_at_its_bound() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        for i in 0..16 {
            s.said("s", R, 1_000 + i, &format!("Words {i:02} to embed."));
        }
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        for _ in 0..SPLIT_FAILS {
            stub.fail_next(400, None);
            assert!(phase.flight.is_none());
            assert!(matches!(
                phase.poll(&s.raw, &k).unwrap(),
                Step::Waiting { .. }
            ));
            while !phase.done() {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        }
        assert!(phase.split.is_none());
        assert_eq!(stub.requests(), SPLIT_FAILS as usize);
        let marks: Vec<_> = keys(&s).into_iter().map(|(_, _, why)| why).collect();
        assert!(!marks.is_empty());
        assert!(
            marks.iter().all(|why| why.as_deref() == Some("held")),
            "{marks:?}"
        );
    }

    /// The Global Constraints (Step 7): batches stop 40 requests short of `daily_requests`, kept
    /// for query vectors, which pass until the day's are spent; batches stop at `monthly_usd` too;
    /// embedding spend and the paid curators' spend are counted apart, so neither stops the other.
    #[test]
    fn the_embedding_cap_is_its_own_and_keeps_room_for_queries() {
        use crate::providers_db as pdb;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Words to embed.");
        s.run();
        let home = s.home.path().to_owned();
        let home = home.as_path();
        let k = crate::knowledge::open(home).unwrap();
        let db = pdb::open(home).unwrap();
        let row = |role: &'static str, usd: Option<f64>, provider: &'static str| pdb::Call {
            provider,
            role,
            span: "",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: Some(1),
            usage: pdb::Usage::default(),
            usd,
        };
        for _ in 0..159 {
            pdb::record(&db, &row("embed", None, crate::embed::CALLS)).unwrap();
        }
        // A paid curator spent past the USD 5: the embedder is not stopped.
        pdb::record(&db, &row("curator", Some(9.0), "paid")).unwrap();
        let mut phase = Phase::new(home);
        assert_eq!(call(&s, &k, &mut phase), Step::Idle);
        assert_eq!(stub.requests(), 1);
        s.said("s", R, 2_000, "More words to embed.");
        s.run();
        let Step::Waiting { until, up } = phase.poll(&s.raw, &k).unwrap() else {
            panic!("160 requests in the day, and a batch went")
        };
        assert!(until > crate::db::now_ms() + 3_600_000 && !up);
        assert_eq!(stub.requests(), 1);
        // A query passes, from the 40 kept for queries, until the day's 200 are spent.
        use crate::search::b::{Query, Vector, VectorSkip, query};
        let q = Query {
            text: "Words".into(),
            caller: Some(R.into()),
            limit: 5,
            ..Default::default()
        };
        assert_eq!(query(home, &q).unwrap().vector, Vector::Used);
        assert_eq!(stub.requests(), 2);
        for _ in 0..39 {
            pdb::record(&db, &row("query", None, crate::embed::CALLS)).unwrap();
        }
        let spent = Vector::Skipped(VectorSkip::Waiting);
        assert_eq!(query(home, &q).unwrap().vector, spent);
        assert_eq!(stub.requests(), 2);
        // The month's embedding USD spent: no batch until next month, and the curators' spend
        // leaves it out.
        db.execute("DELETE FROM provider_calls WHERE role = 'embed'", [])
            .unwrap();
        pdb::record(&db, &row("embed", Some(1.0), crate::embed::CALLS)).unwrap();
        assert_eq!(pdb::usd_this_month(&db).unwrap(), 9.0);
        assert_eq!(pdb::embed_usd_this_month(&db).unwrap(), 1.0);
        let Step::Waiting { until, up } = phase.poll(&s.raw, &k).unwrap() else {
            panic!("the month's USD spent, and a batch went")
        };
        assert_eq!((until, up), (pdb::next_month(), false));
        assert_eq!(stub.requests(), 2);
        // Short of it, a request whose cost would pass it waits for the next UTC day, when the
        // free neurons come back.
        db.execute(
            "UPDATE provider_calls SET usd = 1.0 - 1e-9 WHERE role = 'embed'",
            [],
        )
        .unwrap();
        let used = pdb::Call {
            est_tokens: Some(10_000_000),
            usd: Some(0.0),
            ..row("elsewhere", None, crate::embed::CALLS)
        };
        pdb::record(&db, &used).unwrap();
        let now = crate::db::now_ms();
        let Step::Waiting { until, .. } = phase.poll(&s.raw, &k).unwrap() else {
            panic!("a batch past the month's USD went")
        };
        assert!(until > now && until <= now + pdb::DAY_MS && until % pdb::DAY_MS == 0);
        assert_eq!(stub.requests(), 2);
        // The estimate: free inside the day's 10,000 neurons, USD 0.011 per 1,000 past them.
        let tokens = |neurons: f64| (neurons * 1e6 / NEURONS_PER_M / COUNTED_PER_ESTIMATED) as i64;
        assert_eq!(usd(0, tokens(9_000.0)), 0.0);
        let past = usd(tokens(9_000.0), tokens(2_000.0));
        assert!((past - 0.011).abs() < 1e-4, "{past}");
    }

    /// Step 7: a request is counted, with what it may cost, before it is sent: a call that never
    /// comes back, a process that dies, or a write that fails after it leaves it counted. An
    /// answer whose body cannot be used was run, so it is billed as an answer is.
    #[test]
    fn a_request_is_counted_before_it_is_sent() {
        use crate::providers_db as pdb;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Words to embed.");
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let db = pdb::open(home).unwrap();
        let rows = || -> Vec<(String, Option<f64>)> {
            db.prepare("SELECT outcome, usd FROM provider_calls WHERE role = 'embed' ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let mut phase = Phase::new(home);
        let held = stub.hold();
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while stub.requests() == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0, 1);
        assert_eq!(rows(), [("sent".to_owned(), Some(0.0))]);
        drop(held);
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        assert_eq!(rows(), [("ok".to_owned(), Some(0.0))]);
        // Past the day's free neurons, an answer of 200 that holds no vectors is billed.
        pdb::record(
            &db,
            &pdb::Call {
                provider: crate::embed::CALLS,
                role: "elsewhere",
                span: "",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: Some(10_000_000),
                usage: pdb::Usage::default(),
                usd: Some(0.0),
            },
        )
        .unwrap();
        s.said("s", R, 2_000, "More words to embed.");
        s.run();
        stub.fail_next(200, None);
        assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
        let last = rows().pop().unwrap();
        assert_eq!(last.0, "error");
        assert!(last.1.is_some_and(|usd| usd > 0.0), "{last:?}");
    }

    /// Spec 1.7: a restore sets aside a knowledge.db that is not damaged, and the next worker's
    /// first poll carries its vectors into the empty cache, those its WAL still held included (a
    /// process that died left them there): nothing is embedded again.
    #[test]
    fn a_restore_keeps_the_vectors_its_wal_held() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let home = s.home.path().to_owned();
        drop(crate::knowledge::open(&home).unwrap());
        // Open across what follows, so no close copies the WAL into the file (Windows renames no
        // open file).
        #[cfg(unix)]
        let held = {
            let held = Connection::open(home.join("knowledge.db")).unwrap();
            held.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
            held
        };
        claim(
            &mut s,
            "Parser caches stay in Redis.",
            "Parser caches go to Redis.",
        );
        s.said("s", R, 3_000, "Words.");
        s.run();
        embed_all(&s);
        let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
        let (sent, before) = (stub.requests(), indexed(&s));
        assert!(sent >= 1 && before >= 2, "{sent} {before}");
        #[cfg(unix)]
        std::mem::forget(held);
        crate::backup::quarantine(&home, "knowledge.db").unwrap();
        s.run();
        embed_all(&s);
        assert_eq!((stub.requests(), indexed(&s)), (sent, before));
    }

    /// Spec 1.7: a worker that sets knowledge.db aside and opens a new one carries into it too,
    /// before the answer of a call in flight is written there, whatever the cap: the same phase
    /// serves both files.
    #[test]
    fn a_worker_carries_into_each_knowledge_db_it_opens() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        claim(
            &mut s,
            "Parser caches stay in Redis.",
            "Parser caches go to Redis.",
        );
        s.said("s", R, 3_000, "Words.");
        s.run();
        let home = s.home.path().to_owned();
        let mut phase = Phase::new(&home);
        let k = crate::knowledge::open(&home).unwrap();
        until_idle(&s.raw, &k, &mut phase);
        let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
        let before = indexed(&s);
        let held = stub.hold();
        s.said("s", R, 4_000, "More words.");
        s.run();
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        drop(k);
        crate::backup::quarantine(&home, "knowledge.db").unwrap();
        s.run();
        spend_the_day(&home);
        let sent = stub.requests();
        drop(held);
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let k = crate::knowledge::open(&home).unwrap();
        while phase.poll(&s.raw, &k).unwrap() == Step::Covered {}
        assert_eq!((stub.requests(), indexed(&s)), (sent, before + 1));
    }

    /// Step 6: while a call waits, each kind's cached vectors are still mapped: one claim to send
    /// keeps no imported document or record from the vector its text already has.
    #[test]
    fn cached_vectors_are_mapped_while_a_call_waits() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.imported("o1", "r", 1_000, "Notes", "Project notes.");
        s.said("s", R, 2_000, "Words.");
        s.run();
        embed_all(&s);
        let sent = stub.requests();
        // A new index over the cache, as after a rebuild, and a claim with no vector yet.
        let k = crate::knowledge::open(s.home.path()).unwrap();
        k.execute_batch("DELETE FROM vector_keys; DELETE FROM vec_index;")
            .unwrap();
        claim(
            &mut s,
            "Parser caches stay in Redis.",
            "Parser caches go to Redis.",
        );
        s.run();
        spend_the_day(s.home.path());
        let mut phase = Phase::new(s.home.path());
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        assert_eq!(stub.requests(), sent);
        let mapped: Vec<String> = keys(&s)
            .into_iter()
            .filter(|(kind, _, why)| kind != "c" && why.is_none())
            .map(|(kind, ..)| kind)
            .collect();
        assert_eq!(mapped, ["k", "r"]);
    }

    /// Steps 4 and 5 (D13): an `excluded` mark goes whenever its document is written again, its
    /// text the same or not, since what excluded it can change under the same text: a uid
    /// imported again under another project.
    #[test]
    fn a_document_written_again_is_judged_again() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let uid = s.imported("o1", "secret", 1_000, "Notes", "Project notes.");
        s.raw.exclude("github.com/o/secret", false).unwrap();
        s.run();
        embed_all(&s);
        assert_eq!(skipped(&s, &uid).as_deref(), Some("excluded"));
        assert_eq!(stub.requests(), 0);
        s.imported("o1", "r", 1_000, "Notes", "Project notes.");
        s.run();
        embed_all(&s);
        assert_eq!(skipped(&s, &uid), None);
        assert_eq!(stub.requests(), 1);
    }

    /// Rows 55-1 and 55-7: a providers.db that will not open holds back the vectors only: the
    /// phase sends nothing and is idle, so the worker goes on with curation and its backups.
    #[test]
    fn a_providers_db_that_will_not_open_holds_back_only_the_vectors() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Words to embed.");
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let db = home.join("providers.db");
        if db.exists() {
            std::fs::remove_file(&db).unwrap();
        }
        std::fs::create_dir(&db).unwrap();
        let mut phase = Phase::new(home);
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(stub.requests(), 0);
    }

    /// Spec 1.7 (Steps 6 and 9): a rebuild makes no embedding call: the vectors of the
    /// knowledge.db it sets aside are carried, and the next polls map them though the day's
    /// requests are spent. A quarantined knowledge.db's vectors are carried too, and one whose
    /// vectors cannot be read stops a rebuild before anything changes.
    #[test]
    fn a_rebuild_or_a_quarantine_keeps_the_vectors() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        claim(
            &mut s,
            "Parser caches stay in Redis.",
            "Parser caches go to Redis.",
        );
        s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
        s.said("s", R, 3_000, "Words.");
        s.run();
        embed_all(&s);
        let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
        let (sent, before) = (stub.requests(), indexed(&s));
        assert!(before >= 4, "{before}");
        let home = s.home.path().to_owned();
        spend_the_day(&home);
        // Blobs that are no vector: never carried.
        let k = crate::knowledge::open(&home).unwrap();
        let ones: Vec<u8> = (0..crate::embed::DIM)
            .flat_map(|_| 1.0f32.to_le_bytes())
            .collect();
        k.execute(
            "INSERT INTO vectors(embedder, src_sha, vec) VALUES ('bge-m3', 'short', x'00'),
               ('bge-m3', 'nan', ?1), ('bge-m3', 'zero', ?2), ('bge-m3', 'long', ?3)",
            params![
                vec![0xffu8; crate::embed::DIM * 4],
                vec![0u8; crate::embed::DIM * 4],
                ones
            ],
        )
        .unwrap();
        drop(k);
        // With no store open, as `oboete rebuild` runs.
        let Store { home: dir, raw } = s;
        drop(raw);
        crate::worker::rebuild(&home).unwrap();
        let k = crate::knowledge::open(&home).unwrap();
        let bad: i64 = k
            .query_row(
                "SELECT count(*) FROM vectors WHERE src_sha IN ('short', 'nan', 'zero', 'long')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(k);
        assert_eq!(bad, 0);
        let s = Store {
            home: dir,
            raw: crate::raw::open(&home).unwrap(),
        };
        embed_all(&s);
        assert_eq!((stub.requests(), indexed(&s)), (sent, before));
        let left = std::fs::read_dir(&home).unwrap().any(|e| {
            let name = e.unwrap().file_name();
            name.to_string_lossy()
                .starts_with("knowledge.db.rebuilding-")
        });
        assert!(!left);

        // A damaged page of another table: the worker sets the file aside and carries its vectors.
        let file = home.join("knowledge.db");
        {
            let k = Connection::open(&file).unwrap();
            k.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            let page: i64 = k
                .query_row(
                    "SELECT rootpage FROM sqlite_master WHERE name = 'raw_docs'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let size: i64 = k.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
            drop(k);
            let mut bytes = std::fs::read(&file).unwrap();
            let at = ((page - 1) * size) as usize;
            bytes[at..at + size as usize].fill(0xff);
            std::fs::write(&file, bytes).unwrap();
        }
        s.run();
        let quarantined = std::fs::read_dir(&home).unwrap().any(|e| {
            let name = e.unwrap().file_name();
            name.to_string_lossy()
                .starts_with("knowledge.db.quarantined-")
        });
        assert!(quarantined);
        embed_all(&s);
        assert_eq!((stub.requests(), indexed(&s)), (sent, before));

        // Vectors that cannot be read: the rebuild stops, and the file is back as it was.
        let Store { home: dir, raw } = s;
        drop(raw);
        for ext in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(home.join(format!("knowledge.db{ext}")));
        }
        std::fs::write(&file, b"not a database").unwrap();
        let err = crate::worker::rebuild(&home).unwrap_err();
        assert!(
            format!("{err:#}").contains("nothing was changed"),
            "{err:#}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"not a database");
        let aside = std::fs::read_dir(&home).unwrap().any(|e| {
            let name = e.unwrap().file_name();
            name.to_string_lossy()
                .starts_with("knowledge.db.rebuilding-")
        });
        assert!(!aside);
        drop(dir);
    }

    /// Spec 1.7: a rebuild stopped after it set knowledge.db aside, and stopped again when rerun,
    /// loses no vectors: the next polls carry those of every file it left behind, though the
    /// day's requests are spent.
    #[test]
    fn an_interrupted_rebuild_loses_no_vectors() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
        s.said("s", R, 3_000, "Words.");
        s.run();
        embed_all(&s);
        let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
        let (sent, before) = (stub.requests(), indexed(&s));
        assert!(before >= 2, "{before}");
        let home = s.home.path().to_owned();
        spend_the_day(&home);
        let Store { home: dir, raw } = s;
        drop(raw);
        // Each stopped once the new knowledge.db was made, before the vectors were carried.
        let at = crate::db::now_ms();
        for stamp in [at, at + 1] {
            for ext in ["-wal", "-shm", ""] {
                let from = home.join(format!("knowledge.db{ext}"));
                if from.exists() {
                    let to = home.join(format!("knowledge.db.rebuilding-{stamp}{ext}"));
                    std::fs::rename(&from, to).unwrap();
                }
            }
            drop(crate::knowledge::open(&home).unwrap());
        }
        crate::worker::rebuild(&home).unwrap();
        let s = Store {
            home: dir,
            raw: crate::raw::open(&home).unwrap(),
        };
        embed_all(&s);
        assert_eq!((stub.requests(), indexed(&s)), (sent, before));
    }

    /// Step 11: doctor names the embedder and how far it is: what waits per kind, what was passed
    /// over and why, the requests and USD against the caps, and the last error.
    #[test]
    fn doctor_names_the_embedder_and_how_far_it_is() {
        let stub = Stub::start();
        let mut s = Store::new();
        let home = s.home.path().to_owned();
        s.said("s", R, 1_000, "Words to embed.");
        s.said("s", R, 1_500, "<private>all of it</private>");
        s.run();
        let k = crate::knowledge::open(&home).unwrap();
        assert_eq!(doctor_lines(&home, &k).unwrap(), ["embeddings: off"]);
        config(&s, &stub);
        let lines = doctor_lines(&home, &k).unwrap();
        assert!(lines[0].contains("(nothing embedded yet)"), "{lines:?}");
        assert!(
            lines[0].ends_with("0 claims, 0 cards, 0 summaries, 0 imported, 2 records"),
            "{lines:?}"
        );
        stub.fail_next(500, None);
        let mut phase = Phase::new(&home);
        assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
        let db = crate::providers_db::open(&home).unwrap();
        let none = crate::providers_db::State::default();
        crate::providers_db::set_state(&db, crate::embed::CALLS, none).unwrap();
        embed_all(&s);
        let lines = doctor_lines(&home, &k).unwrap();
        let want = [
            "embeddings: bge-m3 (active), waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 0 records",
            "  passed over: 1 empty",
            "  requests in the last day: 2 of 200 (40 kept for queries); USD this month: 0.00 of 1.00",
        ];
        assert_eq!(lines[..3], want);
        assert!(lines[3].starts_with("  last error (embed, "), "{lines:?}");
        // Task 10: the same vectors on this machine; Workers AI's requests and rests are not its.
        crate::settings::set_embedding_provider(&home, "local").unwrap();
        let lines = doctor_lines(&home, &k).unwrap();
        let want = [
            "embeddings: bge-m3 on this machine (active), waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 0 records",
            "  passed over: 1 empty",
        ];
        assert_eq!(lines, want);
    }

    #[test]
    fn doctor_keeps_only_the_vetted_reason_when_a_curator_shares_the_embedder_name() {
        let stub = Stub::start();
        let s = Store::new();
        s.run();
        config(&s, &stub);
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let db = crate::providers_db::open(home).unwrap();
        db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,bytes_out) VALUES(?1,?2,'curator','error',0,'transport',10)", params![crate::db::now_ms(), crate::embed::CALLS]).unwrap();
        crate::providers_db::freeze_unmetered(&db, db.last_insert_rowid(), [123.0, 456.0]).unwrap();
        let shown = doctor_lines(home, &k).unwrap().join("\n");
        assert!(shown.contains("last error (curator,"), "{shown}");
        assert!(shown.ends_with(": transport"), "{shown}");
        assert!(
            !shown.contains("oboete-budget-v1") && !shown.contains("oboete-call-v1"),
            "{shown}"
        );
        assert_eq!(stub.requests(), 0);
    }

    /// Rows 55-1 and 55-7: a call the embedder holds delays only the vectors: the worker indexes
    /// what is recorded meanwhile and full-text search finds it, and it does not spin while the
    /// call is out.
    #[test]
    fn a_held_embedding_call_delays_only_the_vectors() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let first = s.said("s", R, 1_000, "First words.");
        let held = stub.hold();
        let home = s.home.path().to_owned();
        let mut phase = Phase::new(&home);
        let second = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let phases = crate::worker::Phases {
                    embed: Some(&mut phase),
                    ..crate::worker::Phases::default()
                };
                let consumers = crate::worker::consumers(&home);
                crate::worker::run_holding(&home, 200, consumers, || {}, None, phases)
            });
            while stub.requests() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            let second = s.said("s", R, 2_000, "Second words while it waits.");
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let q = crate::search::b::Query {
                    text: "Second words".into(),
                    caller: Some(R.into()),
                    limit: 5,
                    ..Default::default()
                };
                let found = crate::search::b::query(&home, &q).unwrap();
                if found.hits.iter().any(|h| h.key == s.key(second)) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "not indexed while the call was out"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(Duration::from_millis(1_000));
            drop(held);
            worker.join().unwrap().unwrap();
            second
        });
        assert!(phase.polls < 30, "{} polls", phase.polls);
        for seq in [first, second] {
            assert!(
                keys(&s)
                    .iter()
                    .any(|(_, k, why)| *k == s.key(seq) && why.is_none())
            );
        }
    }

    /// D6 and A92 (Step 5): records of imported sources get no vector, and however many there
    /// are they never hold up a live record behind them: one poll marks them `source`, page after
    /// page, and sends the live one. A text the gate leaves empty is marked `empty`.
    #[test]
    fn records_of_imported_sources_and_empty_texts_are_passed_over() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Live words.");
        let empty = s.said("s", R, 1_500, "<private>all of it</private>");
        let many = 3 * PAGE + 50;
        for i in 0..many as i64 {
            let body = serde_json::json!({ "prompt": format!("Old words {i}.") }).to_string();
            let e = crate::raw::Event {
                source: "transcript".into(),
                session: "t".into(),
                repo: Some(R.into()),
                ts: 2_000 + i,
                ..crate::raw::test_event(&body)
            };
            s.raw.append(&e).unwrap();
        }
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        assert_eq!(stub.texts(), [vec!["Live words.".to_owned()]]);
        assert_eq!(skipped(&s, &s.key(empty)).as_deref(), Some("empty"));
        let source = keys(&s)
            .iter()
            .filter(|(.., why)| why.as_deref() == Some("source"))
            .count();
        assert_eq!(source, many);
    }

    /// Tool output gets no vector (#317, decision 42). It is not queued, before the queue is made
    /// or after; one a home made before had queued leaves the queue unsent; and it does not count
    /// as waiting for one. The record beside it is embedded.
    #[test]
    fn tool_output_gets_no_vector() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        s.said("s", R, 1_000, "Live words.");
        let body =
            serde_json::json!({"tool": "Bash", "input": "cargo test", "output": "Tool words."});
        let before = s.event("tool", "s", (R, "main"), 2_000, body.clone());
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        let after = s.event("tool", "s", (R, "main"), 3_000, body);
        s.run();
        let queued = |key: &str| -> i64 {
            k.query_row(
                "SELECT count(*) FROM vector_todo WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!((queued(&s.key(before)), queued(&s.key(after))), (0, 0));
        k.execute(
            "INSERT INTO vector_todo(family, key, ord) VALUES ('r', ?1, ?2)",
            params![s.key(before), before],
        )
        .unwrap();
        // Nothing to send: the queued tool output leaves the queue.
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(queued(&s.key(before)), 0);
        assert_eq!(stub.texts(), [vec!["Live words.".to_owned()]]);
        let tools = [s.key(before), s.key(after)];
        assert!(keys(&s).iter().all(|(_, k, _)| !tools.contains(k)));
        let facts = doctor_facts_in(&k, crate::embed::EMBEDDER);
        assert_eq!(facts.records.unwrap(), 0);
    }

    /// A home whose phase ran before tool output stayed out (#317): its trigger queued tool records
    /// and its index held their vectors. The next poll takes both out and makes the trigger anew,
    /// and the prompt beside them keeps its vector (Codex on #421).
    #[test]
    fn a_home_from_before_loses_its_queued_tool_output_and_its_vectors() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let prompt = s.said("s", R, 1_000, "Live words.");
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        phase.poll(&s.raw, &k).unwrap();
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        k.execute_batch(
            "DROP TRIGGER vector_todo_record;
             CREATE TRIGGER vector_todo_record AFTER INSERT ON raw_docs
               WHEN NOT EXISTS (SELECT 1 FROM vector_keys
                                WHERE kind = 'r' AND key = NEW.device || ':' || NEW.seq)
             BEGIN
               INSERT OR IGNORE INTO vector_todo(family, key, ord)
                 VALUES ('r', NEW.device || ':' || NEW.seq, NEW.seq);
             END;",
        )
        .unwrap();
        let body =
            serde_json::json!({"tool": "Bash", "input": "cargo test", "output": "Tool words."});
        let tools = [
            s.event("tool", "s", (R, "main"), 2_000, body.clone()),
            s.event("tool", "s", (R, "main"), 3_000, body),
        ];
        s.run();
        let tools = tools.map(|seq| s.key(seq));
        let held = Doc {
            kind: "r",
            key: tools[1].clone(),
            sha: "from before".into(),
            repo: String::new(),
            ts: 3_000,
            session: "s".into(),
        };
        index(
            &k,
            crate::embed::EMBEDDER,
            &held,
            &[0u8; crate::embed::DIM * 4],
        )
        .unwrap();
        let count =
            |sql: &str, key: &str| -> i64 { k.query_row(sql, [key], |r| r.get(0)).unwrap() };
        let queued = |key: &str| count("SELECT count(*) FROM vector_todo WHERE key = ?1", key);
        let keyed = |key: &str| count("SELECT count(*) FROM vector_keys WHERE key = ?1", key);
        let indexed = |key: &str| {
            count(
                "SELECT count(*) FROM vec_index WHERE rowid IN
                   (SELECT id FROM vector_keys WHERE key = ?1)",
                key,
            )
        };
        assert_eq!(queued(&tools[0]), 1, "the old trigger queues tool output");
        assert_eq!((keyed(&tools[1]), indexed(&tools[1])), (1, 1));
        phase.poll(&s.raw, &k).unwrap();
        for key in &tools {
            assert_eq!((queued(key), keyed(key)), (0, 0), "{key}");
        }
        let left: i64 = k
            .query_row("SELECT count(*) FROM vec_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "only the prompt's vector");
        assert_eq!(keyed(&s.key(prompt)), 1);
        let later = s.event(
            "tool",
            "s",
            (R, "main"),
            4_000,
            serde_json::json!({"tool": "Bash", "input": "ls", "output": "More."}),
        );
        s.run();
        assert_eq!(queued(&s.key(later)), 0, "the trigger is made anew");
    }

    /// Rows 30-1 and 30-14: what reaches the embedder has passed the gate: a token and a
    /// `<private>` block in a claim, an import op and a record, and a secret across a prompt's
    /// cut is hidden whole, not cut in two.
    #[test]
    fn texts_sent_to_the_embedder_pass_the_gate() {
        let token = ["gh", "p_q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"].concat();
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        // A curator's body can hold what its quote does not: the rescan masks a secret in raw,
        // so a quote of one would no longer read there.
        let uid = claim(
            &mut s,
            "Deploy with the token always.",
            &format!("Deploy with {token} <private>for acme</private> always."),
        );
        // An import is not rescanned, so its token reaches the phase, here across the cut: the
        // composed text is "decision: Deploy notes\n" and the body, and the cut keeps 12,000.
        let pad = crate::embed::MAX_CHARS - "decision: Deploy notes\n".len() - 15;
        let body = format!(
            "<private>acme</private>{} {token} tail",
            "x".repeat(pad - 23)
        );
        let imported = s.imported("o1", "r", 2_000, "Deploy notes", &body);
        let seq = s.said("s", R, 3_000, "<private>acme corp</private> Deploy words.");
        s.run();
        embed_all(&s);
        let sent: Vec<String> = stub.texts().concat();
        assert!(!sent.is_empty());
        for text in &sent {
            assert!(!text.contains(&token[..12]), "{text}");
            assert!(!text.contains("acme"), "{text}");
        }
        let got = keys(&s);
        for key in [uid, imported, s.key(seq)] {
            assert!(
                got.iter()
                    .any(|(_, k, skipped)| *k == key && skipped.is_none()),
                "{key}: {got:?}"
            );
        }
    }

    /// The ranges `composed_out` gates alone are the title and the body in the composed text.
    #[test]
    fn composed_parts_name_the_title_and_the_body() {
        let (text, parts) = composed_parts("decision", "Tabs", "Use tabs.\nAlways.");
        assert_eq!(text, composed("decision", "Tabs", "Use tabs.\nAlways."));
        assert_eq!(&text[parts[0].clone()], "Tabs");
        assert_eq!(&text[parts[1].clone()], "Use tabs.\nAlways.");
        for kind in ["prompt", "summary"] {
            assert_eq!(composed_parts(kind, "T", "B"), ("B".to_owned(), Vec::new()));
        }
    }

    /// Codex on 52803e3: a rule anchored to an imported title (`^...$`), added after the import,
    /// holds in the text sent as in search's hits: the title is gated alone before its kind is
    /// prefixed. The rules are the process's, so the phase runs in a child.
    #[test]
    fn a_rule_anchored_to_an_imported_title_holds_in_the_text_sent() {
        const HOME: &str = "OBOETE_TEST_TITLE_RULE_HOME";
        const SECRET: &str = "INTERNAL-GAMMA-3";
        const OTHER: &str = "INTERNAL-DELTA-5";
        if let Ok(home) = std::env::var(HOME) {
            let home = std::path::PathBuf::from(home);
            crate::redact::set_home(&home).unwrap();
            let raw = crate::raw::open(&home).unwrap();
            let k = crate::knowledge::open(&home).unwrap();
            until_idle(&raw, &k, &mut Phase::new(&home));
            return;
        }
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let uid = s.imported("o1", "r", 2_000, SECRET, "deploy notes");
        // Codex on 6c19081: a rule on the whole composed text keeps its context though the
        // title's own rule masks the title.
        let other = s.imported("o2", "r", 2_000, OTHER, "private deployment value");
        s.run();
        let config = s.home.path().join("config.toml");
        let plain = std::fs::read_to_string(&config).unwrap();
        let ruled = format!(
            "{plain}[redaction]\nextra_rules = [{{ id = \"gamma\", regex = '^{SECRET}$' }}, \
             {{ id = \"delta\", regex = '^{OTHER}$' }}, \
             {{ id = \"whole\", regex = '(?s)^decision: {OTHER}\\n.*$' }}]\n"
        );
        std::fs::write(&config, ruled).unwrap();
        let name =
            "embed_phase::tests::a_rule_anchored_to_an_imported_title_holds_in_the_text_sent";
        let out = std::process::Command::new(std::env::args_os().next().unwrap())
            .args(["--exact", name])
            .env(HOME, s.home.path())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{said}");
        assert!(said.contains("1 passed"), "{said}");
        let sent: Vec<String> = stub.texts().concat();
        assert!(sent.iter().any(|t| t.contains("deploy notes")), "{sent:?}");
        assert!(sent.iter().all(|t| !t.contains(SECRET)), "{sent:?}");
        assert!(sent.iter().all(|t| !t.contains("private")), "{sent:?}");
        let got = keys(&s);
        assert!(
            got.iter()
                .any(|(_, k, skipped)| *k == uid && skipped.is_none())
        );
        // Gated to the mask whole: nothing to send.
        assert!(
            got.iter()
                .any(|(_, k, skipped)| *k == other && skipped.as_deref() == Some("empty")),
            "{got:?}"
        );
    }

    /// Row 55-6: a correction or a tombstone that lands while a call is out keeps the vector made
    /// from the old text off its document; the next batch embeds the new text.
    #[test]
    fn a_vector_is_never_kept_for_text_that_changed_while_it_was_embedded() {
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let uid = s.decided(R, 1_000, "Parser caches stay in Redis.", &[]);
        let text = "Deploy words to mask.";
        let seq = s.said("s", R, 2_000, text);
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let mut phase = Phase::new(s.home.path());
        let mapped = |k: &Connection, key: &str| -> Option<String> {
            k.query_row(
                "SELECT src_sha FROM vector_keys WHERE key = ?1 AND skipped IS NULL",
                [key],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
        };
        let body = serde_json::json!({ "prompt": text }).to_string();
        let changes: [&dyn Fn(&mut Store); 2] = [
            &|s| {
                crate::claims::correct(s.home.path(), &uid, None, Some("Caches go to files."))
                    .unwrap()
            },
            &|s| {
                let target = crate::raw::Target::Range {
                    device: s.raw.device().to_owned(),
                    seq,
                    offset: body.find("mask").unwrap() as i64,
                    length: 4,
                };
                s.raw.append_tombstone(target).unwrap();
            },
        ];
        for (call, (change, key)) in changes.iter().zip([uid.clone(), s.key(seq)]).enumerate() {
            let held = stub.hold();
            assert!(matches!(
                phase.poll(&s.raw, &k).unwrap(),
                Step::Waiting { .. }
            ));
            while stub.requests() == call {
                std::thread::sleep(Duration::from_millis(5));
            }
            change(&mut s);
            s.run();
            drop(held);
            while !phase.done() {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
            assert_eq!(mapped(&k, &key), None, "{key}");
        }
        embed_all(&s);
        let texts = stub.texts().concat();
        assert!(
            texts.iter().any(|t| t == "Caches go to files."),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "Deploy words to ****."),
            "{texts:?}"
        );
        assert_eq!(mapped(&k, &uid), Some(sha("Caches go to files.")));
        assert!(mapped(&k, &s.key(seq)).is_some());
    }

    /// Row 46-2: every way a document's stored text changes takes its vector off, and the next
    /// polls give it the vector of the text it has now (embedded, or mapped when that text was
    /// embedded before): a re-derivation, a correction, a tombstone's mask and its removal, a
    /// second import of a uid, and the three rewinds. A status-only correction sends nothing, and
    /// the same text imported again only moves its index row's time.
    #[test]
    fn every_way_a_documents_text_changes_replaces_its_vector() {
        use crate::consumer::{claims::Claims, fts::Fts, imported::Imported};
        use crate::worker::Consumer;
        let stub = Stub::start();
        let mut s = Store::new();
        config(&s, &stub);
        let quote = "Parser caches stay in Redis.";
        let seq = s.said("s", R, 1_000, quote);
        let uid = derive(&mut s, seq, quote, quote);
        let masked = s.said("s", R, 2_000, "Deploy words to mask.");
        let removed = s.said("s", R, 3_000, "Words to forget.");
        let doc = s.imported("o1", "r", 4_000, "Deploy notes", "First notes.");
        let (home, device) = (s.home.path().to_owned(), s.raw.device().to_owned());
        let k = crate::knowledge::open(&home).unwrap();
        // The key's vector, if any, is the one of its stored text now.
        let follows = |s: &Store, kind: &str, key: &str| {
            s.run();
            embed_all(s);
            let now = current(Some(&s.raw), &k, kind, key)
                .unwrap()
                .map(|r| r.doc.sha);
            let got: Option<String> = k
                .query_row(
                    "SELECT src_sha FROM vector_keys WHERE key = ?1 AND skipped IS NULL",
                    [key],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert_eq!(got, now, "{key}");
        };
        follows(&s, "c", &uid);
        let first =
            |sql: &str, key: &str| -> i64 { k.query_row(sql, [key], |r| r.get(0)).unwrap() };
        let claim_op = first("SELECT MIN(op_seq) FROM derivations WHERE uid = ?1", &uid);
        let import_op = first("SELECT MIN(op_seq) FROM imported WHERE uid = ?1", &doc);

        derive(&mut s, seq, quote, "Parser caches go to disk.");
        follows(&s, "c", &uid);
        assert!(
            current(None, &k, "c", &uid).unwrap().unwrap().doc.sha
                == sha("Parser caches go to disk.")
        );
        crate::claims::correct(&home, &uid, None, Some("Caches go to files.")).unwrap();
        follows(&s, "c", &uid);
        let sent = stub.requests();
        crate::claims::correct(&home, &uid, Some("done"), None).unwrap();
        follows(&s, "c", &uid);
        assert_eq!(stub.requests(), sent);

        let body = serde_json::json!({ "prompt": "Deploy words to mask." }).to_string();
        s.raw
            .append_tombstone(crate::raw::Target::Range {
                device: device.clone(),
                seq: masked,
                offset: body.find("mask").unwrap() as i64,
                length: 4,
            })
            .unwrap();
        follows(&s, "r", &s.key(masked));
        assert!(
            stub.texts()
                .concat()
                .iter()
                .any(|t| t == "Deploy words to ****.")
        );
        s.raw
            .append_tombstone(crate::raw::Target::Record {
                device: device.clone(),
                seq: removed,
            })
            .unwrap();
        follows(&s, "r", &s.key(removed));

        s.imported("o1", "r", 4_000, "Deploy notes", "Second notes.");
        follows(&s, "k", &doc);
        let sent = stub.requests();
        s.imported("o1", "r", 5_000, "Deploy notes", "Second notes.");
        follows(&s, "k", &doc);
        assert_eq!(stub.requests(), sent);
        let id = first("SELECT id FROM vector_keys WHERE key = ?1", &doc);
        let ts: i64 = k
            .query_row("SELECT ts FROM vec_index WHERE rowid = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(ts, 5_000);

        // A restore that lost the later ops and records: each consumer's rewind. The texts the
        // claim and the import had before were embedded then, so they are mapped with no call.
        Imported.rewind(&k, &device, import_op).unwrap();
        follows(&s, "k", &doc);
        Claims.rewind(&k, &device, claim_op).unwrap();
        follows(&s, "c", &uid);
        assert_eq!(stub.requests(), sent);
        Fts.rewind(&k, &device, masked - 1).unwrap();
        follows(&s, "r", &s.key(masked));
        // The record the claim quotes goes, and the claim with it.
        s.raw
            .append_tombstone(crate::raw::Target::Record { device, seq })
            .unwrap();
        follows(&s, "c", &uid);
        assert!(current(None, &k, "c", &uid).unwrap().is_none());
        let (index, kept): (i64, i64) = k
            .query_row(
                "SELECT (SELECT count(*) FROM vec_index),
                        (SELECT count(*) FROM vector_keys WHERE skipped IS NULL)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(index, kept);
    }

    /// Task 10: a home whose embedder is the local model, which loads as `how` says
    /// (`embed::stub::local`).
    fn local(s: &Store, how: &str) {
        let config = "[embedding]\nprovider = \"local\"\n";
        std::fs::write(s.home.path().join("config.toml"), config).unwrap();
        crate::embed::stub::local(s.home.path(), how);
    }

    fn indexed_count(k: &Connection) -> i64 {
        k.query_row("SELECT count(*) FROM vec_index", [], |r| r.get(0))
            .unwrap()
    }

    /// D14: the worker loads the local model once 20 documents wait, of every kind together (a
    /// fresh claim's batch comes first), or one has waited 10 minutes; embeds them, and drops it
    /// when none wait. A smaller, newer backlog is no work that keeps the worker up.
    #[test]
    fn the_worker_loads_past_the_threshold_and_drops_when_none_pend() {
        let mut s = Store::new();
        let now = crate::db::now_ms();
        for i in 0..5 {
            s.said("s", R, now, &format!("Fresh words {i}."));
        }
        s.run();
        local(&s, "0");
        let home = s.home.path().to_owned();
        let k = crate::knowledge::open(&home).unwrap();
        let mut phase = Phase::new(&home);
        let step = phase.poll(&s.raw, &k).unwrap();
        let waits =
            matches!(step, Step::Waiting { until, up: false } if until >= now + LOAD_AFTER_MS);
        assert!(waits, "{step:?}");
        let loads = || crate::embed::stub::local_loads(&home);
        assert_eq!((loads(), indexed_count(&k)), (0, 0));
        // The claim and the record it quotes, and 13 more records: 20 in all.
        s.decided(R, now, "A fresh claim goes first.", &[]);
        for i in 0..13 {
            s.said("s", R, now, &format!("More words {i}."));
        }
        s.run();
        until_idle(&s.raw, &k, &mut phase);
        assert_eq!((loads(), indexed_count(&k)), (1, 20));
        assert!(
            phase.local.is_none(),
            "the model stayed with nothing to embed"
        );
        s.said("s", R, now - LOAD_AFTER_MS, "Words that waited.");
        s.run();
        until_idle(&s.raw, &k, &mut phase);
        assert_eq!((loads(), indexed_count(&k)), (2, 21));
    }

    /// Task 10: without the model (none placed, a load that fails, a vector the index will not
    /// take) nothing is indexed or marked and the documents wait. A failure rests the model in
    /// providers.db, as a failed call rests Workers AI, so a fresh worker waits too; doctor says
    /// so. The model back, what waited is embedded.
    #[test]
    fn a_missing_or_changed_model_leaves_the_phase_waiting() {
        use crate::providers_db as pdb;
        let mut s = Store::new();
        s.said("s", R, 1_000, "Words to embed.");
        s.run();
        local(&s, "0");
        let home = s.home.path().to_owned();
        std::fs::remove_file(crate::embed::local_dir(&home).join("stub")).unwrap();
        let k = crate::knowledge::open(&home).unwrap();
        assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
        let waiting = doctor_lines(&home, &k).unwrap();
        assert!(
            waiting[0].ends_with("waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 1 records")
        );
        for how in ["fail", "nan"] {
            crate::embed::stub::local(&home, how);
            let mut phase = Phase::new(&home);
            assert!(matches!(
                phase.poll(&s.raw, &k).unwrap(),
                Step::Waiting { .. }
            ));
            while !phase.done() {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
            let fresh = Phase::new(&home).poll(&s.raw, &k).unwrap();
            let rests = matches!(fresh, Step::Waiting { until, .. } if until > crate::db::now_ms());
            assert!(rests, "{how}: {fresh:?}");
            assert_eq!(indexed_count(&k), 0, "{how}");
            let lines = doctor_lines(&home, &k).unwrap();
            assert!(
                lines[1].starts_with("  local model resting until"),
                "{lines:?}"
            );
            let db = pdb::open(&home).unwrap();
            pdb::set_state(&db, LOCAL, pdb::State::default()).unwrap();
        }
        crate::embed::stub::local(&home, "0");
        embed_all(&s);
        assert_eq!(indexed_count(&k), 1);
    }

    /// Codex on #426: while the local model rests after a failure, cached vectors are still
    /// mapped and no model is loaded, as while a Workers AI call waits.
    #[test]
    fn cached_vectors_are_mapped_while_the_local_model_rests() {
        use crate::providers_db as pdb;
        let mut s = Store::new();
        s.imported("o1", "r", 1_000, "Notes", "Project notes.");
        s.said("s", R, 2_000, "Words.");
        s.run();
        local(&s, "0");
        embed_all(&s);
        let home = s.home.path().to_owned();
        let loads = crate::embed::stub::local_loads(&home);
        // A new index over the cache, as after a rebuild, and a claim with no vector yet, while
        // the model rests.
        let k = crate::knowledge::open(&home).unwrap();
        k.execute_batch("DELETE FROM vector_keys; DELETE FROM vec_index;")
            .unwrap();
        claim(
            &mut s,
            "Parser caches stay in Redis.",
            "Parser caches go to Redis.",
        );
        s.run();
        let rest = pdb::State {
            down_until: crate::db::now_ms() + 60_000,
            ..pdb::State::default()
        };
        pdb::set_state(&pdb::open(&home).unwrap(), LOCAL, rest).unwrap();
        let step = Phase::new(&home).poll(&s.raw, &k).unwrap();
        assert!(matches!(step, Step::Waiting { .. }), "{step:?}");
        assert_eq!(crate::embed::stub::local_loads(&home), loads);
        let mapped: Vec<String> = keys(&s)
            .into_iter()
            .filter(|(kind, _, why)| kind != "c" && why.is_none())
            .map(|(kind, ..)| kind)
            .collect();
        assert_eq!(mapped, ["k", "r"]);
        assert_eq!(indexed_count(&k), 2);
    }

    /// Task 10: the local model calls no Workers AI and reads no key, Workers AI's settings left
    /// in place; with its key, Workers AI gets the next document (the control).
    #[test]
    fn a_local_run_calls_no_workers_ai_and_reads_no_key() {
        let stub = Stub::start();
        let mut s = Store::new();
        s.said("s", R, 1_000, "Words to embed.");
        s.run();
        let home = s.home.path().to_owned();
        let text = format!(
            "[embedding]\nprovider = \"local\"\naccount_id = \"a\"\nkey_file = '{}'\nurl = \"{}\"\n",
            home.join("no-key.md").display(),
            stub.url
        );
        std::fs::write(home.join("config.toml"), text).unwrap();
        crate::embed::stub::local(&home, "0");
        embed_all(&s);
        let k = crate::knowledge::open(&home).unwrap();
        assert_eq!((stub.requests(), indexed_count(&k)), (0, 1));
        config(&s, &stub);
        s.said("s", R, 2_000, "More words.");
        s.run();
        embed_all(&s);
        assert_eq!((stub.requests(), indexed_count(&k)), (1, 2));
    }

    /// Row 46-2: both runners make the same model's vectors, so switching between them queues
    /// nothing and loads nothing; each embeds only what is new.
    #[test]
    fn switching_runners_queues_nothing() {
        let stub = Stub::start();
        let mut s = Store::new();
        s.said("s", R, 1_000, "Parser words.");
        s.said("s", R, 2_000, "Deploy words.");
        s.run();
        config(&s, &stub);
        embed_all(&s);
        let home = s.home.path().to_owned();
        let k = crate::knowledge::open(&home).unwrap();
        local(&s, "0");
        assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(crate::embed::stub::local_loads(&home), 0);
        s.said("s", R, 3_000, "Parser words again.");
        s.run();
        embed_all(&s);
        assert_eq!(
            (crate::embed::stub::local_loads(&home), indexed_count(&k)),
            (1, 3)
        );
        config(&s, &stub);
        assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
        assert_eq!(stub.requests(), 1);
    }

    /// Row 55-6 on the local model: a text corrected while the model embeds it keeps no vector,
    /// and the next batch embeds the new text.
    #[test]
    fn the_local_model_keeps_no_vector_of_text_that_changed() {
        let mut s = Store::new();
        let uid = s.decided(R, 1_000, "Parser caches stay in Redis.", &[]);
        s.run();
        local(&s, "300");
        let home = s.home.path().to_owned();
        let k = crate::knowledge::open(&home).unwrap();
        let mut phase = Phase::new(&home);
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        crate::claims::correct(&home, &uid, None, Some("Caches go to files.")).unwrap();
        s.run();
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        let kept = |k: &Connection| -> i64 {
            k.query_row(
                "SELECT count(*) FROM vector_keys WHERE kind = 'c' AND skipped IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(kept(&k), 0);
        until_idle(&s.raw, &k, &mut phase);
        assert_eq!(kept(&k), 1);
    }

    /// Row 30-1 on the local model: a session that touched an excluded repository reaches no
    /// embedder, the local one included (D13); the rest is embedded.
    #[test]
    fn an_excluded_repositorys_records_reach_no_local_model() {
        let mut s = Store::new();
        let there = s.said("sx", "github.com/o/secret", 1_000, "Secret words.");
        let open = s.said("so", R, 2_000, "Open words.");
        s.raw.exclude("github.com/o/secret", false).unwrap();
        s.run();
        local(&s, "0");
        embed_all(&s);
        assert_eq!(skipped(&s, &s.key(there)).as_deref(), Some("excluded"));
        assert_eq!(skipped(&s, &s.key(open)), None);
        let k = crate::knowledge::open(s.home.path()).unwrap();
        assert_eq!(indexed_count(&k), 1);
    }
}
