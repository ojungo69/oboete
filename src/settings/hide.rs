//! Parity row 3, decision 57: a card or a session summary removed from the page alone, or brought
//! back (docs/card-hide.md). A `hide` op names it, with no text; the cards or turns consumer
//! applies it before the answer, as a correction is (`claims::applied`).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Refusal, refused};
use crate::hide::{CARD, HideOp, SUMMARY};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    kind: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hide {
    item: Item,
    hidden: bool,
}

/// The longest id the page sends: a card's `<device>.<op seq>.<n>`.
const MAX_ID: usize = 200;

/// How many of each Settings lists.
const LISTED: usize = 200;

/// A card or a summary removed from the page (`hidden`) or brought back, once applied; the same
/// state again writes nothing.
pub(crate) fn save(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Hide = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let bad = || refused(422, "hide_item", "item");
    let target = match posted.item.kind.as_str() {
        "card" => CARD,
        "summary" => SUMMARY,
        _ => return Err(bad()),
    };
    // Opening raw.db makes one where there is none.
    if posted.item.id.len() > MAX_ID || !crate::raw::exists(home) {
        return Err(bad());
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = |e: anyhow::Error| {
        eprintln!("oboete: a removal from the page could not be read or recorded: {e:#}");
        refused(503, "hide_unavailable", "")
    };
    let mut raw = crate::raw::open(home).map_err(unavailable)?;
    let k = crate::knowledge::open(home).map_err(unavailable)?;
    crate::cards::schema(&k).map_err(unavailable)?;
    crate::turns::schema(&k).map_err(unavailable)?;
    let local = raw.device().to_owned();
    // What it names, current, and its mark now.
    let id = posted.item.id.as_str();
    let (device, op_seq, n, now) = if target == CARD {
        let (device, op_seq, n) = crate::cards::id_parts(id, &local).ok_or_else(bad)?;
        let now: Option<bool> = k
            .query_row(
                "SELECT hidden FROM cards WHERE device = ?1 AND op_seq = ?2 AND n = ?3
                   AND replaced_by IS NULL",
                params![device, op_seq, n],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| unavailable(e.into()))?;
        (device.to_owned(), op_seq, Some(n), now)
    } else {
        let (device, op_seq) = crate::turns::id_parts(id, &local).ok_or_else(bad)?;
        let now: Option<bool> = k
            .query_row(
                "SELECT hidden FROM turns WHERE device = ?1 AND op_seq = ?2 AND skipped = 0",
                params![device, op_seq],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| unavailable(e.into()))?;
        (device.to_owned(), op_seq, None, now)
    };
    let Some(shown) = now else {
        return Err(refused(404, "hide_not_found", "item"));
    };
    // The newest op's mark, applied or not: an Undo pressed while the removal waits for the
    // worker is recorded, not taken for the state it already is (Codex on #658).
    let now = crate::hide::newest(&raw, target, (&device, op_seq, n))
        .map_err(unavailable)?
        .unwrap_or(shown);
    if now == posted.hidden {
        let state = if shown == now { "applied" } else { "pending" };
        return Ok(json!({"state": state, "hidden": now}));
    }
    let op = HideOp {
        target: target.to_owned(),
        device,
        op_seq,
        n,
        hidden: posted.hidden,
    };
    let body = serde_json::to_value(&op).map_err(|e| unavailable(e.into()))?;
    let seqs = (raw.append_ops(&[(crate::raw::OpKind::Hide, body)])).map_err(unavailable)?;
    drop(raw);
    let consumer = if target == CARD { "cards" } else { "turns" };
    Ok(
        match crate::claims::applied(home, &k, &local, &seqs, ("removal", consumer)) {
            Ok(()) => json!({"state": "applied", "hidden": posted.hidden}),
            Err(e) => {
                eprintln!("oboete: a removal from the page is recorded, not applied yet: {e:#}");
                json!({"state": "pending", "hidden": posted.hidden, "code": "hide_pending"})
            }
        },
    )
}

/// Where Settings' list of removed cards and summaries goes on (#661): the last of each kind it
/// showed, as the feed's cursor keeps one a kind.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Position {
    cards: Option<crate::cards::Position>,
    summaries: Option<crate::turns::Position>,
}

/// The cards and summaries removed from the page, the newest first after `after`, `LISTED` at
/// most, each with its kind, its ID, its first line, its repository's name and its time, gated as
/// every reader gates them, and where the next page starts (`next`, none at the end); the viewer's
/// answer gate passes over it too, as the feed's.
pub(crate) fn list(home: &Path, mut after: Position) -> Result<Value, Refusal> {
    let unavailable = |e: anyhow::Error| {
        eprintln!("oboete: the removed cards could not be read: {e:#}");
        refused(503, "hide_unavailable", "")
    };
    let Some((raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Ok(json!({"items": [], "next": null}));
    };
    let rules = crate::capture::Settings::load(home)
        .map_err(unavailable)?
        .rules;
    let local = raw.device().to_owned();
    // As the feed names a repository.
    let repo_name = |repo: &Option<String>| {
        crate::consumer::manifest::repo_name(repo.as_deref().unwrap_or(""), &rules)
    };
    let (cards, cards_more) = crate::cards::removed(&k, &raw, after.cards.as_ref(), LISTED, &rules)
        .map_err(unavailable)?;
    let (summaries, summaries_more) =
        crate::turns::removed(&k, &raw, after.summaries.as_ref(), LISTED, &rules)
            .map_err(unavailable)?;
    // The feed's order: the newest first, a card before a summary of the same time.
    let mut rows = Vec::new();
    for c in cards {
        rows.push((
            0,
            c.position(),
            json!({"kind": "card", "id": c.id(&local), "title": c.row_title,
            "repo_name": repo_name(&c.repo), "ts": c.ts}),
        ));
    }
    for s in summaries {
        let (ts, device, seq) = s.position();
        rows.push((
            1,
            (ts, device, seq, 0),
            json!({"kind": "summary", "id": s.id(&local), "title": s.row,
            "repo_name": repo_name(&s.repo), "ts": s.ts}),
        ));
    }
    rows.sort_by(|(ak, a, _), (bk, b, _)| {
        b.0.cmp(&a.0)
            .then_with(|| ak.cmp(bk))
            .then_with(|| b.cmp(a))
    });
    let more = rows.len() > LISTED || cards_more || summaries_more;
    rows.truncate(LISTED);
    let mut items = Vec::new();
    for (kind, (ts, device, seq, n), value) in rows {
        match kind {
            0 => after.cards = Some((ts, device, seq, n)),
            _ => after.summaries = Some((ts, device, seq)),
        }
        items.push(value);
    }
    let next = more
        .then(|| serde_json::to_string(&after))
        .transpose()
        .map_err(|e| unavailable(e.into()))?;
    Ok(json!({"items": items, "next": next}))
}

#[cfg(test)]
mod tests;
