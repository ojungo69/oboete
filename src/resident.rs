//! Milestone 4 D14: a local model held by one thread of the process that uses it (the worker,
//! MCP, the viewer, the CLI's `--vectors`), never served to another process. The thread loads
//! it, embeds one text at a time, and drops it when its owner drops the `Resident`, or after
//! `idle` without a request.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

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
                let answer = (job.texts.iter())
                    .map(|text| model(text).and_then(crate::embed::unit))
                    .collect::<Result<Vec<_>>>()
                    .map_err(|e| format!("{e:#}"));
                let _ = job.reply.send(answer);
            }
        });
        Resident { jobs, state }
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
        let job = Job {
            texts: texts.to_vec(),
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
            Err(RecvTimeoutError::Disconnected) => Err(Busy::Failed(self.why())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    fn model(delay: Duration) -> Load {
        Box::new(move || {
            std::thread::sleep(delay);
            Ok(Box::new(|text: &str| Ok(crate::embed::stub::vector("t", text))) as Model)
        })
    }

    fn texts(t: &[&str]) -> Vec<String> {
        t.iter().map(|t| t.to_string()).collect()
    }

    /// Loading, a caller with a wait gets `Loading` at once and one without waits for the load;
    /// the vectors come in order, as unit vectors.
    #[test]
    fn it_loads_on_its_thread_and_answers_in_order() {
        let r = Resident::start(model(Duration::from_millis(300)), None);
        let t = Instant::now();
        let busy = r.embed(&texts(&["a"]), Some(Duration::from_secs(5)));
        assert_eq!(busy, Err(Busy::Loading));
        assert!(t.elapsed() < Duration::from_millis(200));
        let got = r.embed(&texts(&["alpha", "beta"]), None).unwrap();
        assert!(r.ready());
        assert_eq!(got[0], crate::embed::stub::vector("t", "alpha"));
        assert_eq!(got[1], crate::embed::stub::vector("t", "beta"));
        let got = r.embed(&texts(&["gamma"]), Some(Duration::from_secs(5)));
        assert_eq!(got.unwrap()[0], crate::embed::stub::vector("t", "gamma"));
    }

    #[test]
    fn a_failed_load_or_a_bad_vector_is_a_failure_that_says_why() {
        let r = Resident::start(Box::new(|| anyhow::bail!("no runtime")), None);
        assert_eq!(
            r.embed(&texts(&["a"]), None),
            Err(Busy::Failed("no runtime".into()))
        );
        assert!(r.gone());
        assert_eq!(
            r.embed(&texts(&["a"]), Some(Duration::from_secs(1))),
            Err(Busy::Failed("no runtime".into()))
        );
        let bad: Load =
            Box::new(|| Ok(Box::new(|_: &str| Ok(vec![f32::NAN; crate::embed::DIM])) as Model));
        let r = Resident::start(bad, None);
        let Err(Busy::Failed(why)) = r.embed(&texts(&["a"]), None) else {
            panic!("a NaN vector was taken");
        };
        assert!(why.contains("finite"), "{why}");
    }

    /// A model whose thread panics is gone, so its owner starts another.
    #[test]
    fn a_panic_on_the_models_thread_leaves_it_gone() {
        let load: Load =
            Box::new(|| Ok(Box::new(|_: &str| -> Result<Vec<f32>> { panic!("boom") }) as Model));
        let r = Resident::start(load, None);
        assert!(matches!(
            r.embed(&texts(&["a"]), None),
            Err(Busy::Failed(_))
        ));
        let t = Instant::now();
        while !r.gone() {
            assert!(t.elapsed() < Duration::from_secs(5), "never gone");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// D14: idle past its time, or dropped by its owner, the model goes with its thread.
    #[test]
    fn the_model_goes_when_idle_or_dropped() {
        let r = Resident::start(model(Duration::ZERO), Some(Duration::from_millis(100)));
        r.embed(&texts(&["a"]), None).unwrap();
        let t = Instant::now();
        while !r.gone() {
            assert!(t.elapsed() < Duration::from_secs(5), "never went idle");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            r.embed(&texts(&["a"]), None),
            Err(Busy::Failed(_))
        ));
        struct Held(Arc<AtomicBool>);
        impl Drop for Held {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let freed = Arc::new(AtomicBool::new(false));
        let held = Held(Arc::clone(&freed));
        let load: Load = Box::new(move || {
            Ok(Box::new(move |text: &str| {
                let _ = &held;
                Ok(crate::embed::stub::vector("t", text))
            }) as Model)
        });
        let r = Resident::start(load, None);
        r.embed(&texts(&["a"]), None).unwrap();
        drop(r);
        let t = Instant::now();
        while !freed.load(Ordering::SeqCst) {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "the model outlived its owner"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
