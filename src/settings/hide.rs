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
    let Some(now) = now else {
        return Err(refused(404, "hide_not_found", "item"));
    };
    if now == posted.hidden {
        return Ok(json!({"state": "applied", "hidden": now}));
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

/// The cards and summaries removed from the page, the newest first, each with its kind, its ID,
/// its first line and its time, gated as every reader gates them.
pub(crate) fn list(home: &Path) -> Result<Value, Refusal> {
    let unavailable = |e: anyhow::Error| {
        eprintln!("oboete: the removed cards could not be read: {e:#}");
        refused(503, "hide_unavailable", "")
    };
    let Some((raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Ok(json!({"items": []}));
    };
    let rules = crate::capture::Settings::load(home)
        .map_err(unavailable)?
        .rules;
    let local = raw.device().to_owned();
    let mut items = Vec::new();
    for c in crate::cards::removed(&k, &raw, LISTED, &rules).map_err(unavailable)? {
        items.push((
            c.ts,
            json!({"kind": "card", "id": c.id(&local), "title": c.row_title,
            "repo": c.repo, "ts": c.ts}),
        ));
    }
    for s in crate::turns::removed(&k, &raw, LISTED, &rules).map_err(unavailable)? {
        items.push((
            s.ts,
            json!({"kind": "summary", "id": s.id(&local), "title": s.row,
            "repo": s.repo, "ts": s.ts}),
        ));
    }
    items.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    Ok(json!({"items": items.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;
    use crate::worker::Consumer;

    const R: &str = "github.com/o/r";

    fn hide(home: &Path, kind: &str, id: &str, hidden: bool) -> Result<Value, Refusal> {
        let body = json!({"item": {"kind": kind, "id": id}, "hidden": hidden});
        save(home, &Mutex::new(()), &serde_json::to_vec(&body).unwrap())
    }

    fn code(r: Result<Value, Refusal>) -> (u16, &'static str) {
        let r = r.unwrap_err();
        (r.status, r.code)
    }

    /// What a search for `text` finds of the caller's repository, every kind of hit.
    fn found(home: &Path, text: &str) -> usize {
        let q = crate::search::b::Query {
            text: text.into(),
            caller: Some(R.into()),
            limit: 20,
            ..Default::default()
        };
        crate::search::b::query(home, &q).unwrap().hits.len()
    }

    fn card_titles(home: &Path) -> Vec<String> {
        let raw = crate::raw::open(home).unwrap();
        let k = crate::knowledge::open(home).unwrap();
        let rules = crate::redact::Rules::default();
        (crate::cards::recent(&k, &raw, R, 10, &rules)
            .unwrap()
            .into_iter())
        .map(|c| c.title)
        .collect()
    }

    /// A card removed from the page leaves session start, the feed, get and search, Settings lists
    /// it, a rebuild keeps it removed, and bringing it back shows it everywhere again; the same
    /// state twice writes one op.
    #[test]
    fn a_card_removed_from_the_page_leaves_its_readers_and_comes_back() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let cards = json!([{"type": "feature", "title": "Azimuth parser",
            "files_read": ["src/a.rs"]}, {"type": "feature", "title": "Bravo lexer"}]);
        let ids = s.cards(seq, seq, cards, false);
        s.run();
        let Store {
            home: dir,
            raw: held,
        } = s;
        drop(held);
        let home = dir.path();
        // What each reader shows: get, the feed's page, the timeline, the note on src/a.rs, the
        // cards a turn's summary is shown, and the words search.
        let seen = |home: &Path| {
            let raw = crate::raw::open(home).unwrap();
            let k = crate::knowledge::open(home).unwrap();
            let rules = crate::redact::Rules::default();
            let got = crate::cards::get(&k, &raw, &ids[0], &rules).unwrap();
            let (page, _) =
                crate::cards::page(&k, &raw, (Some(R), None), None, 10, &rules).unwrap();
            let around =
                crate::cards::around(&k, &raw, Some(R), i64::MAX, true, 10, &rules).unwrap();
            let filed = crate::cards::by_file(&k, &raw, R, &["src/a.rs"], 40, &rules).unwrap();
            let device = raw.device().to_owned();
            let turn =
                (crate::cards::of_turn(&k, &raw, &device, ("claude", "s1"), (1, 1), 10, &rules))
                    .unwrap();
            let found = found(home, "Azimuth") > 0;
            (
                got.is_some(),
                page.len(),
                around.len(),
                filed.len(),
                turn.len(),
                found,
            )
        };
        let (shown, gone) = ((true, 2, 2, 1, 2, true), (false, 1, 1, 0, 1, false));
        assert_eq!(card_titles(home), ["Azimuth parser", "Bravo lexer"]);
        assert_eq!(seen(home), shown);
        assert_eq!(
            hide(home, "card", &ids[0], true).unwrap(),
            json!({"state": "applied", "hidden": true})
        );
        assert_eq!(card_titles(home), ["Bravo lexer"]);
        assert_eq!(seen(home), gone);
        let listed = list(home).unwrap();
        assert_eq!(
            (&listed["items"][0]["kind"], &listed["items"][0]["id"]),
            (&json!("card"), &json!(ids[0]))
        );
        crate::worker::rebuild(home).unwrap();
        assert_eq!(seen(home), gone, "a rebuild keeps it removed");
        let ops = || crate::raw::open(home).unwrap().max_op_seq().unwrap();
        let before = ops();
        hide(home, "card", &ids[0], true).unwrap();
        assert_eq!(ops(), before, "the same state again writes nothing");
        hide(home, "card", &ids[0], false).unwrap();
        assert_eq!(card_titles(home), ["Azimuth parser", "Bravo lexer"]);
        assert_eq!(seen(home), shown);
        assert_eq!(list(home).unwrap(), json!({"items": []}));
    }

    /// A removed card loses its vector and waits for none; brought back, it waits for one again.
    #[test]
    fn a_removed_card_loses_its_vector_and_waits_for_none() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        s.run();
        let home = s.home.path();
        let key = format!("{}.{}", s.raw.device(), ids[0]);
        let k = crate::knowledge::open(home).unwrap();
        k.execute(
            "INSERT INTO vector_keys(id, embedder, kind, key, src_sha) VALUES (1, 'test', 'o', ?1, 'x')",
            [&key],
        )
        .unwrap();
        k.execute(
            "INSERT INTO vec_index(rowid, embedder, kind, repo, ts, session, embedding)
             VALUES (1, 'test', 'o', ?1, 1000, 's1', vec_bit(?2))",
            params![R, crate::embed::bits(&vec![0.5_f32; crate::embed::DIM])],
        )
        .unwrap();
        let waits = || {
            *crate::embed_phase::doctor_facts_in(&k, "test")
                .cards
                .as_ref()
                .unwrap()
        };
        let rows = |table: &str| -> i64 {
            k.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(waits(), 0);
        hide(home, "card", &ids[0], true).unwrap();
        assert_eq!((rows("vector_keys"), rows("vec_index")), (0, 0));
        assert_eq!(waits(), 0, "a removed card waits for no vector");
        hide(home, "card", &ids[0], false).unwrap();
        assert_eq!(waits(), 1, "brought back, it waits for one");
    }

    /// A summary removed from the page leaves its readers, its row stays (what was asked is not
    /// asked again), and it comes back.
    #[test]
    fn a_summary_removed_from_the_page_keeps_its_row_and_comes_back() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Build it.");
        let id = s.turn(
            json!({"agent": "claude", "session": "s1", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Zulu request"}, "skipped": false}),
        );
        s.run();
        let home = s.home.path();
        let rules = crate::redact::Rules::default();
        let shown = || {
            let k = crate::knowledge::open(home).unwrap();
            let recent = crate::turns::recent(&k, &s.raw, R, 10, &rules).unwrap();
            let got = crate::turns::get(&k, &s.raw, &id, &rules).unwrap();
            let (page, _) =
                crate::turns::page(&k, &s.raw, (Some(R), None), None, 10, &rules).unwrap();
            let around =
                crate::turns::around(&k, &s.raw, Some(R), i64::MAX, true, 10, &rules).unwrap();
            let found = found(home, "Zulu") > 0;
            (recent.len(), got.is_some(), page.len(), around.len(), found)
        };
        assert_eq!(shown(), (1, true, 1, 1, true));
        hide(home, "summary", &id, true).unwrap();
        assert_eq!(shown(), (0, false, 0, 0, false));
        let k = crate::knowledge::open(home).unwrap();
        let rows: i64 = k
            .query_row(
                "SELECT count(*) FROM turns WHERE op_seq = ?1 AND hidden = 1",
                [&id[1..]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(list(home).unwrap()["items"][0]["id"], json!(id));
        hide(home, "summary", &id, false).unwrap();
        assert_eq!(shown(), (1, true, 1, 1, true));
    }

    /// A rewind past a hide op shows what it removed again: the op is gone from the log.
    #[test]
    fn a_rewind_past_its_hide_op_shows_the_card_again() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        s.run();
        let device = s.raw.device().to_owned();
        let before = s.raw.max_op_seq().unwrap();
        let home = s.home.path();
        hide(home, "card", &ids[0], true).unwrap();
        assert!(card_titles(home).is_empty());
        let k = crate::knowledge::open(home).unwrap();
        crate::consumer::cards::Cards
            .rewind(&k, &device, before)
            .unwrap();
        drop(k);
        assert_eq!(card_titles(home), ["Azimuth parser"]);
    }

    /// A hide op raises the home to the format that reads it, in the transaction that writes it,
    /// and a rebuild from the backup's lines stamps it too: a binary before it stops at the op.
    #[test]
    fn a_hide_raises_the_home_to_the_format_that_reads_it() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        s.run();
        let home = s.home.path();
        let stamp = |path: &Path| -> i64 {
            rusqlite::Connection::open(path)
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::OPEN_FORMAT);
        hide(home, "card", &ids[0], true).unwrap();
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::HIDDEN_FORMAT);
        let (device, lines, ops) = (
            s.raw.device().to_owned(),
            s.raw.export_lines(0, usize::MAX).unwrap(),
            s.raw.export_op_lines(0, usize::MAX).unwrap(),
        );
        let rebuilt = home.join("raw.db.rebuilt");
        let mut rebuild = crate::raw::Rebuild::new(&rebuilt, &device).unwrap();
        for (_, line) in &lines {
            rebuild.add(line).unwrap();
        }
        for (_, line) in &ops {
            rebuild.add_op(line).unwrap();
        }
        rebuild.finish().unwrap();
        assert_eq!(stamp(&rebuilt), crate::raw::HIDDEN_FORMAT);
    }

    /// What is no current card or summary is refused before anything is written, and a home with
    /// no store gets none.
    #[test]
    fn what_is_no_card_or_summary_is_refused() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            code(hide(empty.path(), "card", "1.0", true)),
            (422, "hide_item")
        );
        assert!(!crate::raw::exists(empty.path()));
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "A card"}]),
            false,
        );
        s.run();
        let home = s.home.path();
        let before = s.raw.max_op_seq().unwrap();
        for (kind, id, expected) in [
            ("claim", "1.0", (422, "hide_item")),
            ("card", "not an id", (422, "hide_item")),
            ("card", "999.0", (404, "hide_not_found")),
            ("summary", "S999", (404, "hide_not_found")),
            ("summary", "1.0", (422, "hide_item")),
        ] {
            assert_eq!(code(hide(home, kind, id, true)), expected, "{kind} {id}");
        }
        assert_eq!(s.raw.max_op_seq().unwrap(), before);
    }
}
