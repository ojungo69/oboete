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
    let (Ok(start), Ok(len)) = (usize::try_from(e.offset), usize::try_from(e.length)) else {
        return Ok(Quote::Gone);
    };
    let event = match record.map(|r| r.item) {
        Some(Item::Event(event)) => event,
        // A mask over it cannot be read against its text any more: expiry waits for every mask's
        // purge, which rewrites the claim ops of a masked record's window first, and for every
        // quote of its records to read (docs/retention.md, rule 8). Its place is still checked.
        Some(Item::Expired(labels)) if len == e.quote.len() => return Ok(Quote::Expired(labels)),
        _ => return Ok(Quote::Gone),
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
mod tests;
