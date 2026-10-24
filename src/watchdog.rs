//! Parity row 2 (docs/resident.md R14): a worker whose loop makes no progress leaves, so that the
//! next hook starts a new one, as claude-mem's launchers reclaim a wedged worker. The loop marks
//! each turn (a round, a batch, a wait's tick); a call to a provider marks its start and its end
//! and may keep the loop still for as long as its own timeout allows. Nothing counts before the
//! loop's first mark: a start that restores or migrates a large store is not stillness, as
//! claude-mem gives a new worker a boot grace.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// than a call out allows, `stuck` runs with that number and the watch ends.
pub(crate) fn watch(
    watched: Arc<Watched>,
    every: Duration,
    still: u64,
    mut looked: impl FnMut(u64) + Send + 'static,
    stuck: impl FnOnce(u64) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut last = watched.marks.load(Ordering::Relaxed);
        let mut unchanged = 0;
        loop {
            std::thread::sleep(every);
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
    })
}

/// How many looks `d` spans, rounded up.
fn looks(d: Duration, every: Duration) -> u64 {
    u64::try_from(d.as_millis().div_ceil(every.as_millis().max(1))).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const TICK: Duration = Duration::from_millis(10);

    /// A loop on this thread that has marked once, watched every 10 ms, stuck after `still` looks
    /// without a mark: the number `stuck` got, once it ran.
    fn watched(still: u64) -> (Arc<Watched>, mpsc::Receiver<u64>) {
        let watched = Watched::here();
        let (tx, rx) = mpsc::channel();
        // The test may be over by then: nobody waits for the number.
        watch(
            Arc::clone(&watched),
            TICK,
            still,
            |_| {},
            move |n| {
                let _ = tx.send(n);
            },
        );
        progressed();
        (watched, rx)
    }

    /// A worker that has not begun its loop (it restores or migrates a large store first) is not
    /// still yet.
    #[test]
    fn a_loop_that_has_not_marked_yet_is_not_stuck() {
        let watched = Watched::here();
        let (tx, stuck) = mpsc::channel();
        watch(
            watched,
            TICK,
            3,
            |_| {},
            move |n| {
                let _ = tx.send(n);
            },
        );
        std::thread::sleep(TICK * 20);
        assert!(stuck.try_recv().is_err());
        progressed();
        assert_eq!(stuck.recv_timeout(Duration::from_secs(5)), Ok(4));
    }

    #[test]
    fn a_loop_that_stops_marking_is_stuck() {
        let (_watched, stuck) = watched(3);
        assert_eq!(stuck.recv_timeout(Duration::from_secs(5)), Ok(4));
    }

    /// A second of stillness allowed, so that a loaded machine's pause between two marks does not
    /// count as stuck.
    #[test]
    fn a_loop_that_marks_is_not_stuck() {
        let (_watched, stuck) = watched(100);
        for _ in 0..50 {
            progressed();
            std::thread::sleep(TICK);
        }
        assert!(stuck.try_recv().is_err());
    }

    /// A call out waits its timeout and the margin: 5 minutes and 100 ms here, far more than the
    /// 3 looks a loop gets without one. Back, the loop has 3 looks again.
    #[test]
    fn a_call_out_waits_for_its_timeout() {
        let (_watched, stuck) = watched(3);
        let call = calling(Duration::from_millis(100));
        std::thread::sleep(TICK * 20);
        assert!(stuck.try_recv().is_err(), "stuck during a call out");
        drop(call);
        assert_eq!(stuck.recv_timeout(Duration::from_secs(5)), Ok(4));
    }

    /// Another thread's loop is none of this watch's business: a test's loop marks nothing.
    #[test]
    fn marks_from_an_unwatched_thread_count_nothing() {
        let (_watched, stuck) = watched(3);
        std::thread::spawn(|| {
            for _ in 0..50 {
                progressed();
                let _call = calling(Duration::from_secs(60));
                std::thread::sleep(TICK);
            }
        });
        assert_eq!(stuck.recv_timeout(Duration::from_secs(5)), Ok(4));
    }

    #[test]
    fn looks_round_up() {
        assert_eq!(looks(Duration::from_secs(301), EVERY), 11);
        assert_eq!(looks(Duration::from_secs(300), EVERY), 10);
        assert_eq!(looks(Duration::ZERO, EVERY), 0);
    }
}
