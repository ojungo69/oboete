//! Parity row 2 (docs/resident.md R14): a worker whose loop makes no progress leaves, so that the
//! next hook starts a new one, as claude-mem's launchers reclaim a wedged worker. The loop marks
//! each turn (a round, a batch, a wait's tick); a call to a provider marks its start and its end
//! and may keep the loop still for as long as its own timeout allows. Nothing counts before the
//! loop's first mark: a start that restores or migrates a large store is not stillness, as
//! claude-mem gives a new worker a boot grace.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How often the watchdog looks.
pub(crate) const EVERY: Duration = Duration::from_secs(30);

/// Looks that find no new mark before the loop counts as stuck: 15 minutes at `EVERY`. Looks are
/// counted, not times compared, so a clock that steps (WSL under load, a suspend) moves nothing.
pub(crate) const STILL: u64 = 30;

/// What a call to a provider may take past its own timeout before the loop counts as stuck.
const CALL_MARGIN: Duration = Duration::from_secs(300);

/// What one worker's loop has done, as its watchdog reads it.
#[derive(Default)]
pub(crate) struct Watched {
    marks: AtomicU64,
    /// Calls out now, and the longest stillness the longest of them allows.
    calls: Mutex<(u32, Duration)>,
    /// The run it watches has returned: its watch ends.
    ended: AtomicBool,
}

thread_local! {
    /// The loop's thread's `Watched`: set by `oboete worker` only, so a test's loop marks nothing.
    static WATCHED: RefCell<Option<Arc<Watched>>> = const { RefCell::new(None) };
}

impl Watched {
    /// This thread's loop is watched from now on.
    pub(crate) fn here() -> Arc<Watched> {
        let watched = Arc::new(Watched::default());
        WATCHED.with(|w| *w.borrow_mut() = Some(Arc::clone(&watched)));
        watched
    }

    fn mark(&self) {
        self.marks.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn marks(&self) -> u64 {
        self.marks.load(Ordering::Relaxed)
    }
}

/// One run's watch, which ends when this is dropped: with the run, however it returns, so that a
/// process that goes on (a test binary that runs many workers) is not ended by a watch that a run
/// before left behind (CodeRabbit on #613).
#[must_use = "the watch ends when this is dropped"]
pub(crate) struct Watch(Arc<Watched>);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::Relaxed);
    }
}

/// The loop made progress.
pub(crate) fn progressed() {
    WATCHED.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            w.mark();
        }
    });
}

/// A call out, which may keep the loop still for up to its timeout, until it is dropped.
pub(crate) struct Calling(Option<Arc<Watched>>);

pub(crate) fn calling(timeout: Duration) -> Calling {
    let watched = WATCHED.with(|w| w.borrow().clone());
    if let Some(w) = &watched {
        let mut calls = w.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls.0 += 1;
        calls.1 = calls.1.max(timeout.saturating_add(CALL_MARGIN));
        w.mark();
    }
    Calling(watched)
}

impl Drop for Calling {
    fn drop(&mut self) {
        if let Some(w) = &self.0 {
            let mut calls = w.calls.lock().unwrap_or_else(PoisonError::into_inner);
            calls.0 = calls.0.saturating_sub(1);
            // Two calls out at once keep the longer allowance until both are back.
            if calls.0 == 0 {
                calls.1 = Duration::ZERO;
            }
            w.mark();
        }
    }
}

/// On a thread of its own: looks every `every`, and runs `looked` with the looks in a row that
/// found no new mark, from the loop's first mark on. After more than `still` of them, or more
/// than a call out allows, `stuck` runs with that number and the watch ends; it ends too once the
/// `Watch` it returns is dropped.
pub(crate) fn watch(
    watched: Arc<Watched>,
    every: Duration,
    still: u64,
    mut looked: impl FnMut(u64) + Send + 'static,
    stuck: impl FnOnce(u64) + Send + 'static,
) -> Watch {
    let ends = Watch(Arc::clone(&watched));
    std::thread::spawn(move || {
        let mut last = watched.marks.load(Ordering::Relaxed);
        let mut unchanged = 0;
        loop {
            std::thread::sleep(every);
            if watched.ended.load(Ordering::Relaxed) {
                return;
            }
            // Read together under the calls' lock, which a call's start and end mark under: a call
            // that ends between the two reads could otherwise take its allowance away while the
            // mark it made at its end is not seen yet.
            let (marks, allowed) = {
                let calls = watched.calls.lock().unwrap_or_else(PoisonError::into_inner);
                let marks = watched.marks.load(Ordering::Relaxed);
                (marks, still.max(looks(calls.1, every)))
            };
            if marks == 0 {
                continue;
            }
            if marks == last {
                unchanged += 1;
            } else {
                (last, unchanged) = (marks, 0);
            }
            looked(unchanged);
            if unchanged > allowed {
                stuck(unchanged);
                return;
            }
        }
    });
    ends
}

/// How many looks `d` spans, rounded up.
fn looks(d: Duration, every: Duration) -> u64 {
    u64::try_from(d.as_millis().div_ceil(every.as_millis().max(1))).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
