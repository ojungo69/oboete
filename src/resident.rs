//! Milestone 4 D14: a local model held by one thread of the process that uses it (the worker,
//! MCP, the viewer, the CLI's `--vectors`), never served to another process. The thread loads
//! it, embeds one text at a time, and drops it when its owner drops the `Resident`, or after
//! `idle` without a request.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Result;

/// A loaded model: one text's vector.
pub type Model = Box<dyn FnMut(&str) -> Result<Vec<f32>> + Send>;
/// What loads it, run on the model's thread.
pub type Load = Box<dyn FnOnce() -> Result<Model> + Send>;

/// Why `embed` gave no vectors.
#[derive(Debug, Clone, PartialEq)]
pub enum Busy {
    /// The model is still loading, and the caller would not wait for it.
    Loading,
    /// The answer did not come within the wait.
    Timeout,
    /// The model did not load, a text failed, or the model was dropped.
    Failed(String),
}

enum State {
    Loading,
    Ready,
    Failed(String),
    Gone,
}

type Answer = std::result::Result<Vec<Vec<f32>>, String>;

struct Job {
    texts: Vec<String>,
    /// Past it, its caller has gone back to full text: the job is dropped unrun, so searches
    /// that came faster than the model never queue work nobody waits for.
    until: Option<Instant>,
    reply: mpsc::Sender<Answer>,
}

pub struct Resident {
    jobs: mpsc::Sender<Job>,
    state: Arc<Mutex<State>>,
}

impl Resident {
    /// The model's thread, which loads it at once.
    pub fn start(load: Load, idle: Option<Duration>) -> Resident {
        let (jobs, queue) = mpsc::channel::<Job>();
        let state = Arc::new(Mutex::new(State::Loading));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            // However the thread ends, a panic included, its owner reads that it is gone.
            struct Exit(Arc<Mutex<State>>);
            impl Drop for Exit {
                fn drop(&mut self) {
                    let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                    if !matches!(*state, State::Failed(_)) {
                        *state = State::Gone;
                    }
                }
            }
            let _exit = Exit(Arc::clone(&shared));
            let set = |s| *shared.lock().unwrap_or_else(PoisonError::into_inner) = s;
            let mut model = match load() {
                Ok(model) => model,
                // The jobs queued meanwhile are dropped with the queue: their callers read why.
                Err(e) => return set(State::Failed(format!("{e:#}"))),
            };
            set(State::Ready);
            loop {
                let job = match idle {
                    Some(idle) => queue.recv_timeout(idle),
                    None => queue.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                let Ok(job) = job else { break };
                if job.until.is_some_and(|until| Instant::now() >= until) {
                    continue;
                }
                let answer = (job.texts.iter())
                    .map(|text| model(text).and_then(crate::embed::unit))
                    .collect::<Result<Vec<_>>>()
                    .map_err(|e| format!("{e:#}"));
                let _ = job.reply.send(answer);
            }
        });
        Resident { jobs, state }
    }

    /// Counts as a request: the model stays its `idle` longer, loading on if it is still loading
    /// (parity row 31: kept warm while the hooks record).
    pub fn keep(&self) {
        let (reply, _) = mpsc::channel();
        let _ = self.jobs.send(Job {
            texts: Vec::new(),
            until: None,
            reply,
        });
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn ready(&self) -> bool {
        matches!(*self.state(), State::Ready)
    }

    pub fn failed(&self) -> bool {
        matches!(*self.state(), State::Failed(_))
    }

    /// Whether it embeds no more: its load failed, it was idle too long, or it was dropped.
    pub fn gone(&self) -> bool {
        matches!(*self.state(), State::Failed(_) | State::Gone)
    }

    fn why(&self) -> String {
        match &*self.state() {
            State::Failed(why) => why.clone(),
            _ => "the local model was dropped".to_owned(),
        }
    }

    /// The texts' vectors, in order. With a `wait`, a caller gets `Loading` at once while the
    /// model loads, and `Timeout` past it; with none, it waits for the load and every text.
    pub fn embed(
        &self,
        texts: &[String],
        wait: Option<Duration>,
    ) -> std::result::Result<Vec<Vec<f32>>, Busy> {
        match &*self.state() {
            State::Loading if wait.is_some() => return Err(Busy::Loading),
            State::Failed(why) => return Err(Busy::Failed(why.clone())),
            State::Gone => return Err(Busy::Failed("the local model was dropped".into())),
            State::Loading | State::Ready => {}
        }
        let (reply, answer) = mpsc::channel();
        let until = wait.map(|wait| Instant::now() + wait);
        let job = Job {
            texts: texts.to_vec(),
            until,
            reply,
        };
        if self.jobs.send(job).is_err() {
            return Err(Busy::Failed(self.why()));
        }
        let got = match wait {
            Some(wait) => answer.recv_timeout(wait),
            None => answer.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(Ok(vectors)) => Ok(vectors),
            Ok(Err(why)) => Err(Busy::Failed(why)),
            Err(RecvTimeoutError::Timeout) => Err(Busy::Timeout),
            // A job past its `until` is dropped unrun, which can reach its caller just before
            // the caller's own wait ends (macOS CI on #426): a timeout, the model still loaded.
            Err(RecvTimeoutError::Disconnected)
                if until.is_some_and(|until| Instant::now() >= until) && !self.gone() =>
            {
                Err(Busy::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(Busy::Failed(self.why())),
        }
    }
}

#[cfg(test)]
mod tests;
