//! Slice 2 of docs/subagents.md: what raw's records say of each Claude Code subagent, one row a
//! record and what it says, with no text. A step is a live tool record with its `agent_id`, a stop a
//! `subagent_stop` record (the hook's or the owner's), a launch a live record's `launched` (the
//! subagent an `Agent` or `Task` call started). My Work reads them (`my_work::subagents`).

use crate::raw::{Event, Item, Raw, SUBAGENT_STOP, Target};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::Value;

pub struct Subagents;

/// Records per step, one knowledge.db transaction each.
const BATCH: usize = 500;

/// The consumer's name, its checkpoints' key.
pub(crate) const NAME: &str = "subagents";

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS subagent_facts(
           device TEXT NOT NULL, seq INTEGER NOT NULL, fact TEXT NOT NULL,
           agent_id TEXT NOT NULL, agent TEXT NOT NULL, session TEXT NOT NULL,
           ts INTEGER NOT NULL, agent_type TEXT, returned INTEGER NOT NULL DEFAULT 0,
           PRIMARY KEY (device, seq, fact, agent_id));
         CREATE INDEX IF NOT EXISTS subagent_facts_session
           ON subagent_facts(agent, session);
         CREATE TABLE IF NOT EXISTS subagent_rebuild(device TEXT PRIMARY KEY);",
    )?;
    Ok(())
}

/// One thing a record says of a subagent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fact {
    pub fact: &'static str,
    pub agent_id: String,
    pub agent_type: Option<String>,
    /// A launch whose call returned with its subagent (a foreground call that completed).
    pub returned: bool,
}

/// What `e` says of Claude Code's subagents: a subagent's own call that starts another is a step
/// of the one and a launch of the other.
pub(crate) fn facts(e: &Event) -> Vec<Fact> {
    if e.agent != "claude" {
        return Vec::new();
    }
    let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
    // An id a mask touched (`*`, which no agent's id holds) names no subagent of the record's.
    let named = |v: &Value| {
        (v.as_str())
            .filter(|s| !s.is_empty() && !s.contains('*'))
            .map(str::to_owned)
    };
    let mut out = Vec::new();
    if e.kind == SUBAGENT_STOP {
        if let Some(agent_id) = named(&body["agent_id"]) {
            out.push(Fact {
                fact: "stop",
                agent_id,
                agent_type: named(&body["agent_type"]),
                returned: false,
            });
        }
        return out;
    }
    if e.kind != "tool" || !crate::raw::is_live(&e.source) {
        return out;
    }
    if let Some(agent_id) = named(&body["agent_id"]) {
        out.push(Fact {
            fact: "step",
            agent_id,
            agent_type: named(&body["agent_type"]),
            returned: false,
        });
    }
    if let Some((agent_id, returned)) = launched(&body) {
        out.push(Fact {
            fact: "launch",
            agent_id,
            agent_type: None,
            returned,
        });
    }
    out
}

/// The subagent a call started and whether the call returned with it: its `launched`, or, in a
/// record from before slice 2, its output when that still parses.
fn launched(body: &Value) -> Option<(String, bool)> {
    let kept = &body["launched"];
    let launched = if kept.is_object() {
        kept.clone()
    } else if matches!(body["tool"].as_str(), Some("Agent" | "Task")) {
        let output: Value = serde_json::from_str(body["output"].as_str()?).ok()?;
        let status = output["status"].clone();
        let in_back = output["isAsync"] == true || status == "async_launched";
        serde_json::json!({"agent_id": output["agentId"], "status": status, "async": in_back})
    } else {
        return None;
    };
    let agent_id = (launched["agent_id"].as_str()).filter(|s| !s.is_empty() && !s.contains('*'))?;
    let returned = launched["status"] == "completed" && launched["async"] != true;
    Some((agent_id.to_owned(), returned))
}

fn insert(k: &Connection, device: &str, seq: i64, e: &Event) -> Result<()> {
    for f in facts(e) {
        k.execute(
            "INSERT OR REPLACE INTO subagent_facts(device, seq, fact, agent_id, agent, session, ts,
               agent_type, returned)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                device,
                seq,
                f.fact,
                f.agent_id,
                e.agent,
                e.session,
                e.ts,
                f.agent_type,
                f.returned
            ],
        )?;
    }
    Ok(())
}

impl Consumer for Subagents {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Every device with records in raw.db, and this one, as `fts-text` reads them: a copied
    /// home's records under its old id are listed by My Work too.
    fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
        let mut devices = raw.devices()?;
        if !devices.iter().any(|d| d == raw.device()) {
            devices.push(raw.device().to_owned());
        }
        Ok(devices)
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        // After a rewind, from seq 1, as the manifest's facts: its checkpoint moves back, which the
        // worker takes.
        let from = if k.execute("DELETE FROM subagent_rebuild WHERE device = ?1", [device])? > 0 {
            0
        } else {
            after
        };
        let recs = raw.after(device, from, BATCH)?;
        for r in &recs {
            match &r.item {
                Item::Event(e) => insert(k, device, r.seq, e)?,
                // The target's rows again, from what raw gives now: none once removed or expired,
                // what a mask left otherwise.
                Item::Tombstone(
                    Target::Record { device: d, seq } | Target::Range { device: d, seq, .. },
                ) => {
                    k.execute(
                        "DELETE FROM subagent_facts WHERE device = ?1 AND seq = ?2",
                        params![d, seq],
                    )?;
                    let now = (raw.after(d, seq - 1, 1)?.into_iter())
                        .find(|x| x.seq == *seq)
                        .map(|x| x.item);
                    if let Some(Item::Event(e)) = now {
                        insert(k, d, *seq, &e)?;
                    }
                }
                // An expired record's body is gone: it says nothing, and a rebuild makes no row of
                // it. The read passes over a row whose record expired since (retention rebuilds).
                Item::Expired(_) | Item::Removed => {}
            }
        }
        Ok(recs.last().map_or(from, |r| r.seq))
    }

    /// Lost commits may hold tombstones that changed older records' rows: the device's rows are
    /// dropped and made again from seq 1, as the manifest's facts are.
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        k.execute("DELETE FROM subagent_facts WHERE device = ?1", [device])?;
        k.execute(
            "INSERT OR IGNORE INTO subagent_rebuild(device) VALUES(?1)",
            [device],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
                (seqs[5], "step".into(), "a5".into(), None, false),
            ]
        );
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
        // A rebuild from raw alone gives the same; an expired record gives none.
        store.expire_for_test(&[c]);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let k = drain(home.path(), &store);
        assert!(ids(&k).is_empty());
    }
}
