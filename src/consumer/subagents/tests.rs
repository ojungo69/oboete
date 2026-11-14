use super::*;
use crate::raw::{self, test_event};
use crate::worker;
use serde_json::json;

fn ev(kind: &str, body: Value) -> Event {
    Event {
        kind: kind.into(),
        ..test_event(&body.to_string())
    }
}

fn rows(k: &Connection) -> Vec<(i64, String, String, Option<String>, bool)> {
    k.prepare(
        "SELECT seq, fact, agent_id, agent_type, returned FROM subagent_facts
             ORDER BY seq, fact, agent_id",
    )
    .unwrap()
    .query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn drain(home: &std::path::Path, store: &Raw) -> Connection {
    let mut consumers: Vec<Box<dyn worker::Consumer>> = vec![Box::new(Subagents)];
    let mut k = crate::knowledge::open(home).unwrap();
    worker::drain(store, &mut k, &mut consumers).unwrap();
    k
}

/// Slice 2 (docs/subagents.md): a live tool record with an `agent_id` is a step, a stop of any
/// source a stop, a `launched` a launch (returned only when completed and not async), one record
/// both when a subagent starts another; a transcript's tool record and a Codex one are none; a
/// record from before slice 2 is read from its output.
#[test]
fn each_record_says_what_it_says_of_a_subagent() {
    let home = tempfile::tempdir().unwrap();
    let mut store = raw::open(home.path()).unwrap();
    let mut seqs = Vec::new();
    let mut add = |e: Event| seqs.push(store.append(&e).unwrap());
    add(ev(
        "tool",
        json!({"tool": "Read", "agent_id": "a1", "agent_type": "Explore"}),
    ));
    add(ev(
        SUBAGENT_STOP,
        json!({"agent_id": "a1", "agent_type": "Explore"}),
    ));
    add(ev(
        "tool",
        json!({"tool": "Agent", "agent_id": "a1", "launched": {"agent_id": "a2",
                   "status": "completed", "async": false}}),
    ));
    add(ev(
        "tool",
        json!({"tool": "Agent", "launched": {"agent_id": "a3", "status": "async_launched",
                   "async": true}}),
    ));
    let old = json!({"agentId": "a4", "status": "completed", "isAsync": false});
    add(ev(
        "tool",
        json!({"tool": "Task", "output": old.to_string()}),
    ));
    // Run in the background, as capture reads `is_async` too: no return.
    for (id, flag) in [("b4", json!(true)), ("c4", json!(1))] {
        let old = json!({"agentId": id, "status": "completed", "is_async": flag});
        add(ev(
            "tool",
            json!({"tool": "Task", "output": old.to_string()}),
        ));
    }
    add(Event {
        source: "replay".into(),
        ..ev("tool", json!({"tool": "Read", "agent_id": "a5"}))
    });
    add(Event {
        source: "transcript".into(),
        ..ev("tool", json!({"tool": "Read", "agent_id": "a6"}))
    });
    add(Event {
        agent: "codex".into(),
        ..ev("tool", json!({"tool": "exec", "agent_id": "a7"}))
    });
    // Completed in the background is no foreground call's return.
    add(ev(
        "tool",
        json!({"tool": "Agent", "launched": {"agent_id": "a8", "status": "completed",
                   "async": true}}),
    ));
    // An id the capture gate masked names no subagent.
    add(ev(
        "tool",
        json!({"tool": "Agent", "agent_id": "[REDACTED]", "launched": {"agent_id":
                   "x-[REDACTED]", "status": "completed", "async": false}}),
    ));
    add(ev(SUBAGENT_STOP, json!({"agent_id": "[REDACTED]"})));
    let k = drain(home.path(), &store);
    let t = |s: &str| Some(s.to_owned());
    assert_eq!(
        rows(&k),
        [
            (seqs[0], "step".into(), "a1".into(), t("Explore"), false),
            (seqs[1], "stop".into(), "a1".into(), t("Explore"), false),
            (seqs[2], "launch".into(), "a2".into(), None, true),
            (seqs[2], "step".into(), "a1".into(), None, false),
            (seqs[3], "launch".into(), "a3".into(), None, false),
            (seqs[4], "launch".into(), "a4".into(), None, true),
            (seqs[5], "launch".into(), "b4".into(), None, false),
            (seqs[6], "launch".into(), "c4".into(), None, false),
            (seqs[7], "step".into(), "a5".into(), None, false),
            (seqs[10], "launch".into(), "a8".into(), None, false),
        ]
    );
}

/// Slice 2 (Codex on slice 2): when raw loses a device's tombstone of another device's record,
/// that device's rewind makes the record's rows again.
#[test]
fn a_rewind_makes_again_what_another_devices_lost_tombstone_took() {
    let from = tempfile::tempdir().unwrap();
    let mut store = raw::open(from.path()).unwrap();
    let step = ev("tool", json!({"tool": "Read", "agent_id": "a1"}));
    let a = store.append(&step).unwrap();
    let first = store.device().to_owned();
    drop(drain(from.path(), &store));
    drop(store);
    let to = tempfile::tempdir().unwrap();
    for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
        if from.path().join(f).exists() {
            std::fs::copy(from.path().join(f), to.path().join(f)).unwrap();
        }
    }
    let mut store = raw::open(to.path()).unwrap();
    let second = store.device().to_owned();
    (store.append_tombstone(Target::Record {
        device: first,
        seq: a,
    }))
    .unwrap();
    let k = drain(to.path(), &store);
    assert!(rows(&k).is_empty());
    (Connection::open(to.path().join("raw.db")).unwrap())
        .execute("DELETE FROM records WHERE type = 'tombstone'", [])
        .unwrap();
    Subagents.rewind(&k, &second, 0).unwrap();
    drop(k);
    let k = drain(to.path(), &store);
    let ids: Vec<String> = rows(&k).into_iter().map(|r| r.2).collect();
    assert_eq!(ids, ["a1"]);
}

/// Slice 2: a forget of a record takes its rows, a mask makes them again from what is left (an
/// id it touched names no subagent), and a rewind or a rebuild gives the same rows; an expired
/// record gives none to a rebuild.
#[test]
fn a_records_rows_follow_its_tombstones_rewinds_and_expiry() {
    let home = tempfile::tempdir().unwrap();
    let mut store = raw::open(home.path()).unwrap();
    let step = |id: &str| ev("tool", json!({"tool": "Read", "agent_id": id}));
    let a = store.append(&step("a1")).unwrap();
    let b = store.append(&step("b2")).unwrap();
    let c = store.append(&step("c3")).unwrap();
    let ids = |k: &Connection| -> Vec<String> { rows(k).into_iter().map(|r| r.2).collect() };
    let k = drain(home.path(), &store);
    assert_eq!(ids(&k), ["a1", "b2", "c3"]);
    let device = store.device().to_owned();
    (store.append_tombstone(Target::Record {
        device: device.clone(),
        seq: a,
    }))
    .unwrap();
    // A mask of the id leaves a body whose id is the mask's.
    let body = store.after(&device, b - 1, 1).unwrap()[0].clone();
    let raw::Item::Event(e) = body.item else {
        panic!("an event")
    };
    let at = e.body.find("b2").unwrap() as i64;
    (store.append_tombstone(Target::Range {
        device: device.clone(),
        seq: b,
        offset: at,
        length: 2,
    }))
    .unwrap();
    let k = drain(home.path(), &store);
    assert_eq!(ids(&k), ["c3"]);
    // A rewind drops the rows and makes them again from seq 1.
    let mut consumer = Subagents;
    consumer.rewind(&k, &device, 0).unwrap();
    assert!(rows(&k).is_empty());
    drop(k);
    let k = drain(home.path(), &store);
    assert_eq!(ids(&k), ["c3"]);
    // A rebuild from raw alone gives the same; an expired record gives none. The rows and the
    // checkpoint go, not the file, which Windows keeps while it is open.
    store.expire_for_test(&[c]);
    k.execute("DELETE FROM subagent_facts", []).unwrap();
    k.execute("DELETE FROM checkpoints WHERE consumer = ?1", [NAME])
        .unwrap();
    drop(k);
    let k = drain(home.path(), &store);
    assert!(ids(&k).is_empty());
}
