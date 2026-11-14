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

/// docs/subagents.md, slice 3: whether the facts hold every record of `device` up to `seq`: the
/// consumer's checkpoint has reached it with no rebuild owed (none is reached with no checkpoint).
pub(crate) fn reached(k: &Connection, device: &str, seq: i64) -> Result<bool> {
    schema(k)?;
    if crate::knowledge::checkpoint::get(k, NAME, device)? < seq {
        return Ok(false);
    }
    let owed: bool = k.query_row(
        "SELECT EXISTS(SELECT 1 FROM subagent_rebuild WHERE device = ?1)",
        [device],
        |r| r.get(0),
    )?;
    Ok(!owed)
}

/// The seqs of one subagent's (agent, session, id) `fact` rows on `device` strictly between two
/// seqs, oldest first.
pub(crate) fn fact_seqs(
    k: &Connection,
    device: &str,
    (agent, session, agent_id): (&str, &str, &str),
    fact: &str,
    (after, before): (i64, i64),
) -> Result<Vec<i64>> {
    let mut q = k.prepare_cached(
        "SELECT seq FROM subagent_facts WHERE device = ?1 AND fact = ?2 AND agent = ?3
           AND session = ?4 AND agent_id = ?5 AND seq > ?6 AND seq < ?7 ORDER BY seq",
    )?;
    let rows = q.query_map(
        params![device, fact, agent, session, agent_id, after, before],
        |r| r.get(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
    let named = |v: &Value| (v.as_str()).filter(|s| unmasked(s)).map(str::to_owned);
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

/// An id no mask touched: a tombstone's (`*`) or the capture gate's (`[REDACTED]`), which no agent's
/// id holds, makes ids alike that were not (Codex on slice 2).
fn unmasked(id: &str) -> bool {
    !id.is_empty() && !id.contains('*') && !id.contains(crate::redact::MASK)
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
        // As capture reads a response (`capture::launched`; cubic on #850).
        let in_back = output["isAsync"] == true
            || output["is_async"] == true
            || output["is_async"] == 1
            || status == "async_launched";
        serde_json::json!({"agent_id": output["agentId"], "status": status, "async": in_back})
    } else {
        return None;
    };
    let agent_id = (launched["agent_id"].as_str()).filter(|s| unmasked(s))?;
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

    /// Lost commits may hold tombstones that changed older records' rows, another device's too (a
    /// forget written here of another device's record): every device's rows are dropped and made
    /// again from seq 1 (Codex on slice 2; the manifest's facts make the device's alone again).
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "INSERT OR IGNORE INTO subagent_rebuild(device)
             SELECT device FROM checkpoints WHERE consumer = ?1",
            [NAME],
        )?;
        k.execute(
            "INSERT OR IGNORE INTO subagent_rebuild(device) VALUES(?1)",
            [device],
        )?;
        k.execute("DELETE FROM subagent_facts", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
