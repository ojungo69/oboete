use crate::raw::{self, Item};

fn enc(home: &std::path::Path, seq: i64) -> String {
    let c = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    c.query_row("SELECT enc FROM records WHERE seq = ?1", [seq], |r| {
        r.get(0)
    })
    .unwrap()
}

#[test]
fn the_worker_compresses_a_record_and_it_reads_back_the_same() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let long = "the same words again and again ".repeat(200);
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event(&long)).unwrap();
    raw.append(&raw::test_event("hi")).unwrap(); // too small for zstd to win
    crate::worker::run_once(p).unwrap();
    assert_eq!(enc(p, 1), "zstd");
    assert_eq!(enc(p, 2), "plain");
    let recs = raw.after(raw.device(), 0, 10).unwrap();
    let bodies: Vec<&str> = recs
        .iter()
        .map(|r| match &r.item {
            Item::Event(e) => e.body.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(bodies, [long.as_str(), "hi"]);
    // Indexed before it was compressed, and found after (Task 6's index runs first).
    let hits = crate::search::raw(p, "again and again", None, 10).unwrap();
    assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), [1]);
    let c = rusqlite::Connection::open(p.join("raw.db")).unwrap();
    let stored: i64 = c
        .query_row("SELECT length(body) FROM records WHERE seq = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!((stored as usize) < long.len() / 10, "{stored}");
}
