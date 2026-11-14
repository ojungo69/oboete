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
    cleared(&k, id, &reading, &rules).unwrap();
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
