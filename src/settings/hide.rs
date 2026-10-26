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

    /// Whether `table`'s full-text index has an entry for its row `id` names (`<op_seq>.<n>` or
    /// `S<op_seq>`).
    fn indexed(home: &Path, id: &str) -> bool {
        let k = crate::knowledge::open(home).unwrap();
        let (sql, op_seq, n) = match id.strip_prefix('S') {
            Some(seq) => (
                "SELECT count(*) FROM turns_fts WHERE rowid IN
                   (SELECT rowid FROM turns WHERE op_seq = ?1 AND ?2 = 0)",
                seq.parse::<i64>().unwrap(),
                0,
            ),
            None => {
                let (seq, n) = id.split_once('.').unwrap();
                (
                    "SELECT count(*) FROM cards_fts WHERE rowid IN
                       (SELECT rowid FROM cards WHERE op_seq = ?1 AND n = ?2)",
                    seq.parse::<i64>().unwrap(),
                    n.parse::<i64>().unwrap(),
                )
            }
        };
        k.query_row(sql, params![op_seq, n], |r| r.get::<_, i64>(0))
            .unwrap()
            > 0
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
        assert!(!indexed(home, &ids[0]), "its full-text entry goes");
        let listed = list(home, Position::default()).unwrap();
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
        assert!(indexed(home, &ids[0]));
        assert_eq!(
            list(home, Position::default()).unwrap(),
            json!({"items": [], "next": null})
        );
        crate::worker::rebuild(home).unwrap();
        assert_eq!(seen(home), shown, "a rebuild takes the newest op's mark");
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
    /// asked again), a rebuild keeps it removed, and it comes back.
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
        let Store {
            home: dir,
            raw: held,
        } = s;
        drop(held);
        let home = dir.path();
        let shown = || {
            let raw = crate::raw::open(home).unwrap();
            let k = crate::knowledge::open(home).unwrap();
            let rules = crate::redact::Rules::default();
            let recent = crate::turns::recent(&k, &raw, R, 10, &rules).unwrap();
            let got = crate::turns::get(&k, &raw, &id, &rules).unwrap();
            let (page, _) =
                crate::turns::page(&k, &raw, (Some(R), None), None, 10, &rules).unwrap();
            let around =
                crate::turns::around(&k, &raw, Some(R), i64::MAX, true, 10, &rules).unwrap();
            let found = found(home, "Zulu") > 0;
            (recent.len(), got.is_some(), page.len(), around.len(), found)
        };
        let rows = || -> i64 {
            (crate::knowledge::open(home).unwrap())
                .query_row(
                    "SELECT count(*) FROM turns WHERE op_seq = ?1 AND hidden = 1",
                    [&id[1..]],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(shown(), (1, true, 1, 1, true));
        hide(home, "summary", &id, true).unwrap();
        assert_eq!((shown(), rows()), ((0, false, 0, 0, false), 1));
        assert!(!indexed(home, &id), "its full-text entry goes");
        assert_eq!(
            list(home, Position::default()).unwrap()["items"][0]["id"],
            json!(id)
        );
        crate::worker::rebuild(home).unwrap();
        assert_eq!(
            (shown(), rows()),
            ((0, false, 0, 0, false), 1),
            "a rebuild keeps it removed"
        );
        hide(home, "summary", &id, false).unwrap();
        assert_eq!(shown(), (1, true, 1, 1, true));
        assert!(indexed(home, &id));
    }

    /// A store with `cards` cards (one window) and `summaries` summaries removed from the page, the
    /// first summary at the cards' time; their ids.
    fn removed(cards: usize, summaries: usize) -> (Store, Vec<String>) {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let device = s.raw.device().to_owned();
        let observations: Vec<Value> = (0..cards)
            .map(|i| json!({"type": "feature", "title": format!("Card {i}")}))
            .collect();
        let card_ids = if cards > 0 {
            s.cards(seq, seq, Value::Array(observations), false)
        } else {
            Vec::new()
        };
        s.run();
        let card_ts: i64 = if cards > 0 {
            (crate::knowledge::open(s.home.path()).unwrap())
                .query_row("SELECT max(ts) FROM cards", [], |r| r.get(0))
                .unwrap()
        } else {
            2_000
        };
        let summary_ids: Vec<String> = (0..summaries as i64)
            .map(|i| {
                let ts = if i == 0 { card_ts } else { 2_000 + i };
                s.turn(
                    json!({"agent": "claude", "session": "s1", "repo": R, "ts": ts,
                    "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
                    "fields": {"request": format!("Summary {i}")}, "skipped": false}),
                )
            })
            .collect();
        let mark = |target: &str, op_seq: i64, n: Option<i64>| {
            let op = HideOp {
                target: target.into(),
                device: device.clone(),
                op_seq,
                n,
                hidden: true,
            };
            (crate::raw::OpKind::Hide, serde_json::to_value(op).unwrap())
        };
        let mut ops: Vec<_> = (card_ids.iter())
            .map(|id| {
                let (op_seq, n) = id.split_once('.').unwrap();
                mark(CARD, op_seq.parse().unwrap(), Some(n.parse().unwrap()))
            })
            .collect();
        ops.extend((summary_ids.iter()).map(|id| mark(SUMMARY, id[1..].parse().unwrap(), None)));
        s.raw.append_ops(&ops).unwrap();
        s.run();
        (s, card_ids.into_iter().chain(summary_ids).collect())
    }

    /// Every page of Settings' list, each after the one before; a list that does not end fails
    /// instead of reading on.
    fn pages(home: &Path) -> Vec<Vec<Value>> {
        let (mut pages, mut after) = (Vec::new(), Position::default());
        loop {
            assert!(pages.len() < 10, "the list does not end");
            let page = list(home, after).unwrap();
            pages.push(page["items"].as_array().unwrap().clone());
            match page["next"].as_str() {
                Some(next) => after = serde_json::from_str(next).unwrap(),
                None => return pages,
            }
        }
    }

    /// #661: Settings lists the removed cards, the removed summaries, and both, page by page past
    /// `LISTED`: each once, the newest first and a card before a summary of the same time, each
    /// page after the last of each kind the one before showed; Bring back on the oldest brings it
    /// back.
    #[test]
    fn settings_lists_every_removed_card_and_summary_page_by_page() {
        for (cards, summaries, expected) in [
            (201, 0, vec![LISTED, 1]),
            (0, 201, vec![LISTED, 1]),
            (201, 201, vec![LISTED, LISTED, 2]),
        ] {
            let (s, made) = removed(cards, summaries);
            let home = s.home.path();
            let read = pages(home);
            let all: Vec<&Value> = read.iter().flatten().collect();
            let ids: Vec<&str> = all.iter().map(|i| i["id"].as_str().unwrap()).collect();
            let case = format!("{cards} cards, {summaries} summaries");
            assert_eq!(
                read.iter().map(Vec::len).collect::<Vec<_>>(),
                expected,
                "{case}"
            );
            let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
            assert_eq!(unique.len(), cards + summaries, "each once: {case}");
            assert!(made.iter().all(|id| unique.contains(id.as_str())), "{case}");
            let key = |i: &Value| {
                (
                    std::cmp::Reverse(i["ts"].as_i64().unwrap()),
                    i["kind"] != "card",
                )
            };
            assert!(all.windows(2).all(|w| key(w[0]) <= key(w[1])), "{case}");
            let oldest = ids.last().unwrap().to_string();
            let kind = all.last().unwrap()["kind"].as_str().unwrap().to_owned();
            hide(home, &kind, &oldest, false).unwrap();
            let ids: Vec<String> = (pages(home).iter().flatten())
                .map(|i| i["id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(ids.len(), cards + summaries - 1, "{case}");
            assert!(!ids.contains(&oldest), "brought back: {case}");
        }
    }

    /// A hide op read before the card or the summary it names (another device's, read first)
    /// still removes it, with the newest op's mark.
    #[test]
    fn a_hide_op_read_before_what_it_names_still_marks_it() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let device = s.raw.device().to_owned();
        let first = s.raw.max_op_seq().unwrap() + 1;
        let (card, summary) = (first + 4, first + 5);
        let mark = |target: &str, op_seq: i64, n: Option<i64>, hidden: bool| {
            let op = HideOp {
                target: target.into(),
                device: device.clone(),
                op_seq,
                n,
                hidden,
            };
            (crate::raw::OpKind::Hide, serde_json::to_value(op).unwrap())
        };
        // Each named twice, the newest removing it.
        s.raw
            .append_ops(&[
                mark(CARD, card, Some(0), false),
                mark(CARD, card, Some(0), true),
                mark(SUMMARY, summary, None, false),
                mark(SUMMARY, summary, None, true),
            ])
            .unwrap();
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        let id = s.turn(
            json!({"agent": "claude", "session": "s1", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Zulu request"}, "skipped": false}),
        );
        assert_eq!(
            (ids[0].clone(), id.clone()),
            (format!("{card}.0"), format!("S{summary}"))
        );
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let rules = crate::redact::Rules::default();
        assert!(card_titles(home).is_empty());
        assert!(
            crate::turns::recent(&k, &s.raw, R, 10, &rules)
                .unwrap()
                .is_empty()
        );
        assert!(!indexed(home, &ids[0]) && !indexed(home, &id));
        assert_eq!(
            list(home, Position::default()).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    /// A rewind past a hide op shows what it removed again: the op is gone from the log.
    #[test]
    fn a_rewind_past_its_hide_op_shows_the_card_and_the_summary_again() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        let summary = s.turn(
            json!({"agent": "claude", "session": "s1", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Zulu request"}, "skipped": false}),
        );
        s.run();
        let device = s.raw.device().to_owned();
        let before = s.raw.max_op_seq().unwrap();
        let home = s.home.path();
        hide(home, "card", &ids[0], true).unwrap();
        hide(home, "summary", &summary, true).unwrap();
        let summaries = || {
            let k = crate::knowledge::open(home).unwrap();
            let rules = crate::redact::Rules::default();
            crate::turns::recent(&k, &s.raw, R, 10, &rules)
                .unwrap()
                .len()
        };
        assert_eq!((card_titles(home).len(), summaries()), (0, 0));
        let k = crate::knowledge::open(home).unwrap();
        crate::consumer::cards::Cards
            .rewind(&k, &device, before)
            .unwrap();
        crate::consumer::turns::Turns
            .rewind(&k, &device, before)
            .unwrap();
        drop(k);
        assert_eq!(card_titles(home), ["Azimuth parser"]);
        assert_eq!(summaries(), 1);
    }

    /// An Undo pressed while the removal waits for the worker is recorded and wins once both are
    /// applied; a request for the state the newest waiting op gives writes nothing and says it is
    /// pending (Codex on #658).
    #[test]
    fn an_undo_before_the_removal_is_applied_is_recorded() {
        let mut s = Store::new();
        let seq = s.said("s1", R, 1_000, "Ship the parser.");
        let ids = s.cards(
            seq,
            seq,
            json!([{"type": "feature", "title": "Azimuth parser"}]),
            false,
        );
        s.run();
        let home = s.home.path().to_owned();
        let card: i64 = ids[0].split_once('.').unwrap().0.parse().unwrap();
        let device = s.raw.device().to_owned();
        let mut waiting = |hidden: bool| {
            let op = HideOp {
                target: CARD.into(),
                device: device.clone(),
                op_seq: card,
                n: Some(0),
                hidden,
            };
            let body = serde_json::to_value(&op).unwrap();
            s.raw
                .append_ops(&[(crate::raw::OpKind::Hide, body)])
                .unwrap();
        };
        let ops = || crate::raw::open(&home).unwrap().max_op_seq().unwrap();
        // A removal and its bringing back, both waiting for the worker: the newest says shown.
        waiting(true);
        waiting(false);
        assert_eq!(card_titles(&home), ["Azimuth parser"], "not applied yet");
        let before = ops();
        assert_eq!(
            hide(&home, "card", &ids[0], false).unwrap(),
            json!({"state": "applied", "hidden": false})
        );
        assert_eq!(ops(), before, "the newest waiting op already says it");
        // A removal waiting: the same removal again writes nothing; an Undo is recorded.
        waiting(true);
        let before = ops();
        assert_eq!(
            hide(&home, "card", &ids[0], true).unwrap(),
            json!({"state": "pending", "hidden": true})
        );
        assert_eq!(ops(), before);
        hide(&home, "card", &ids[0], false).unwrap();
        assert_eq!(ops(), before + 1, "the Undo is recorded");
        s.run();
        assert_eq!(card_titles(&home), ["Azimuth parser"]);
    }

    /// The newest op's mark wins whatever order the devices' ops are read in: a removal and a later
    /// bringing back from two other devices, in either order of their ids, for a card and a
    /// summary, through a rebuild too (Codex on #658).
    #[test]
    fn the_newest_mark_wins_across_devices() {
        for (older, newer) in [("aaaa", "zzzz"), ("zzzz", "aaaa")] {
            let mut s = Store::new();
            let seq = s.said("s1", R, 1_000, "Ship the parser.");
            let ids = s.cards(
                seq,
                seq,
                json!([{"type": "feature", "title": "Azimuth parser"}]),
                false,
            );
            let summary = s.turn(json!({"agent": "claude", "session": "s1", "repo": R,
                "ts": 2_000, "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
                "fields": {"request": "Zulu request"}, "skipped": false}));
            let card: i64 = ids[0].split_once('.').unwrap().0.parse().unwrap();
            let turn: i64 = summary[1..].parse().unwrap();
            let device = s.raw.device().to_owned();
            // The older op has the higher op seq: only the time orders them.
            let put = |(op_device, op_seq): (&str, i64),
                       ts: i64,
                       (target, seq, n): (&str, i64, Option<i64>),
                       hidden: bool| {
                let op = HideOp {
                    target: target.into(),
                    device: device.clone(),
                    op_seq: seq,
                    n,
                    hidden,
                };
                rusqlite::Connection::open(s.home.path().join("raw.db"))
                    .unwrap()
                    .execute(
                        "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                         VALUES(?1, ?2, 'hide', ?3, ?4, ?2)",
                        params![op_device, op_seq, ts, serde_json::to_string(&op).unwrap()],
                    )
                    .unwrap();
            };
            for (i, named) in [(CARD, card, Some(0)), (SUMMARY, turn, None)]
                .into_iter()
                .enumerate()
            {
                let i = i as i64;
                put((older, 10 + i), 1_000, named, true);
                put((newer, 1 + i), 2_000, named, false);
            }
            s.run();
            let Store {
                home: dir,
                raw: held,
            } = s;
            drop(held);
            let summaries = |home: &Path| {
                let raw = crate::raw::open(home).unwrap();
                let k = crate::knowledge::open(home).unwrap();
                let rules = crate::redact::Rules::default();
                crate::turns::recent(&k, &raw, R, 10, &rules).unwrap().len()
            };
            for pass in ["read", "rebuilt"] {
                if pass == "rebuilt" {
                    crate::worker::rebuild(dir.path()).unwrap();
                }
                assert_eq!(
                    card_titles(dir.path()),
                    ["Azimuth parser"],
                    "{pass}, {older} {newer}"
                );
                assert_eq!(summaries(dir.path()), 1, "{pass}, {older} {newer}");
            }
        }
    }

    /// A hide op raises the home to the format that reads it, in the transaction that writes it,
    /// and a rebuild from the backup's lines stamps it too, the outstanding expiry batch a restore
    /// finishes on it included: a binary before it stops at the op.
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
        rebuild
            .expire(&[(device.clone(), seq, 1_000)], 2_000)
            .unwrap();
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
