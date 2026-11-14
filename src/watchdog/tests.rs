    use super::*;
    use std::sync::mpsc;

    const TICK: Duration = Duration::from_millis(10);

    /// A loop on this thread that has marked once, watched every 10 ms, stuck after `still` looks
    /// without a mark: the number `stuck` got, once it ran.
    fn watched(still: u64) -> (Watch, mpsc::Receiver<u64>) {
        let (tx, rx) = mpsc::channel();
        // The test may be over by then: nobody waits for the number.
        let ends = watch(
            Watched::here(),
            TICK,
            still,
            |_| {},
            move |n| {
                let _ = tx.send(n);
            },
        );
        progressed();
        (ends, rx)
    }

    /// A worker that has not begun its loop (it restores or migrates a large store first) is not
    /// still yet.
    #[test]
    fn a_loop_that_has_not_marked_yet_is_not_stuck() {
        let watched = Watched::here();
        let (tx, stuck) = mpsc::channel();
        let _watch = watch(
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

    /// A run that returns ends its watch: `stuck` never runs, and is dropped with the thread.
    #[test]
    fn a_watch_ends_with_its_run() {
        let (ends, stuck) = watched(3);
        drop(ends);
        assert_eq!(
            stuck.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn looks_round_up() {
        assert_eq!(looks(Duration::from_secs(301), EVERY), 11);
        assert_eq!(looks(Duration::from_secs(300), EVERY), 10);
        assert_eq!(looks(Duration::ZERO, EVERY), 0);
    }
