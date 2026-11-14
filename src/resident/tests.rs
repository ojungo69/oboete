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
/// the vectors come in order, as unit vectors. A gate holds the load until the first call has
/// its answer, so no machine finishes it sooner.
#[test]
fn it_loads_on_its_thread_and_answers_in_order() {
    let (release, gate) = mpsc::channel::<()>();
    let load: Load = Box::new(move || {
        gate.recv().unwrap();
        Ok(Box::new(|text: &str| Ok(crate::embed::stub::vector("t", text))) as Model)
    });
    let r = Resident::start(load, None);
    let busy = r.embed(&texts(&["a"]), Some(Duration::from_secs(5)));
    assert_eq!(busy, Err(Busy::Loading));
    release.send(()).unwrap();
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

/// Searches faster than the model: each that timed out is dropped unrun, so the next one is not
/// held behind work nobody waits for (Codex on Task 10). A gate holds the model on one text
/// until every late query has timed out, so no scheduler can run one in time (macOS CI on #426).
#[test]
fn a_timed_out_query_is_dropped_unrun() {
    use std::sync::atomic::AtomicUsize;
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&runs);
    let (held, holding) = mpsc::channel::<()>();
    let (release, gate) = mpsc::channel::<()>();
    let load: Load = Box::new(move || {
        Ok(Box::new(move |text: &str| {
            counted.fetch_add(1, Ordering::SeqCst);
            if text == "hold" {
                held.send(()).unwrap();
                gate.recv().unwrap();
            }
            Ok(crate::embed::stub::vector("t", text))
        }) as Model)
    });
    let r = Arc::new(Resident::start(load, None));
    r.embed(&texts(&["warm"]), None).unwrap();
    let holder = Arc::clone(&r);
    let hold = std::thread::spawn(move || holder.embed(&texts(&["hold"]), None));
    holding.recv().unwrap();
    for i in 0..4 {
        let late = r.embed(&texts(&[&format!("q{i}")]), Some(Duration::from_millis(20)));
        assert_eq!(late, Err(Busy::Timeout));
    }
    release.send(()).unwrap();
    hold.join().unwrap().unwrap();
    r.embed(&texts(&["last"]), None).unwrap();
    // The warm-up, the held text and the last: the four late queries were never run.
    assert_eq!(runs.load(Ordering::SeqCst), 3);
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

/// Parity row 31: kept, the model stays past its idle time, and goes once no longer kept. Kept
/// every 50 ms against an idle time of a second, so a CI runner that sleeps a thread long under
/// load does not let it go between two keeps (#675).
#[test]
fn kept_it_stays_past_its_idle_and_goes_after() {
    let r = Resident::start(model(Duration::ZERO), Some(Duration::from_secs(1)));
    r.embed(&texts(&["a"]), None).unwrap();
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(2_500) {
        r.keep();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!r.gone(), "kept, it stays");
    let t = Instant::now();
    while !r.gone() {
        assert!(t.elapsed() < Duration::from_secs(5), "never went idle");
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
