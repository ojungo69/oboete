//! Milestone 3 Task 6: the claims consumer. It reads the op log of every device that has ops and
//! keeps each claim's derivations, quotes and supersedes edges in knowledge.db, with each uid's
//! active derivation (`claims::schema`).

use crate::claims::{ClaimOp, CorrectionOp, Evidence, normalize, schema, uid};
use crate::curate::Span;
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

/// The spans a recuration window `op` covered, out of the recuration queue (Task 11).
fn uncover(k: &Connection, op: &crate::raw::Op) -> Result<()> {
    // What this window curated, and the part of its span curated through it (`covers`,
    // when the span took several): each queued span keeps what is left of it on either
    // side (#192), a record it curated only part of whole, and the records of an excluded
    // session it kept back (Codex on #304).
    let ranges = [&op.body, &op.body["covers"]];
    for c in ranges
        .into_iter()
        .flat_map(|range| crate::curate::curated_parts(&op.body, range))
    {
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
    Ok(())
}

impl Consumer for Claims {
    fn name(&self) -> &'static str {
        "claims"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        retry(raw, k)?;
        let ops = whole_batches(raw, device, after)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        // A span a recuration covered leaves the queue (Task 11), before its claims are derived:
        // a quote of its own that no longer reads queues it again. A curated window a forget took
        // the text of (slice 3d-2) joins it whole, in op order with the recurations: its op is
        // the authority, so a rebuild queues it again. Its Removed rows are in no later window's
        // text and in that window's `removed`, so no part of the span is cut out.
        for op in ops.iter().filter(|o| o.kind == OpKind::Window) {
            if op.body["recurate"] == true {
                uncover(k, op)?;
            }
            if op.body["forgotten"].is_string()
                && op.body["outcome"] == "curated"
                && let Some(span) = crate::curate::op_span(&op.body)
                && !raw.holds_expired(&op.device, span.from, span.to)?
            {
                k.execute(
                    "INSERT INTO recurate(device, from_seq, to_seq, op_device, op_seq)
                     SELECT ?1, ?2, ?3, ?1, ?4
                     WHERE NOT EXISTS (SELECT 1 FROM recurate WHERE op_device = ?1 AND op_seq = ?4)",
                    params![op.device, span.from, span.to, op.op_seq],
                )?;
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
            "support",
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
    // Milestone 5 D5 (2b): one a forget rewrote is passed for good, never read again by `retry`.
    let fault = match serde_json::from_value::<CorrectionOp>(op.body.clone()) {
        _ if op.forgotten() => "forgotten",
        Ok(c) => match c.fault() {
            None => {
                k.execute(
                    "INSERT INTO corrections(op_device, op_seq, ts, uid, status, body, muted)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        op.device, op.op_seq, op.ts, c.uid, c.status, c.body, c.muted
                    ],
                )?;
                // Unmute is visible even when a shortlist was built while the claim was muted.
                if c.muted.is_some() && crate::consumer::manifest::exists(k, "table", "shortlists")?
                {
                    for table in ["shortlist", "shortlists"] {
                        k.execute(
                            &format!(
                                "DELETE FROM {table} WHERE repo IN
                               (SELECT repo FROM active WHERE uid = ?1)"
                            ),
                            [&c.uid],
                        )?;
                    }
                }
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

/// The corrections a consumer passed without keeping them, read again at each step (one query
/// when there is none). A worker of an older oboete passes one it does not understand (a mute,
/// to a consumer that knew none) and moves its checkpoint beyond it: the first step of a consumer
/// that understands it keeps it, with no rewind. One that corrects nothing is only read.
fn retry(raw: &Raw, k: &Connection) -> Result<()> {
    let passed: Vec<(String, i64)> = k
        .prepare(
            "SELECT op_device, op_seq FROM claim_skips
             WHERE reason IN ('corrects nothing', 'not a correction')",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (device, seq) in passed {
        let op = raw.ops_after(&device, seq - 1, 1)?.into_iter().next();
        let Some(op) = op.filter(|o| o.op_seq == seq && o.kind == OpKind::Correction) else {
            continue;
        };
        let kept = serde_json::from_value::<CorrectionOp>(op.body.clone())
            .is_ok_and(|c| c.fault().is_none());
        if !kept {
            continue;
        }
        if let Some(uid) = correction(k, &op)? {
            k.execute(
                "DELETE FROM claim_skips WHERE op_device = ?1 AND op_seq = ?2",
                params![device, seq],
            )?;
            activate(k, &uid)?;
        }
    }
    Ok(())
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
    // Milestone 5 D5 (2b): a forget rewrote it.
    if op.forgotten() {
        return skip("forgotten");
    }
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
        match quote(raw, e)? {
            // docs/retention.md: an expired record's quote stands on the op's kept quote, and its
            // labels give the claim its repository and time.
            Quote::Reads(event) | Quote::Expired(event) if anchor.is_none() => {
                anchor = Some(*event)
            }
            Quote::Reads(_) | Quote::Expired(_) => {}
            Quote::Gone => {
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
    // #167: a status another record raised stands while one of them is left, or the owner set it
    // before they went; else it is a proposal, as the gates would have made it, which supersedes
    // nothing (`Anchors` does the same when the last one goes later).
    let records = || c.support.iter().map(|s| (&s.device, s.seq));
    let stands =
        c.support.is_empty() || supported(raw, records())? || owner_set(raw, &uid, records())?;
    let status = if stands { status } else { "proposed" };
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
    for s in &c.support {
        k.execute(
            "INSERT OR IGNORE INTO support(op_device, op_seq, device, seq) VALUES(?1, ?2, ?3, ?4)",
            params![op.device, op.op_seq, s.device, s.seq],
        )?;
    }
    Ok(Some(Derived {
        op_seq: op.op_seq,
        batch: op.batch,
        id: c.id,
        uid,
        supersedes: if stands { c.supersedes } else { Vec::new() },
    }))
}

/// What a quote's record holds now, as `Raw::after` returns it.
pub(crate) enum Quote {
    /// The event, its quote verbatim there.
    Reads(Box<crate::raw::Event>),
    /// An expired record no tombstone hides (docs/retention.md): its labels. The claim stands on
    /// the quote its op keeps, which no reader shows: the words around it are gone, so a rule that
    /// needs them could not hide it.
    Expired(Box<crate::raw::Event>),
    /// Nothing: a tombstoned record reads as nothing and a masked range as its mask, so a secret
    /// masked after curation does not come back through a claim (spec 6.4).
    Gone,
}

pub(crate) fn quote(raw: &Raw, e: &Evidence) -> Result<Quote> {
    let record = raw
        .after(&e.device, e.seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == e.seq);
    let event = match record.map(|r| r.item) {
        Some(Item::Event(event)) => event,
        // A mask over it cannot be read against its text any more: expiry waits for every mask's
        // purge, which rewrites the claim ops of a masked record's window first (docs/retention.md,
        // "Masks come first").
        Some(Item::Expired(labels)) => return Ok(Quote::Expired(labels)),
        _ => return Ok(Quote::Gone),
    };
    let (Ok(start), Ok(len)) = (usize::try_from(e.offset), usize::try_from(e.length)) else {
        return Ok(Quote::Gone);
    };
    let reads = crate::curate::long_text(&event)
        .and_then(|long| {
            long.get(start..start.checked_add(len)?)
                .map(|q| q == e.quote)
        })
        .unwrap_or(false);
    Ok(if reads {
        Quote::Reads(event)
    } else {
        Quote::Gone
    })
}

/// The event `e` anchors on, when its quote still reads verbatim there (`Quote::Reads`).
pub(crate) fn live(raw: &Raw, e: &Evidence) -> Result<Option<crate::raw::Event>> {
    Ok(match quote(raw, e)? {
        Quote::Reads(event) => Some(*event),
        _ => None,
    })
}

/// Whether one of `records` is still there, a mask over part of it or not (#167): a status stands
/// on what a record was (an acceptance, a passing run), which a masked token does not change; a
/// record removed whole reads as `Removed` or not at all.
pub(crate) fn supported<'a>(
    raw: &Raw,
    records: impl IntoIterator<Item = (&'a String, i64)>,
) -> Result<bool> {
    for (device, seq) in records {
        let record = raw
            .after(device, seq - 1, 1)?
            .into_iter()
            .find(|r| r.seq == seq);
        // docs/retention.md: an expired record is still the record the status stood on.
        if record.is_some_and(|r| matches!(r.item, Item::Event(_) | Item::Expired(_))) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// #167 (Codex on #473): whether the owner set `uid`'s status (a correction `correction` applies,
/// which `active` shows over any derivation's) before the last of `records`, those its
/// derivation's status stood on, went: then it stands, and supersedes what it did. In the same
/// millisecond counts as before: the clock cannot order them, and the owner's word stands (Codex
/// on #473). One set after them, or with one of them never here, does not raise it again, in the
/// live consumer and in a rebuild alike.
pub(crate) fn owner_set<'a>(
    raw: &Raw,
    uid: &str,
    records: impl IntoIterator<Item = (&'a String, i64)>,
) -> Result<bool> {
    let Some(gone) = raw.gone_at(records)? else {
        return Ok(false);
    };
    Ok(raw.corrections_naming(uid)?.into_iter().any(|(ts, body)| {
        ts <= gone
            && serde_json::from_value::<crate::claims::CorrectionOp>(body)
                .is_ok_and(|c| c.uid == uid && c.status.is_some() && c.fault().is_none())
    }))
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
    // docs/retention.md: a window with an expired record cannot be curated again.
    if raw.holds_expired(device, from, to)? {
        return Ok(());
    }
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
                    if matches!(quote(raw, e)?, Quote::Gone) {
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
                for table in ["derivations", "evidence", "edges", "support"] {
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
            // #167: a derivation whose status stood on the record, with none of its records left,
            // is a proposal, which supersedes nothing, as `derive` makes it in a rebuild.
            let supported_by: Vec<(String, i64)> = k
                .prepare(
                    "SELECT DISTINCT op_device, op_seq FROM support WHERE device = ?1 AND seq = ?2",
                )?
                .query_map(params![target, seq], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (op_device, op_seq) in supported_by {
                let records: Vec<(String, i64)> = k
                    .prepare(
                        "SELECT device, seq FROM support WHERE op_device = ?1 AND op_seq = ?2",
                    )?
                    .query_map(params![op_device, op_seq], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                let uid: String = k.query_row(
                    "SELECT uid FROM derivations WHERE op_device = ?1 AND op_seq = ?2",
                    params![op_device, op_seq],
                    |r| r.get(0),
                )?;
                let records = || records.iter().map(|(d, s)| (d, *s));
                if supported(raw, records())? || owner_set(raw, &uid, records())? {
                    continue;
                }
                k.execute(
                    "UPDATE derivations SET status = 'proposed' WHERE op_device = ?1 AND op_seq = ?2",
                    params![op_device, op_seq],
                )?;
                k.execute(
                    "DELETE FROM edges WHERE op_device = ?1 AND op_seq = ?2",
                    params![op_device, op_seq],
                )?;
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
                              AND x.body IS NOT NULL AND NOT EXISTS (SELECT 1 FROM corrections y
                                WHERE y.uid = x.uid AND y.op_device = x.op_device
                                  AND y.body IS NOT NULL AND y.op_seq > x.op_seq)
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
        return crate::embed_phase::touched(None, k, "c", uid);
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
    crate::embed_phase::touched(None, k, "c", uid)
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
            support: Vec::new(),
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

    /// #303: a reader waits only for tombstones the worker will apply. The anchors consumer steps
    /// this device's records, so a copied home's old id's mask is never pending for it: a claim
    /// quoting the rest of that record is shown, not hidden for good.
    #[test]
    fn a_mask_under_a_copied_homes_old_id_never_hides_a_claim() {
        let home = tempfile::tempdir().unwrap();
        let text = "Deploy to staging with token hunter2.";
        let mut raw = as_device(home.path(), "dev-a");
        let seq = raw.append(&event(text, 1)).unwrap();
        let c = claim(
            "c",
            "decision",
            "Deploy to staging.",
            vec![quote("dev-a", seq, text, "Deploy to staging", 0)],
        );
        raw.append_ops(&[op(&c)]).unwrap();
        raw.append_tombstone(Target::Range {
            device: "dev-a".into(),
            seq,
            offset: text.find("hunter2").unwrap() as i64,
            length: 7,
        })
        .unwrap();
        let raw = as_device(home.path(), "dev-b");
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Deploy to staging."]);
        let pending = crate::claims::Pending::read(&raw, &k).unwrap();
        let uid = crate::claims::uid("decision", &c.evidence[0]);
        assert!(!pending.touches(&k, &uid).unwrap());
    }

    /// docs/retention.md: a claim stands on the quote of a record whose text expired, through a
    /// rebuild (its repository and time from the record's labels, its status from its support),
    /// and through a mask of its other quote's record outside that quote; a forget of the
    /// expired record takes it.
    #[test]
    fn a_claim_on_an_expired_record_stands_until_a_forget_takes_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        let old = "Use tabs in the Go files.";
        let new = "Deploy with token hunter2 from the CI.";
        let a = raw.append(&event(old, 5)).unwrap();
        let b = raw.append(&event(new, 6)).unwrap();
        let mut c = claim(
            "c",
            "decision",
            "Use tabs; deploy from the CI.",
            vec![
                quote(&device, a, old, "Use tabs", 0),
                quote(&device, b, new, "Deploy with", 0),
            ],
        );
        c.support = vec![crate::claims::Support {
            device: device.clone(),
            seq: a,
        }];
        raw.append_ops(&[op(&c)]).unwrap();
        raw.expire_for_test(&[a]);
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Use tabs; deploy from the CI."]);
        let (repo, from, status): (String, i64, String) = k
            .query_row(
                "SELECT repo, valid_from, status FROM derivations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((repo.as_str(), from, status.as_str()), ("r", 5, "decided"));
        // A range is of the stored body.
        let body = event(new, 6).body;
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: b,
            offset: body.find("hunter2").unwrap() as i64,
            length: 7,
        })
        .unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Use tabs; deploy from the CI."]);
        assert_eq!(count(&k, "recurate"), 0);
        raw.append_tombstone(Target::Record {
            device: device.clone(),
            seq: a,
        })
        .unwrap();
        run(&raw, &mut k);
        assert!(bodies(&k).is_empty());
        // The window of a record that expired is never queued: it cannot be curated again.
        assert_eq!(count(&k, "recurate"), 0);
        // Nor is a curated window a forget took the text of, while it holds an expired record;
        // one that holds none is.
        let forgotten = |from: i64, to: i64| {
            (
                raw::OpKind::Window,
                json!({"from_seq": from, "to_seq": to, "outcome": "curated", "forgotten": "j"}),
            )
        };
        raw.append_ops(&[forgotten(a, b)]).unwrap();
        run(&raw, &mut k);
        assert_eq!(count(&k, "recurate"), 0);
        raw.append_ops(&[forgotten(b, b)]).unwrap();
        run(&raw, &mut k);
        assert_eq!(count(&k, "recurate"), 1);
    }

    /// #249: the claims a window takes as its own are those quoted in its records on its device:
    /// another device's claim quoted in a record of the same seq still supersedes for it.
    #[test]
    fn another_devices_claim_at_the_same_seq_is_not_the_windows_own() {
        let home = tempfile::tempdir().unwrap();
        let over = "Move sessions to SQLite.";
        let seq = as_device(home.path(), "dev-a")
            .append(&event(over, 1))
            .unwrap();
        let mut raw = as_device(home.path(), "dev-b");
        let kept = "Keep sessions in Postgres.";
        assert_eq!(raw.append(&event(kept, 2)).unwrap(), seq);
        let postgres = claim(
            "c1",
            "decision",
            kept,
            vec![quote("dev-b", seq, kept, kept, 0)],
        );
        let mut sqlite = claim(
            "c2",
            "decision",
            over,
            vec![quote("dev-a", seq, over, over, 0)],
        );
        sqlite.supersedes = vec![uid("decision", &postgres.evidence[0])];
        raw.append_ops(&[op(&postgres), op(&sqlite)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let rules = crate::redact::Rules::default();
        let w = crate::curate::next_window(&raw, "dev-b", 100_000, &rules)
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.device.as_str(), w.from_seq, w.to_seq),
            ("dev-b", seq, seq)
        );
        let current: Vec<String> = crate::claims::current_before(&k, "r", &w)
            .unwrap()
            .into_iter()
            .map(|c| c.body)
            .collect();
        assert_eq!(current, [over]);
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
        // The other order, step by step, into another home's knowledge.db.
        let elsewhere = tempfile::tempdir().unwrap();
        let other = crate::knowledge::open(elsewhere.path()).unwrap();
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

    #[test]
    fn the_newest_mute_wins_by_time_device_and_sequence() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let ship = claim("c", "decision", text, vec![quote(&dev, seq, text, text, 0)]);
        let uid = crate::claims::uid("decision", &ship.evidence[0]);
        raw.append_ops(&[op(&ship)]).unwrap();
        let mute = |muted| {
            (
                OpKind::Correction,
                json!({
                    "uid": uid, "anchor": {"device": dev, "seq": seq}, "muted": muted
                }),
            )
        };
        let mut z = as_device(home.path(), "dev-z");
        z.append_ops(&[mute(true)]).unwrap();
        let mut a = as_device(home.path(), "dev-a");
        a.append_ops(&[mute(false), mute(false), mute(false)])
            .unwrap();
        let clock = Connection::open(home.path().join("raw.db")).unwrap();
        clock
            .execute("UPDATE ops SET ts = 20 WHERE type = 'correction'", [])
            .unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let state = |k: &Connection| {
            k.query_row(
                "SELECT muted, status, body FROM active WHERE uid = ?1",
                [&uid],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(state(&k), (1, "decided".into(), "Ship on Fridays.".into()));
        z.append_ops(&[mute(false)]).unwrap();
        clock
            .execute("UPDATE ops SET ts = 20 WHERE type = 'correction'", [])
            .unwrap();
        run(&raw, &mut k);
        assert_eq!(state(&k), (0, "decided".into(), "Ship on Fridays.".into()));
        z.append_ops(&[mute(true)]).unwrap();
        clock
            .execute(
                "UPDATE ops SET ts = 19 WHERE device = 'dev-z' AND op_seq = 3",
                [],
            )
            .unwrap();
        run(&raw, &mut k);
        assert_eq!(state(&k), (0, "decided".into(), "Ship on Fridays.".into()));
    }

    /// A worker of an older oboete can still hold the lock while a newer command mutes: it knows
    /// no `muted`, passes the op as one that corrects nothing and moves its checkpoint beyond it.
    /// The next run of this consumer keeps it, with no rewind (Codex on #358).
    #[test]
    fn a_correction_an_older_consumer_passed_is_kept_by_the_next_run() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let ship = claim("c", "decision", text, vec![quote(&dev, seq, text, text, 0)]);
        let uid = crate::claims::uid("decision", &ship.evidence[0]);
        raw.append_ops(&[op(&ship)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        // What the older consumer left of a mute, and of an op that does correct nothing.
        let correction = |body: serde_json::Value| (OpKind::Correction, body);
        let anchor = json!({"device": dev, "seq": seq});
        let passed = raw
            .append_ops(&[
                correction(json!({"uid": uid, "anchor": anchor, "muted": true})),
                correction(json!({"uid": uid, "anchor": anchor})),
            ])
            .unwrap();
        for at in &passed {
            k.execute(
                "INSERT INTO claim_skips(op_device, op_seq, reason)
                 VALUES(?1, ?2, 'corrects nothing')",
                params![dev, at],
            )
            .unwrap();
        }
        use crate::knowledge::checkpoint;
        checkpoint::set_in(&k, checkpoint::OPS, "claims", &dev, passed[1]).unwrap();
        run(&raw, &mut k);
        let muted: bool = k
            .query_row("SELECT muted FROM active WHERE uid = ?1", [&uid], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(muted, "the mute an older consumer passed was not kept");
        let skipped: Vec<i64> = k
            .prepare("SELECT op_seq FROM claim_skips ORDER BY op_seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            skipped,
            [passed[1]],
            "only the empty correction stays skipped"
        );
    }

    /// #162: of one device's corrections, the later in its op log wins even when its clock went
    /// back between them (an NTP step, a resumed VM); between devices, the newer time wins.
    #[test]
    fn a_later_correction_with_an_earlier_clock_still_wins() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let anchor = crate::claims::active_one(&k, &uid).unwrap().unwrap();
        s.correct(&uid, Some("retracted"), Some("Ship never."));
        s.correct(&uid, Some("done"), Some("Ship on Fridays only."));
        // The clock stepped back between the two: the first is stamped an hour after the second.
        let raw = rusqlite::Connection::open(s.home.path().join("raw.db")).unwrap();
        let first: i64 = raw
            .query_row(
                "SELECT MIN(op_seq) FROM ops WHERE type = 'correction'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        raw.execute(
            "UPDATE ops SET ts = ts + 3600000 WHERE type = 'correction' AND op_seq = ?1",
            [first],
        )
        .unwrap();
        s.run();
        let state = |k: &rusqlite::Connection| {
            k.query_row(
                "SELECT status, body FROM active WHERE uid = ?1",
                [&uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap()
        };
        assert_eq!(state(&k), ("done".into(), "Ship on Fridays only.".into()));
        // The search row carries the same body.
        let matches = |q: &str| -> i64 {
            k.query_row(
                "SELECT count(*) FROM claims_fts f JOIN claims c ON c.rowid = f.rowid
                 WHERE c.uid = ?1 AND claims_fts MATCH ?2",
                params![uid, q],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            matches("\"Fridays only\""),
            1,
            "the search row has the later body"
        );
        assert_eq!(matches("never"), 0, "nor the earlier one");
        // Another device's correction, stamped after both, wins: between devices, time orders.
        let op = json!({
            "uid": uid, "anchor": {"device": anchor.device, "seq": anchor.seq}, "status": "decided"
        });
        raw.execute(
            "INSERT INTO ops(device, op_seq, type, ts, body, batch)
             VALUES('dev-z', 1, 'correction', ?1, ?2, 1)",
            params![crate::db::now_ms() + 7_200_000, op.to_string()],
        )
        .unwrap();
        s.run();
        assert_eq!(state(&k).0, "decided");
    }

    /// #162: a knowledge.db whose `active` view still orders a device's corrections by its clock
    /// gets the view that orders them by the op log.
    #[test]
    fn an_active_view_that_orders_by_the_clock_is_replaced() {
        let mut s = crate::search::b::fixture::Store::new();
        s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        k.execute_batch(
            "DROP VIEW active;
             CREATE VIEW active AS
               SELECT c.uid, d.kind, d.speaker, d.scope, d.repo, d.valid_from,
                 d.anchor_device, d.anchor_seq,
                 COALESCE((SELECT x.status FROM corrections x WHERE x.uid = c.uid
                           AND x.status IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.status) AS status,
                 COALESCE((SELECT x.body FROM corrections x WHERE x.uid = c.uid
                           AND x.body IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.body) AS body,
                 COALESCE((SELECT x.muted FROM corrections x WHERE x.uid = c.uid
                           AND x.muted IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          0) AS muted
               FROM claims c JOIN derivations d
                 ON d.op_device = c.op_device AND d.op_seq = c.op_seq;",
        )
        .unwrap();
        crate::claims::schema(&k).unwrap();
        let sql: String = k
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'view' AND name = 'active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sql.contains("y.op_seq > x.op_seq"), "{sql}");
    }

    #[test]
    fn an_old_corrections_table_and_active_view_are_upgraded() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        s.correct(&uid, Some("done"), Some("Ship only on Fridays."));
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        k.execute_batch(
            "DROP VIEW active;
             ALTER TABLE corrections RENAME TO corrections_new;
             CREATE TABLE corrections(
               op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, ts INTEGER NOT NULL,
               uid TEXT NOT NULL, status TEXT, body TEXT, PRIMARY KEY (op_device, op_seq));
             INSERT INTO corrections SELECT op_device, op_seq, ts, uid, status, body
               FROM corrections_new;
             DROP TABLE corrections_new;
             CREATE VIEW active AS
               SELECT c.uid, d.kind, d.speaker, d.scope, d.repo, d.valid_from,
                 d.anchor_device, d.anchor_seq,
                 COALESCE((SELECT x.status FROM corrections x WHERE x.uid = c.uid
                           AND x.status IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.status) AS status,
                 COALESCE((SELECT x.body FROM corrections x WHERE x.uid = c.uid
                           AND x.body IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.body) AS body
               FROM claims c JOIN derivations d
                 ON d.op_device = c.op_device AND d.op_seq = c.op_seq;",
        )
        .unwrap();
        crate::claims::correct(s.home.path(), &uid, None, Some("Ship every Friday.")).unwrap();
        let state = k
            .query_row(
                "SELECT muted, status, body FROM active WHERE uid = ?1",
                [&uid],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state, (0, "done".into(), "Ship every Friday.".into()));
        s.raw
            .append_ops(&[(
                OpKind::Correction,
                json!({
                    "uid": uid, "anchor": {"device": s.raw.device(), "seq": 1}, "muted": true
                }),
            )])
            .unwrap();
        s.run();
        assert_eq!(
            k.query_row("SELECT muted FROM active WHERE uid = ?1", [&uid], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn mute_survives_recuration_rebuild_and_other_owner_corrections() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let text = "Ship on Fridays.";
        let seq = raw.append(&event(text, 5)).unwrap();
        let dev = raw.device().to_owned();
        let ship = claim("c", "decision", text, vec![quote(&dev, seq, text, text, 0)]);
        raw.append_ops(&[op(&ship)]).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        let uid = crate::claims::uid("decision", &ship.evidence[0]);
        let state = || {
            let k = crate::knowledge::open(home.path()).unwrap();
            k.query_row(
                "SELECT muted, status, body FROM active WHERE uid = ?1",
                [&uid],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        for _ in 0..2 {
            crate::claims::mute(home.path(), &uid, true).unwrap();
            assert_eq!(state(), (1, "decided".into(), "Ship on Fridays.".into()));
        }
        let before = raw.max_op_seq().unwrap();
        let err = crate::claims::mute(home.path(), &"0".repeat(64), true).unwrap_err();
        assert!(err.to_string().starts_with("no claim has the uid "));
        assert_eq!(raw.max_op_seq().unwrap(), before);
        let again = claim(
            "c",
            "decision",
            "Ship every Friday.",
            vec![quote(&dev, seq, text, "on Fridays", 0)],
        );
        raw.append_ops(&[
            (
                OpKind::Window,
                json!({
                    "from_seq": seq, "to_seq": seq, "recurate": true, "outcome": "curated"
                }),
            ),
            op(&again),
        ])
        .unwrap();
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(state(), (1, "decided".into(), "Ship every Friday.".into()));
        crate::claims::correct(home.path(), &uid, Some("proposed"), None).unwrap();
        crate::claims::correct(home.path(), &uid, None, Some("Ship when ready.")).unwrap();
        assert_eq!(state(), (1, "proposed".into(), "Ship when ready.".into()));
        drop(raw);
        crate::worker::rebuild(home.path()).unwrap();
        assert_eq!(state(), (1, "proposed".into(), "Ship when ready.".into()));
        for _ in 0..2 {
            crate::claims::mute(home.path(), &uid, false).unwrap();
            assert_eq!(state(), (0, "proposed".into(), "Ship when ready.".into()));
        }
        crate::worker::rebuild(home.path()).unwrap();
        assert_eq!(state(), (0, "proposed".into(), "Ship when ready.".into()));
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
                muted: None,
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
        // Nor a tip to a later window, whose own claim it is not.
        let window = json!({"from_seq": 1, "to_seq": seq, "outcome": "curated"});
        raw.append_ops(&[(OpKind::Window, window)]).unwrap();
        raw.append(&event("Later.", 6)).unwrap();
        let rules = crate::redact::Rules::default();
        let w = crate::curate::next_window(&raw, &dev, 100_000, &rules)
            .unwrap()
            .unwrap();
        assert!(w.from_seq > seq);
        assert!(crate::claims::tip(&k, &uid, &w).unwrap().is_none());
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

    /// #167: a derivation whose status stood on records besides its quotes is a proposal once none
    /// of them is left, in the live consumer and after a rebuild, and then supersedes nothing: the
    /// claim it superseded is current again. The claim stays. With one of two records left, or a
    /// record masked in part, it stands; an owner correction's status is kept over it.
    #[test]
    fn a_status_whose_records_are_removed_is_a_proposal_again() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let at = |raw: &mut Raw, text: &str, ts| raw.append(&event(text, ts)).unwrap();
        let (old, proposal, done) = ("We store it in JSON files.", "Let's use SQLite.", "Fixed.");
        let s_old = at(&mut raw, old, 1);
        let s_proposal = at(&mut raw, proposal, 2);
        let s_yes = at(&mut raw, "Yes, go ahead.", 3);
        let run1 = at(&mut raw, "test result: ok. 4 passed", 4);
        let run2 = at(&mut raw, "test result: ok. 5 passed", 5);
        let s_done = at(&mut raw, done, 6);
        let support = |seqs: &[i64]| -> Vec<crate::claims::Support> {
            seqs.iter()
                .map(|&seq| crate::claims::Support {
                    device: dev.clone(),
                    seq,
                })
                .collect()
        };
        let earlier = claim("x", "decision", old, vec![quote(&dev, s_old, old, old, 0)]);
        let x = uid("decision", &earlier.evidence[0]);
        let accepted = ClaimOp {
            speaker: "assistant proposal".into(),
            supersedes: vec![x.clone()],
            support: support(&[s_yes]),
            ..claim(
                "a",
                "decision",
                "Use SQLite.",
                vec![quote(&dev, s_proposal, proposal, "use SQLite", 0)],
            )
        };
        let fixed = ClaimOp {
            status: "done".into(),
            support: support(&[run1, run2]),
            ..claim(
                "d",
                "fix",
                done,
                vec![quote(&dev, s_done, done, "Fixed", 0)],
            )
        };
        let a = uid("decision", &accepted.evidence[0]);
        let d = uid("fix", &fixed.evidence[0]);
        raw.append_ops(&[op(&earlier)]).unwrap();
        let ops = raw.append_ops(&[op(&accepted), op(&fixed)]).unwrap();
        let statuses = |k: &Connection| -> Vec<(String, String)> {
            let mut got: Vec<(String, String)> = current(k, "r")
                .unwrap()
                .into_iter()
                .map(|c| (c.body, c.status))
                .collect();
            got.sort();
            got
        };
        let pair = |b: &str, st: &str| (b.to_owned(), st.to_owned());
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        let standing = [pair(done, "done"), pair("Use SQLite.", "decided")];
        assert_eq!(statuses(&k), standing);
        let pending = |raw: &Raw, k: &Connection| {
            let p = crate::claims::Pending::read(raw, k).unwrap();
            [
                p.touches(k, &a).unwrap(),
                p.touches(k, &d).unwrap(),
                p.touches_op(k, &dev, ops[0]).unwrap(),
                p.touches_op(k, &dev, ops[1]).unwrap(),
            ]
        };
        // One run of two removed, and the acceptance masked in part: both stand, and a reader
        // keeps them while Anchors has not reached the tombstones (Codex on #473).
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: run1,
        })
        .unwrap();
        raw.append_tombstone(Target::Range {
            device: dev.clone(),
            seq: s_yes,
            offset: 0,
            length: 3,
        })
        .unwrap();
        assert_eq!(pending(&raw, &k), [false; 4]);
        run(&raw, &mut k);
        assert_eq!(statuses(&k), standing);
        // The acceptance and the other run removed: proposals, and the decision the proposal
        // superseded is current again.
        assert_eq!(pending(&raw, &k), [false; 4]);
        for seq in [s_yes, run2] {
            raw.append_tombstone(Target::Record {
                device: dev.clone(),
                seq,
            })
            .unwrap();
        }
        // Until Anchors lowers them, a reader leaves them out (Codex on #473).
        assert_eq!(pending(&raw, &k), [true; 4]);
        run(&raw, &mut k);
        let lowered = [
            pair(done, "proposed"),
            pair("Use SQLite.", "proposed"),
            pair(old, "decided"),
        ];
        assert_eq!(statuses(&k), lowered);
        assert_eq!(count(&k, "edges"), 0);
        // A record tombstoned again after Anchors lowered them: nothing left to wait for (Codex on
        // #473).
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: s_yes,
        })
        .unwrap();
        assert_eq!(pending(&raw, &k), [false; 4]);
        run(&raw, &mut k);
        assert_eq!(statuses(&k), lowered);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(statuses(&k), lowered);
        assert_eq!(count(&k, "edges"), 0);
        // An op that comes after its record's removal was read: a proposal from the start, which
        // supersedes nothing.
        let postgres = "Let's use Postgres.";
        let s_late = at(&mut raw, postgres, 7);
        let late = ClaimOp {
            speaker: "assistant proposal".into(),
            supersedes: vec![x.clone()],
            support: support(&[s_yes]),
            ..claim(
                "l",
                "decision",
                "Use Postgres.",
                vec![quote(&dev, s_late, postgres, "use Postgres", 0)],
            )
        };
        raw.append_ops(&[op(&late)]).unwrap();
        run(&raw, &mut k);
        let mut later = lowered.to_vec();
        later.push(pair("Use Postgres.", "proposed"));
        later.sort();
        assert_eq!(statuses(&k), later);
        assert_eq!(count(&k, "edges"), 0);
        // The owner's correction is kept over it; written after the record went, it does not raise
        // the derivation again, which supersedes nothing, in a rebuild either (Codex on #473).
        let correction =
            json!({"uid": a, "anchor": {"device": dev, "seq": s_proposal}, "status": "decided"});
        raw.append_ops(&[(OpKind::Correction, correction)]).unwrap();
        run(&raw, &mut k);
        assert!(statuses(&k).contains(&pair("Use SQLite.", "decided")));
        assert_eq!(count(&k, "edges"), 0);
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert!(statuses(&k).contains(&pair("Use SQLite.", "decided")));
        assert_eq!(count(&k, "edges"), 0);
    }

    /// #167 (Codex on #473): a status the owner set before the record it stood on went stands over
    /// its removal, and still supersedes, in the live consumer and after a rebuild; a reader keeps
    /// the claim while Anchors has not reached the tombstone, as the status it shows cannot change.
    #[test]
    fn a_status_the_owner_set_before_its_record_went_stands() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let at = |raw: &mut Raw, text: &str, ts| raw.append(&event(text, ts)).unwrap();
        let (old, proposal) = ("We store it in JSON files.", "Let's use SQLite.");
        let s_old = at(&mut raw, old, 1);
        let s_proposal = at(&mut raw, proposal, 2);
        let s_yes = at(&mut raw, "Yes, go ahead.", 3);
        let earlier = claim("x", "decision", old, vec![quote(&dev, s_old, old, old, 0)]);
        let accepted = ClaimOp {
            speaker: "assistant proposal".into(),
            supersedes: vec![uid("decision", &earlier.evidence[0])],
            support: vec![crate::claims::Support {
                device: dev.clone(),
                seq: s_yes,
            }],
            ..claim(
                "a",
                "decision",
                "Use SQLite.",
                vec![quote(&dev, s_proposal, proposal, "use SQLite", 0)],
            )
        };
        let a = uid("decision", &accepted.evidence[0]);
        raw.append_ops(&[op(&earlier)]).unwrap();
        raw.append_ops(&[op(&accepted)]).unwrap();
        let correction =
            json!({"uid": a, "anchor": {"device": dev, "seq": s_proposal}, "status": "decided"});
        raw.append_ops(&[(OpKind::Correction, correction)]).unwrap();
        // Written a minute before the record went.
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE ops SET ts = ts - 60000 WHERE type = 'correction'",
                [],
            )
            .unwrap();
        let bodies = |k: &Connection| -> Vec<String> {
            current(k, "r")
                .unwrap()
                .into_iter()
                .map(|c| c.body)
                .collect()
        };
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(bodies(&k), ["Use SQLite."]);
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: s_yes,
        })
        .unwrap();
        let pending = crate::claims::Pending::read(&raw, &k).unwrap();
        assert!(!pending.touches(&k, &a).unwrap());
        run(&raw, &mut k);
        assert_eq!(
            (bodies(&k), count(&k, "edges")),
            (vec!["Use SQLite.".to_owned()], 1)
        );
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(
            (bodies(&k), count(&k, "edges")),
            (vec!["Use SQLite.".to_owned()], 1)
        );
        // A tombstone a restore from segments brought back without its time: it went as late as
        // can be, so the owner's status still came first (Codex on #473).
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("UPDATE records SET ts = 0 WHERE type = 'tombstone'", [])
            .unwrap();
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(
            (bodies(&k), count(&k, "edges")),
            (vec!["Use SQLite.".to_owned()], 1)
        );
        // A correction in the same millisecond as the removal: the clock cannot order them, and
        // the owner's status stands (Codex on #473).
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute_batch(
                "UPDATE records SET ts = 5000 WHERE type = 'tombstone';
                 UPDATE ops SET ts = 5000 WHERE type = 'correction';",
            )
            .unwrap();
        drop(k);
        std::fs::remove_file(home.path().join("knowledge.db")).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        assert_eq!(
            (bodies(&k), count(&k, "edges")),
            (vec!["Use SQLite.".to_owned()], 1)
        );
    }

    /// #167 (Codex on #473): a status the owner set after the record it stood on went does not keep
    /// a reader from leaving the claim out while Anchors has not reached the tombstone, though the
    /// claims consumer applied it: Anchors lowers the derivation and takes its supersedes edge.
    #[test]
    fn a_status_the_owner_set_after_its_record_went_waits_for_anchors() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let at = |raw: &mut Raw, text: &str, ts| raw.append(&event(text, ts)).unwrap();
        let (old, proposal) = ("We store it in JSON files.", "Let's use SQLite.");
        let s_old = at(&mut raw, old, 1);
        let s_proposal = at(&mut raw, proposal, 2);
        let s_yes = at(&mut raw, "Yes, go ahead.", 3);
        let earlier = claim("x", "decision", old, vec![quote(&dev, s_old, old, old, 0)]);
        let accepted = ClaimOp {
            speaker: "assistant proposal".into(),
            supersedes: vec![uid("decision", &earlier.evidence[0])],
            support: vec![crate::claims::Support {
                device: dev.clone(),
                seq: s_yes,
            }],
            ..claim(
                "a",
                "decision",
                "Use SQLite.",
                vec![quote(&dev, s_proposal, proposal, "use SQLite", 0)],
            )
        };
        let a = uid("decision", &accepted.evidence[0]);
        raw.append_ops(&[op(&earlier)]).unwrap();
        raw.append_ops(&[op(&accepted)]).unwrap();
        let mut k = crate::knowledge::open(home.path()).unwrap();
        run(&raw, &mut k);
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: s_yes,
        })
        .unwrap();
        // The record went a minute before the owner set the status.
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET ts = ts - 60000 WHERE type = 'tombstone'",
                [],
            )
            .unwrap();
        let correction =
            json!({"uid": a, "anchor": {"device": dev, "seq": s_proposal}, "status": "decided"});
        raw.append_ops(&[(OpKind::Correction, correction)]).unwrap();
        let mut claims: Vec<Box<dyn Consumer>> = vec![Box::new(Claims)];
        drain(&raw, &mut k, &mut claims).unwrap();
        let pending = crate::claims::Pending::read(&raw, &k).unwrap();
        assert!(pending.touches(&k, &a).unwrap());
        run(&raw, &mut k);
        assert_eq!(count(&k, "edges"), 0);
        assert!(bodies(&k).contains(&"Use SQLite.".to_owned()));
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

    /// A worker and a command that open the same older knowledge.db at once both migrate it
    /// (#213): the second checks while the first holds the write lock, as a step's transaction
    /// does, and finds the change made once it gets the lock. One migration at a time, so the
    /// second meets each while the first still holds the lock.
    #[test]
    fn two_connections_migrate_one_older_knowledge_db_at_once() {
        let older_evidence = "DROP TABLE evidence;
             CREATE TABLE evidence(op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
               idx INTEGER NOT NULL, device TEXT NOT NULL, seq INTEGER NOT NULL,
               offset INTEGER NOT NULL, length INTEGER NOT NULL, sentence INTEGER NOT NULL,
               quote TEXT NOT NULL, PRIMARY KEY (op_device, op_seq, idx));
             CREATE INDEX evidence_anchor ON evidence(device, seq);
             INSERT INTO evidence VALUES('d', 1, 0, 'd', 1, 0, 3, 0, 'use');
             INSERT INTO recurate VALUES('d', 1, 3, 'd', 9);";
        let older_queue = "INSERT INTO evidence VALUES('d', 1, 0, 'd', 1, 0, 3, 0, 'use', NULL);
             DROP TABLE recurate;
             CREATE TABLE recurate(device TEXT NOT NULL, from_seq INTEGER NOT NULL,
               to_seq INTEGER NOT NULL, op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
               PRIMARY KEY (op_device, op_seq));
             INSERT INTO recurate VALUES('d', 1, 3, 'd', 9);";
        for older in [older_evidence, older_queue] {
            let home = tempfile::tempdir().unwrap();
            let first = crate::knowledge::open(home.path()).unwrap();
            schema(&first).unwrap();
            first.execute_batch(older).unwrap();
            let second = crate::knowledge::open(home.path()).unwrap();
            first.execute_batch("BEGIN IMMEDIATE").unwrap();
            let other = std::thread::spawn(move || schema(&second).map_err(|e| e.to_string()));
            std::thread::sleep(std::time::Duration::from_millis(300));
            schema(&first).unwrap();
            first.execute_batch("COMMIT").unwrap();
            other.join().unwrap().unwrap();
            let now: (i64, i64, i64, i64) = first
                .query_row(
                    "SELECT (SELECT count(*) FROM pragma_table_info('evidence')
                             WHERE name = 'claim_at'),
                       (SELECT count(*) FROM pragma_table_info('recurate') WHERE pk > 0),
                       (SELECT count(*) FROM evidence), (SELECT count(*) FROM recurate)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(now, (1, 3, 1, 1), "{older}");
        }
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
