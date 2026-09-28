//! Milestone 3 Task 6: the claims consumer. It reads the op log of every device that has ops and
//! keeps each claim's derivations, quotes and supersedes edges in knowledge.db, with each uid's
//! active derivation (`claims::schema`).

use crate::claims::{ClaimOp, CorrectionOp, Evidence, normalize, schema, uid};
use crate::curate::Span;
use crate::knowledge::checkpoint;
use crate::raw::{Item, Op, OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeSet, HashMap};

pub struct Claims;

/// Ops per step, one knowledge.db transaction each. A window's ops (one batch) are never split
/// across steps, so a sibling a claim supersedes is always in the same step.
const BATCH: usize = 500;

/// A claim op written as a derivation, before its edges.
struct Derived {
    op_seq: i64,
    batch: i64,
    id: String,
    uid: String,
    supersedes: Vec<String>,
}

impl Consumer for Claims {
    fn name(&self) -> &'static str {
        "claims"
    }

    fn devices(&self, raw: &Raw) -> Result<Vec<String>> {
        raw.op_devices()
    }

    fn top(&self, raw: &Raw, device: &str) -> Result<i64> {
        raw.max_op_seq_of(device)
    }

    fn checkpoints(&self) -> &'static str {
        checkpoint::OPS
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let ops = whole_batches(raw, device, after)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        // A window that moves the curation checkpoint (not a recuration, D2): the manifests of
        // its records' checkouts count what is not yet curated again.
        for op in ops
            .iter()
            .filter(|o| o.kind == OpKind::Window && o.body["recurate"] != true)
        {
            if let (Some(from), Some(to)) =
                (op.body["from_seq"].as_i64(), op.body["to_seq"].as_i64())
            {
                crate::consumer::manifest::curated(k, &op.device, from, to)?;
            }
        }
        // A span a recuration covered leaves the queue (Task 11), before its claims are derived:
        // a quote of its own that no longer reads queues it again.
        for op in ops
            .iter()
            .filter(|o| o.kind == OpKind::Window && o.body["recurate"] == true)
        {
            // What this window curated, and the part of its span curated through it (`covers`,
            // when the span took several): each queued span keeps what is left of it on either
            // side (#192), a record it curated only part of whole.
            for range in [&op.body, &op.body["covers"]] {
                let Some(c) = crate::curate::op_span(range) else {
                    continue;
                };
                let whole = Span::records(
                    c.from + i64::from(c.from_offset.is_some()),
                    c.to - i64::from(c.to_offset.is_some()),
                );
                if whole.from > whole.to {
                    continue;
                }
                let rows: Vec<(i64, i64, String, i64)> = k
                    .prepare(
                        "SELECT from_seq, to_seq, op_device, op_seq FROM recurate
                         WHERE device = ?1 AND from_seq <= ?3 AND to_seq >= ?2",
                    )?
                    .query_map(params![op.device, whole.from, whole.to], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                for (from, to, of, at) in rows {
                    k.execute(
                        "DELETE FROM recurate WHERE op_device = ?1 AND op_seq = ?2
                           AND from_seq = ?3",
                        params![of, at, from],
                    )?;
                    for part in Span::records(from, to).minus(&whole) {
                        k.execute(
                            "INSERT OR IGNORE INTO recurate(device, from_seq, to_seq, op_device,
                               op_seq) VALUES(?1, ?2, ?3, ?4, ?5)",
                            params![op.device, part.from, part.to, of, at],
                        )?;
                    }
                }
            }
        }
        let mut derived = Vec::new();
        for op in ops.iter().filter(|o| o.kind == OpKind::Claim) {
            if let Some(d) = derive(raw, k, op)? {
                derived.push(d);
            }
        }
        // The owner's corrections (Task 10): kept by uid, whether or not a claim has it yet.
        let mut corrected = Vec::new();
        for op in ops.iter().filter(|o| o.kind == OpKind::Correction) {
            if let Some(uid) = correction(k, op)? {
                corrected.push(uid);
            }
        }
        // Edges once the whole batch is written: a claim may supersede a sibling after it.
        let siblings: HashMap<(i64, &str), &str> = derived
            .iter()
            .map(|d| ((d.batch, d.id.as_str()), d.uid.as_str()))
            .collect();
        // A corrected uid's search text is its corrected body.
        let mut touched: BTreeSet<String> = corrected.into_iter().collect();
        for d in &derived {
            touched.insert(d.uid.clone());
            for s in &d.supersedes {
                // A sibling's id, else a candidate's uid; anything else names nothing.
                let to = match siblings.get(&(d.batch, s.as_str())) {
                    Some(u) => *u,
                    None if is_uid(s) => s.as_str(),
                    None => continue,
                };
                if to != d.uid {
                    k.execute(
                        "INSERT OR IGNORE INTO edges(op_device, op_seq, to_uid, type)
                         VALUES(?1, ?2, ?3, 'supersedes')",
                        params![device, d.op_seq, to],
                    )?;
                }
            }
        }
        for u in &touched {
            activate(k, u)?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        let uids: Vec<String> = k
            .prepare(
                "SELECT uid FROM derivations WHERE op_device = ?1 AND op_seq > ?2
                 UNION SELECT uid FROM corrections WHERE op_device = ?1 AND op_seq > ?2",
            )?
            .query_map(params![device, to], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for table in [
            "derivations",
            "evidence",
            "edges",
            "claim_skips",
            "recurate",
            "corrections",
        ] {
            k.execute(
                &format!("DELETE FROM {table} WHERE op_device = ?1 AND op_seq > ?2"),
                params![device, to],
            )?;
        }
        for u in &uids {
            activate(k, u)?;
        }
        // Lost window ops move the curation checkpoint back: every checkout of the device counts
        // what is not yet curated again (which records they covered went with them).
        crate::consumer::manifest::curated(k, device, 1, i64::MAX)?;
        Ok(())
    }
}

/// The ops after `after`, up to `BATCH`, with the rest of the last one's batch.
fn whole_batches(raw: &Raw, device: &str, after: i64) -> Result<Vec<Op>> {
    let mut ops = raw.ops_after(device, after, BATCH)?;
    while ops.len() % BATCH == 0 {
        let Some(last) = ops.last() else { break };
        let (batch, at) = (last.batch, last.op_seq);
        let more: Vec<Op> = raw
            .ops_after(device, at, BATCH)?
            .into_iter()
            .take_while(|o| o.batch == batch)
            .collect();
        if more.is_empty() {
            break;
        }
        ops.extend(more);
    }
    Ok(ops)
}

/// A correction op kept in `corrections`, with its uid; otherwise its reason goes to
/// `claim_skips`.
fn correction(k: &Connection, op: &Op) -> Result<Option<String>> {
    let fault = match serde_json::from_value::<CorrectionOp>(op.body.clone()) {
        Ok(c) => match c.fault() {
            None => {
                k.execute(
                    "INSERT INTO corrections(op_device, op_seq, ts, uid, status, body)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![op.device, op.op_seq, op.ts, c.uid, c.status, c.body],
                )?;
                return Ok(Some(c.uid));
            }
            Some(why) => why,
        },
        Err(_) => "not a correction",
    };
    k.execute(
        "INSERT OR REPLACE INTO claim_skips(op_device, op_seq, reason) VALUES(?1, ?2, ?3)",
        params![op.device, op.op_seq, fault],
    )?;
    Ok(None)
}

fn is_uid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `op` as a derivation, when it is a claim whose quotes still read in raw. Otherwise the reason
/// goes to `claim_skips`.
fn derive(raw: &Raw, k: &Connection, op: &Op) -> Result<Option<Derived>> {
    let skip = |reason: &str| -> Result<Option<Derived>> {
        k.execute(
            "INSERT OR REPLACE INTO claim_skips(op_device, op_seq, reason) VALUES(?1, ?2, ?3)",
            params![op.device, op.op_seq, reason],
        )?;
        Ok(None)
    };
    // Part 3b's observation shape, written only where curation was on before Task 7, has none.
    let Ok(c) = serde_json::from_value::<ClaimOp>(op.body.clone()) else {
        return skip("not a claim");
    };
    let Some(first) = c.evidence.first() else {
        return skip("no evidence");
    };
    // The anchor event gives the claim its time and repository (spec 3.4).
    let mut anchor = None;
    for e in &c.evidence {
        match live(raw, e)? {
            Some(event) if anchor.is_none() => anchor = Some(event),
            Some(_) => {}
            None => {
                // Masked or removed after curation: its window is sent again (Task 11), as
                // `Anchors` does for a claim it drops, so a rebuild queues the same windows.
                queue(raw, k, &op.device, op.op_seq, e)?;
                return skip("a quote no longer reads in raw");
            }
        }
    }
    let Some(anchor) = anchor else {
        return skip("no evidence");
    };
    let (kind, status) = normalize(&c.kind, &c.status);
    let uid = uid(kind, first);
    k.execute(
        "INSERT INTO derivations(op_device, op_seq, uid, ts, tier, recipe, kind, status, speaker,
           scope, repo, body, valid_from, anchor_device, anchor_seq)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            op.device,
            op.op_seq,
            uid,
            op.ts,
            c.tier,
            c.recipe,
            kind,
            status,
            c.speaker,
            c.scope,
            anchor.repo,
            c.body,
            anchor.ts,
            first.device,
            first.seq
        ],
    )?;
    for (i, e) in (0_i64..).zip(&c.evidence) {
        k.execute(
            "INSERT INTO evidence(op_device, op_seq, idx, device, seq, offset, length, sentence,
               quote, claim_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                op.device, op.op_seq, i, e.device, e.seq, e.offset, e.length, e.sentence, e.quote,
                e.claim_at
            ],
        )?;
    }
    Ok(Some(Derived {
        op_seq: op.op_seq,
        batch: op.batch,
        id: c.id,
        uid,
        supersedes: c.supersedes,
    }))
}

/// The event `e` anchors on, when its quote still reads verbatim there as `Raw::after` returns
/// it: a tombstoned record reads as nothing and a masked range as its mask, so a secret masked
/// after curation does not come back through a claim (spec 6.4).
fn live(raw: &Raw, e: &Evidence) -> Result<Option<crate::raw::Event>> {
    let record = raw
        .after(&e.device, e.seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == e.seq);
    let Some(Item::Event(event)) = record.map(|r| r.item) else {
        return Ok(None);
    };
    let (Ok(start), Ok(len)) = (usize::try_from(e.offset), usize::try_from(e.length)) else {
        return Ok(None);
    };
    let reads = crate::curate::long_text(&event)
        .and_then(|long| {
            long.get(start..start.checked_add(len)?)
                .map(|q| q == e.quote)
        })
        .unwrap_or(false);
    Ok(reads.then_some(*event))
}

/// The window the claim op `op_seq` of `op_device` came with, for Task 11 to send again: its
/// span, or the quote's event alone when the op came without one.
pub(crate) fn queue(
    raw: &Raw,
    k: &Connection,
    op_device: &str,
    op_seq: i64,
    e: &Evidence,
) -> Result<()> {
    let (device, from, to) = match raw.window_of(op_device, op_seq)? {
        Some((from, to)) => (op_device, from, to),
        None => (e.device.as_str(), e.seq, e.seq),
    };
    // Once per op: an op queued already, whole or in the parts a recuration left, stays so.
    k.execute(
        "INSERT INTO recurate(device, from_seq, to_seq, op_device, op_seq)
         SELECT ?1, ?2, ?3, ?4, ?5
         WHERE NOT EXISTS (SELECT 1 FROM recurate WHERE op_device = ?4 AND op_seq = ?5)",
        params![device, from, to, op_device, op_seq],
    )?;
    Ok(())
}

/// Claims whose quote a tombstone after them masked or removed (a rule the rescan applies after
/// curation, or a forget): each derivation that quoted it goes, with its quotes, edges and search
/// row, and its window is queued for recuration. A raw-seq consumer, after `Claims`.
pub struct Anchors;

impl Consumer for Anchors {
    fn name(&self) -> &'static str {
        "anchors"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let top = raw.max_seq_of(device)?;
        if top <= after {
            return Ok(after);
        }
        for (target, seq) in raw.tombstones_after(device, after)? {
            let quoted: Vec<(String, i64)> = k
                .prepare("SELECT DISTINCT op_device, op_seq FROM evidence WHERE device = ?1 AND seq = ?2")?
                .query_map(params![target, seq], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (op_device, op_seq) in quoted {
                let quotes: Vec<Evidence> = k
                    .prepare(
                        "SELECT device, seq, offset, length, sentence, quote FROM evidence
                         WHERE op_device = ?1 AND op_seq = ?2 ORDER BY idx",
                    )?
                    .query_map(params![op_device, op_seq], |r| {
                        Ok(Evidence {
                            device: r.get(0)?,
                            seq: r.get(1)?,
                            offset: r.get(2)?,
                            length: r.get(3)?,
                            sentence: r.get(4)?,
                            quote: r.get(5)?,
                            claim_at: None,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                let mut dead = None;
                for e in &quotes {
                    if live(raw, e)?.is_none() {
                        dead = Some(e);
                        break;
                    }
                }
                let Some(dead) = dead else { continue };
                queue(raw, k, &op_device, op_seq, dead)?;
                let uid: String = k.query_row(
                    "SELECT uid FROM derivations WHERE op_device = ?1 AND op_seq = ?2",
                    params![op_device, op_seq],
                    |r| r.get(0),
                )?;
                for table in ["derivations", "evidence", "edges"] {
                    k.execute(
                        &format!("DELETE FROM {table} WHERE op_device = ?1 AND op_seq = ?2"),
                        params![op_device, op_seq],
                    )?;
                }
                k.execute(
                    "INSERT OR REPLACE INTO claim_skips(op_device, op_seq, reason)
                     VALUES(?1, ?2, 'a quote no longer reads in raw')",
                    params![op_device, op_seq],
                )?;
                activate(k, &uid)?;
            }
        }
        Ok(top)
    }

    // ponytail: a restore that loses a tombstone does not bring back the claims it dropped (they
    // stay hidden, the safe side) until `oboete rebuild` (Task 11) derives them again.
    fn rewind(&mut self, _k: &Connection, _device: &str, _to: i64) -> Result<()> {
        Ok(())
    }
}

/// `uid`'s active derivation, the highest tier, then the newest (MUST-M18; the op's time, with
/// its device and op_seq only to break a tie), into `claims` and its search row; the row goes
/// when no derivation is left.
fn activate(k: &Connection, uid: &str) -> Result<()> {
    let active: Option<(String, i64, String)> = k
        .query_row(
            "SELECT d.op_device, d.op_seq,
                    COALESCE((SELECT x.body FROM corrections x WHERE x.uid = d.uid
                              AND x.body IS NOT NULL
                              ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                             d.body) || char(10) || COALESCE(
                      (SELECT group_concat(quote, char(10)) FROM evidence e
                       WHERE e.op_device = d.op_device AND e.op_seq = d.op_seq), '')
             FROM derivations d WHERE d.uid = ?1
             ORDER BY d.tier DESC, d.ts DESC, d.op_device DESC, d.op_seq DESC LIMIT 1",
            [uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let rowid: Option<i64> = k
        .query_row("SELECT rowid FROM claims WHERE uid = ?1", [uid], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(rowid) = rowid {
        k.execute("DELETE FROM claims_fts WHERE rowid = ?1", [rowid])?;
    }
    let Some((op_device, op_seq, text)) = active else {
        k.execute("DELETE FROM claims WHERE uid = ?1", [uid])?;
        return Ok(());
    };
    let rowid: i64 = k.query_row(
        "INSERT INTO claims(uid, op_device, op_seq) VALUES(?1, ?2, ?3)
         ON CONFLICT(uid) DO UPDATE SET op_device = excluded.op_device, op_seq = excluded.op_seq
         RETURNING rowid",
        params![uid, op_device, op_seq],
        |r| r.get(0),
    )?;
    k.execute(
        "INSERT INTO claims_fts(rowid, text) VALUES(?1, ?2)",
        params![rowid, text],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::current;
    use crate::raw::{self, Event, Target};
    use crate::worker::{Consumer, drain};
    use serde_json::json;

    fn event(text: &str, ts: i64) -> Event {
        Event {
            ts,
            repo: Some("r".into()),
            body: json!({"prompt": text}).to_string(),
            ..raw::test_event("")
        }
    }

    /// `quote` where it is in `text`, in the sentence that starts at `sentence`.
    fn quote(device: &str, seq: i64, text: &str, quote: &str, sentence: i64) -> Evidence {
        Evidence {
            device: device.into(),
            seq,
            offset: text.find(quote).unwrap() as i64,
            length: quote.len() as i64,
            sentence,
            quote: quote.into(),
            claim_at: None,
        }
    }

    fn claim(id: &str, kind: &str, body: &str, evidence: Vec<Evidence>) -> ClaimOp {
        ClaimOp {
            id: id.into(),
            kind: kind.into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: body.into(),
            evidence,
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
        }
    }

    fn op(c: &ClaimOp) -> (OpKind, serde_json::Value) {
        (OpKind::Claim, serde_json::to_value(c).unwrap())
    }

    fn run(raw: &Raw, k: &mut Connection) {
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Claims), Box::new(Anchors)];
        drain(raw, k, &mut consumers).unwrap();
    }

    fn count(k: &Connection, table: &str) -> i64 {
        k.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// The current bodies, sorted: claims of one event are in uid order (MUST-M7).
    fn bodies(k: &Connection) -> Vec<String> {
        let mut b: Vec<String> = current(k, "r")
            .unwrap()
            .into_iter()
            .map(|c| c.body)
            .collect();
        b.sort();
        b
    }

    /// The home's raw.db under another device id, as a copied home or (milestone 6) sync has
    /// another device's records.
    fn as_device(home: &std::path::Path, id: &str) -> Raw {
        drop(raw::open(home).unwrap());
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("UPDATE meta SET value = ?1 WHERE key = 'device_id'", [id])
            .unwrap();
        raw::open(home).unwrap()
    }

    /// MUST-M18: a claim's uid is its kind and the sentence its quote starts in. A rewording
    /// quoted at another offset of that sentence is a new derivation of the same claim.
    #[test]
    fn a_claim_ops_uid_is_its_kind_and_the_sentence_its_quote_starts_in() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "We indent with tabs from now on. The linter is next.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let first = claim(
            "c1",
            "decision",
            "Tabs.",
            vec![quote(&dev, seq, text, "We indent with tabs", 0)],
        );
        raw.append_ops(&[op(&first)]).unwrap();
        let second = claim(
            "c1",
            "decision",
            "Indent with tabs.",
            vec![quote(&dev, seq, text, "tabs from now on", 0)],
        );
        raw.append_ops(&[op(&second)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let uids: Vec<String> = k
            .prepare("SELECT DISTINCT uid FROM derivations")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(uids, [uid("decision", &first.evidence[0])]);
        assert_eq!((count(&k, "derivations"), count(&k, "claims")), (2, 1));
        assert_eq!(bodies(&k), ["Indent with tabs."]);
        let c = &current(&k, "r").unwrap()[0];
        assert_eq!(
            (c.valid_from, c.device.as_str(), c.seq),
            (5, dev.as_str(), seq)
        );
        // Another sentence is another claim.
        let third = claim(
            "c1",
            "decision",
            "Linter.",
            vec![quote(&dev, seq, text, "The linter is next", 33)],
        );
        raw.append_ops(&[op(&third)]).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Indent with tabs.", "Linter."]);
    }

    /// MUST-M2: a reversal within one window supersedes its sibling by the window-local id.
    #[test]
    fn a_reversal_in_one_append_supersedes_its_sibling() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Use SQLite. No, use Postgres.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let c1 = claim(
            "c1",
            "decision",
            "SQLite.",
            vec![quote(&dev, seq, text, "Use SQLite", 0)],
        );
        let c2 = ClaimOp {
            supersedes: vec!["c1".into()],
            ..claim(
                "c2",
                "decision",
                "Postgres.",
                vec![quote(&dev, seq, text, "No, use Postgres", 12)],
            )
        };
        raw.append_ops(&[op(&c1), op(&c2)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Postgres."]);
        assert_eq!(count(&k, "edges"), 1);
    }

    /// MUST-M7: claims with the same valid_from on two devices come out in one order, whichever
    /// device's ops are read first.
    #[test]
    fn ties_resolve_the_same_way_whatever_order_the_ops_arrive() {
        let home = tempfile::tempdir().unwrap();
        let mut devices = Vec::new();
        for id in ["dev-z", "dev-a"] {
            let mut raw = as_device(home.path(), id);
            let mut ops = Vec::new();
            for i in 0..50 {
                let text = format!("Decision {i} on {id}.");
                let seq = raw.append(&event(&text, 7)).unwrap();
                ops.push(op(&claim(
                    "c",
                    "decision",
                    &text,
                    vec![quote(id, seq, &text, &text, 0)],
                )));
            }
            raw.append_ops(&ops).unwrap();
            devices.push(id);
        }
        let raw = raw::open(home.path()).unwrap();
        // Through the worker's pass: every device with ops, in the order it lists them.
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let passed = current(&k, "r").unwrap();
        assert_eq!(passed.len(), 100);
        // The other order, step by step.
        let other = Connection::open_in_memory().unwrap();
        for id in devices.iter().rev() {
            Claims.step(&raw, &other, id, 0).unwrap();
        }
        assert_eq!(current(&other, "r").unwrap(), passed);
        assert_eq!((passed[0].device.as_str(), passed[0].seq), ("dev-a", 1));
        assert_eq!((passed[99].device.as_str(), passed[99].seq), ("dev-z", 50));
    }

    /// D13: a kind spec 3.2 does not know is a repo fact, unverified; the old kinds map.
    #[test]
    fn an_unknown_kind_is_stored_as_an_unverified_repo_fact() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "The build uses zig. Parsing is slow. Added a cache.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[
            op(&claim(
                "a",
                "gossip",
                "Zig.",
                vec![quote(&dev, seq, text, "The build uses zig", 0)],
            )),
            op(&claim(
                "b",
                "discovery",
                "Slow.",
                vec![quote(&dev, seq, text, "Parsing is slow", 20)],
            )),
            op(&claim(
                "c",
                "feature",
                "Cache.",
                vec![quote(&dev, seq, text, "Added a cache", 37)],
            )),
        ])
        .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let mut got: Vec<(String, String)> = current(&k, "r")
            .unwrap()
            .into_iter()
            .map(|c| (c.kind, c.status))
            .collect();
        got.sort();
        let want = [
            ("change", "decided"),
            ("repo fact", "decided"),
            ("repo fact", "unverified"),
        ];
        assert_eq!(got, want.map(|(a, b)| (a.to_owned(), b.to_owned())));
    }

    /// Part 3b's observation shape, and a claim with no quote, give no claim; each is counted.
    #[test]
    fn an_op_with_no_evidence_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&event("hello", 5)).unwrap();
        raw.append_ops(&[
            (
                OpKind::Claim,
                json!({"kind": "decision", "title": "t", "body": "b"}),
            ),
            op(&claim("c", "decision", "No quote.", Vec::new())),
        ])
        .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!((count(&k, "claims"), count(&k, "claim_skips")), (0, 2));
    }

    /// #144: the tier is in the op, so a rebuild from the ops keeps the paid derivation active
    /// over a later free one.
    #[test]
    fn a_rebuild_keeps_the_higher_tier_derivation_active() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let paid = ClaimOp {
            tier: 3,
            ..claim(
                "c",
                "decision",
                "Paid.",
                vec![quote(&dev, seq, text, "Ship on Fridays", 0)],
            )
        };
        let free = claim(
            "c",
            "decision",
            "Free.",
            vec![quote(&dev, seq, text, "on Fridays", 0)],
        );
        raw.append_ops(&[op(&paid)]).unwrap();
        raw.append_ops(&[op(&free)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Paid."]);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Paid."]);
    }

    /// MUST-M21 (hard), MUST-M18: the owner's correction holds over every derivation of the uid.
    /// A recuration that rewords the claims derives the same uids, newer at the same tier, and the
    /// corrections still apply; each field applies on its own; a rebuild from the op log gives the
    /// same, with no provider (the consumers never call one).
    #[test]
    fn a_recuration_that_rewords_a_claim_keeps_its_uid_and_its_owner_correction() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays. Tabs everywhere.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let at = |q: &str, sentence| vec![quote(&dev, seq, text, q, sentence)];
        let ship = claim(
            "c1",
            "decision",
            "Ship on Fridays.",
            at("Ship on Fridays", 0),
        );
        let tabs = claim(
            "c2",
            "decision",
            "Tabs everywhere.",
            at("Tabs everywhere", 17),
        );
        raw.append_ops(&[op(&ship), op(&tabs)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let uid_of = |k: &Connection, body: &str| {
            current(k, "r")
                .unwrap()
                .into_iter()
                .find(|c| c.body == body)
                .unwrap()
                .uid
        };
        let (ship_uid, tabs_uid) = (
            uid_of(&k, "Ship on Fridays."),
            uid_of(&k, "Tabs everywhere."),
        );
        let correct =
            |uid: &str, status, body| crate::claims::correct(home.path(), uid, status, body);
        correct(&ship_uid, Some("retracted"), None).unwrap();
        correct(&tabs_uid, None, Some("Tabs, never spaces.")).unwrap();
        assert!(correct(&"0".repeat(64), Some("done"), None).is_err());
        run(&raw, &mut k);
        let now = |k: &Connection| {
            current(k, "r")
                .unwrap()
                .into_iter()
                .map(|c| (c.uid, c.status, c.body))
                .collect::<Vec<_>>()
        };
        let corrected = vec![(
            tabs_uid.clone(),
            "decided".into(),
            "Tabs, never spaces.".into(),
        )];
        assert_eq!(now(&k), corrected);
        // The recuration: the same sentences, reworded.
        let ship2 = claim("c1", "decision", "Ship every Friday.", at("on Fridays", 0));
        let tabs2 = claim("c2", "decision", "Use tabs.", at("everywhere", 17));
        raw.append_ops(&[op(&ship2), op(&tabs2)]).unwrap();
        run(&raw, &mut k);
        assert_eq!(now(&k), corrected);
        // A later status correction keeps the body the owner gave.
        correct(&tabs_uid, Some("proposed"), None).unwrap();
        run(&raw, &mut k);
        let proposed = vec![(
            tabs_uid.clone(),
            "proposed".into(),
            "Tabs, never spaces.".into(),
        )];
        assert_eq!(now(&k), proposed);
        drop(k);
        for f in ["knowledge.db", "knowledge.db-wal", "knowledge.db-shm"] {
            let _ = std::fs::remove_file(home.path().join(f));
        }
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(now(&k), proposed);
        assert!(!home.path().join("providers.db").exists());
    }

    /// A correction's body is stored as a typed prompt is: a private block never reaches raw.db.
    #[test]
    fn a_private_block_in_a_correction_is_never_stored() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Tabs everywhere.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let at = vec![quote(&dev, seq, text, "Tabs everywhere", 0)];
        raw.append_ops(&[op(&claim("c1", "decision", text, at))])
            .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let uid = current(&k, "r").unwrap()[0].uid.clone();
        let body = "Tabs, four wide. <private>the ssh pass</private>";
        crate::claims::correct(home.path(), &uid, None, Some(body)).unwrap();
        let ops = raw.ops_after(&dev, 0, 100).unwrap();
        assert!(ops.iter().all(|o| !o.body.to_string().contains("ssh pass")));
        run(&raw, &mut k);
        assert_eq!(current(&k, "r").unwrap()[0].body, "Tabs, four wide.");
    }

    /// A correction synced before its claim is kept and applies when the claim arrives; one that
    /// corrects nothing is skipped with its reason.
    #[test]
    fn a_correction_that_arrives_before_its_claim_applies_when_it_does() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let ship = claim(
            "c1",
            "decision",
            "Ship on Fridays.",
            vec![quote(&dev, seq, text, "Ship", 0)],
        );
        let uid = crate::claims::uid("decision", &ship.evidence[0]);
        let correction = |status: Option<&str>| {
            let op = crate::claims::CorrectionOp {
                uid: uid.clone(),
                anchor: crate::claims::Anchor {
                    device: dev.clone(),
                    seq,
                },
                status: status.map(str::to_owned),
                body: None,
            };
            (OpKind::Correction, serde_json::to_value(op).unwrap())
        };
        raw.append_ops(&[correction(Some("retracted")), correction(None)])
            .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let reason: String = k
            .query_row("SELECT reason FROM claim_skips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "corrects nothing");
        raw.append_ops(&[op(&ship)]).unwrap();
        run(&raw, &mut k);
        assert!(bodies(&k).is_empty());
        assert!(crate::claims::tip(&k, &uid).unwrap().is_none());
        // A restore that lost the correction op takes it back: the claim is current again.
        Claims.rewind(&k, &dev, 0).unwrap();
        assert_eq!(count(&k, "corrections"), 0);
    }

    /// #144: a quote masked since is checked per derivation: the one that quoted it goes, the
    /// claim stays with the other, whose quote after the mask still reads (the mask keeps the
    /// body's length).
    #[test]
    fn a_masked_quote_of_an_inactive_derivation_removes_that_derivation_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Deploy with token hunter2 to staging.";
        let e = event(text, 5);
        let seq = raw.append(&e).unwrap();
        let dev = raw.device().to_owned();
        let active = ClaimOp {
            tier: 3,
            ..claim(
                "c",
                "decision",
                "Deploy to staging.",
                vec![quote(&dev, seq, text, "to staging", 0)],
            )
        };
        let leaky = claim(
            "c",
            "decision",
            "Uses hunter2.",
            vec![quote(&dev, seq, text, "token hunter2", 0)],
        );
        raw.append_ops(&[op(&active)]).unwrap();
        raw.append_ops(&[op(&leaky)]).unwrap();
        let offset = e.body.find("hunter2").unwrap() as i64;
        raw.append_tombstone(Target::Range {
            device: dev.clone(),
            seq,
            offset,
            length: 7,
        })
        .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Deploy to staging."]);
        assert_eq!((count(&k, "derivations"), count(&k, "claim_skips")), (1, 1));
        let found: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM claims_fts WHERE text MATCH 'hunter2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 0);
    }

    /// #144: same-tier derivations from two devices: the newer op is active, not the device
    /// whose id sorts last.
    #[test]
    fn a_newer_same_tier_derivation_from_another_device_becomes_active() {
        let home = tempfile::tempdir().unwrap();
        let text = "Name it oboete.";
        let seq = {
            let mut raw = as_device(home.path(), "dev-a");
            raw.append(&event(text, 5)).unwrap()
        };
        let raw = raw::open(home.path()).unwrap();
        let ev = || vec![quote("dev-a", seq, text, "Name it oboete", 0)];
        let put = |device: &str, ts: i64, body: &str| {
            let c = claim("c", "decision", body, ev());
            rusqlite::Connection::open(home.path().join("raw.db"))
                .unwrap()
                .execute(
                    "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                     VALUES(?1, 1, 'claim', ?2, ?3, 1)",
                    params![device, ts, serde_json::to_string(&c).unwrap()],
                )
                .unwrap();
        };
        put("dev-z", 1_000, "Older.");
        put("dev-a", 2_000, "Newer.");
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Newer."]);
    }

    /// A knowledge.db from before #125 has no `claim_at` on its quotes: the column is added.
    #[test]
    fn an_older_evidence_table_gets_its_claim_at_column() {
        let home = tempfile::tempdir().unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        k.execute_batch(
            "CREATE TABLE evidence(op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
               idx INTEGER NOT NULL, device TEXT NOT NULL, seq INTEGER NOT NULL,
               offset INTEGER NOT NULL, length INTEGER NOT NULL, sentence INTEGER NOT NULL,
               quote TEXT NOT NULL, PRIMARY KEY (op_device, op_seq, idx));",
        )
        .unwrap();
        schema(&k).unwrap();
        schema(&k).unwrap();
        k.execute(
            "INSERT INTO evidence VALUES('d', 1, 0, 'd', 1, 0, 3, 0, 'use', 5)",
            [],
        )
        .unwrap();
    }

    /// A knowledge.db from before #192 keys its queue by the claim op alone: the table is made
    /// again keyed by the op and the first record, with its rows.
    #[test]
    fn the_queue_of_an_older_knowledge_db_is_keyed_again() {
        let home = tempfile::tempdir().unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        k.execute_batch(
            "CREATE TABLE recurate(device TEXT NOT NULL, from_seq INTEGER NOT NULL,
               to_seq INTEGER NOT NULL, op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
               PRIMARY KEY (op_device, op_seq));
             INSERT INTO recurate VALUES('d', 1, 3, 'd', 9);",
        )
        .unwrap();
        schema(&k).unwrap();
        schema(&k).unwrap();
        // A second part of the same op, as a recuration of the middle leaves.
        k.execute("INSERT INTO recurate VALUES('d', 5, 6, 'd', 9)", [])
            .unwrap();
        let rows: Vec<(i64, i64)> = k
            .prepare("SELECT from_seq, to_seq FROM recurate ORDER BY from_seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, [(1, 3), (5, 6)]);
    }

    /// A restore that lost a claim op takes its derivation back out, with its quotes and edges,
    /// and the uid's older derivation is active again.
    #[test]
    fn a_rewind_drops_the_lost_derivations_and_activates_the_one_left() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Use SQLite. No, use Postgres.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let old = claim(
            "c1",
            "decision",
            "SQLite.",
            vec![quote(&dev, seq, text, "Use SQLite", 0)],
        );
        raw.append_ops(&[op(&old)]).unwrap();
        let newer = claim(
            "c1",
            "decision",
            "SQLite, again.",
            vec![quote(&dev, seq, text, "SQLite", 0)],
        );
        let other = ClaimOp {
            supersedes: vec![uid("decision", &old.evidence[0])],
            ..claim(
                "c2",
                "decision",
                "Postgres.",
                vec![quote(&dev, seq, text, "use Postgres", 12)],
            )
        };
        raw.append_ops(&[op(&newer), op(&other)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Postgres."]);
        Claims.rewind(&k, &dev, 1).unwrap();
        assert_eq!(bodies(&k), ["SQLite."]);
        let left = |table: &str| -> i64 {
            k.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE op_seq > 1"),
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            (left("derivations"), left("evidence"), left("edges")),
            (0, 0, 0)
        );
        let found: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM claims_fts WHERE text MATCH 'Postgres'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 0);
    }

    /// Spec 6.4: a rule the rescan applies after curation masks a quoted secret. The claim that
    /// quoted it goes, with its quotes and search row, its window is queued for recuration, and a
    /// rebuild from raw gives the same.
    #[test]
    fn a_rule_added_after_curation_drops_the_claims_quoting_the_masked_text() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Deploy with token hunter2 now. Staging first.";
        let e = event(text, 5);
        let seq = raw.append(&e).unwrap();
        let dev = raw.device().to_owned();
        let window = json!({"from_seq": seq, "from_offset": null, "to_seq": seq,
            "to_offset": null, "outcome": "curated"});
        let leaky = claim(
            "c1",
            "decision",
            "Token.",
            vec![quote(&dev, seq, text, "token hunter2", 0)],
        );
        let safe = claim(
            "c2",
            "decision",
            "Staging first.",
            vec![quote(&dev, seq, text, "Staging first", 31)],
        );
        raw.append_ops(&[(OpKind::Window, window), op(&leaky), op(&safe)])
            .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Staging first.", "Token."]);
        let offset = e.body.find("hunter2").unwrap() as i64;
        raw.append_tombstone(Target::Range {
            device: dev.clone(),
            seq,
            offset,
            length: 7,
        })
        .unwrap();
        run(&raw, &mut k);
        let state = |k: &Connection| {
            let spans: Vec<(String, i64, i64)> = k
                .prepare("SELECT device, from_seq, to_seq FROM recurate")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let found: i64 = k
                .query_row(
                    "SELECT COUNT(*) FROM claims_fts WHERE text MATCH 'hunter2'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            (bodies(k), count(k, "evidence"), found, spans)
        };
        let want = (
            vec!["Staging first.".to_owned()],
            1,
            0,
            vec![(dev.clone(), seq, seq)],
        );
        assert_eq!(state(&k), want);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(state(&k), want);
        // A restore that loses the claim op takes its span back out.
        Claims.rewind(&k, &dev, 1).unwrap();
        assert_eq!(count(&k, "recurate"), 0);
    }
}
