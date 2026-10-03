//! The cards consumer (docs/cards.md): it reads the op log of every device that has ops and keeps
//! each curated window's cards in knowledge.db (`cards::schema`).

use crate::cards::schema;
use crate::raw::{OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Cards;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;
impl Consumer for Cards {
    fn name(&self) -> &'static str {
        "cards"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in ops.iter().filter(|o| o.kind == OpKind::Window) {
            let Some(span) = crate::curate::op_span(&op.body) else {
                continue;
            };
            if op.body["recurate"] == true {
                replace(k, device, op.op_seq, &op.body)?;
            }
            if op.body["outcome"] != "curated" {
                continue;
            }
            // K1: its observations, or without that list its summary, as a card with no title. A
            // summary as long as the op keeps one may have been cut there, ungated (K6): no card.
            let summary = op.body["summary"].as_str().unwrap_or("").trim();
            let of_summary = [serde_json::json!({"narrative": summary})];
            let cards: &[serde_json::Value] = match op.body["observations"].as_array() {
                Some(cards) => cards,
                None if summary.is_empty()
                    || summary.chars().count() >= crate::curate::MAX_SUMMARY_CHARS =>
                {
                    continue;
                }
                None => &of_summary,
            };
            let labels = raw.labels_in(device, span.from, span.to)?;
            let (agent, session) = labels.session.unzip();
            // A list as the table keeps it; none where the op or the card gives none. An op
            // that lists no removal: every removal from its records hides the card (K4).
            let list = |v: &serde_json::Value| match v {
                list @ serde_json::Value::Array(_) => list.to_string(),
                _ => "[]".to_owned(),
            };
            let (goals, removed) = (list(&op.body["goals"]), list(&op.body["removed"]));
            for (n, c) in cards.iter().enumerate() {
                let text = |field: &str| c[field].as_str().unwrap_or("");
                k.execute(
                    "INSERT INTO cards(device, op_seq, n, from_seq, from_offset, to_seq,
                       to_offset, goals, removed, ts, agent, session, repo, type, title,
                       subtitle, narrative, facts, concepts, files_read, files_modified)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19, ?20, ?21)",
                    params![
                        device,
                        op.op_seq,
                        n as i64,
                        span.from,
                        span.from_offset,
                        span.to,
                        span.to_offset,
                        goals,
                        removed,
                        labels.ts.unwrap_or(op.ts),
                        agent,
                        session,
                        labels.repo,
                        c["type"].as_str(),
                        text("title"),
                        text("subtitle"),
                        text("narrative"),
                        list(&c["facts"]),
                        list(&c["concepts"]),
                        list(&c["files_read"]),
                        list(&c["files_modified"])
                    ],
                )?;
            }
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM cards WHERE device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        // What a lost recuration replaced is current again (K5).
        k.execute(
            "UPDATE cards SET replaced_by = NULL WHERE device = ?1 AND replaced_by > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}

/// K3: recuration op `op_seq` replaces the cards of `device`'s earlier windows that overlap what
/// it curated: its range, less the records it kept back. Not what it `covers`, which holds the
/// run's own earlier windows.
fn replace(k: &Connection, device: &str, op_seq: i64, op: &serde_json::Value) -> Result<()> {
    for part in crate::curate::curated_parts(op, op) {
        // Chosen by records; `minus` judges the offsets of a record two windows share.
        let mut st = k.prepare(
            "SELECT DISTINCT op_seq, from_seq, from_offset, to_seq, to_offset FROM cards
             WHERE device = ?1 AND op_seq < ?2 AND replaced_by IS NULL
               AND from_seq <= ?3 AND to_seq >= ?4",
        )?;
        let over = st
            .query_map(params![device, op_seq, part.to, part.from], |r| {
                let span = crate::curate::Span {
                    from: r.get(1)?,
                    from_offset: r.get(2)?,
                    to: r.get(3)?,
                    to_offset: r.get(4)?,
                };
                Ok((r.get::<_, i64>(0)?, span))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (earlier, span) in over {
            if span.minus(&part) != [span] {
                k.execute(
                    "UPDATE cards SET replaced_by = ?1 WHERE device = ?2 AND op_seq = ?3",
                    params![op_seq, device, earlier],
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Cards;
    use crate::cards::{self, Card};
    use crate::raw::{self, Event, OpKind, Raw};
    use crate::redact::Rules;
    use crate::worker::Consumer;
    use serde_json::{Value, json};
    use std::path::Path;

    /// K1: a title is a summary's first sentence, in either language, and says when it was cut.
    #[test]
    fn a_title_is_the_first_sentence_and_shows_a_cut() {
        use crate::cards::first_sentence;
        assert_eq!(first_sentence("v1.2 is out. Next."), "v1.2 is out.");
        assert_eq!(first_sentence("Is it done? Yes."), "Is it done?");
        assert_eq!(
            first_sentence("設定を一つにまとめた。次は表。"),
            "設定を一つにまとめた。"
        );
        assert_eq!(first_sentence("One line\nand another"), "One line");
        let long = first_sentence(&"word ".repeat(40));
        assert_eq!(long.chars().count(), 120);
        assert!(long.ends_with("word…"), "{long}");
    }

    /// A tool event of `session` in `repo`, at `ts`.
    fn event(session: &str, repo: &str, ts: i64) -> Event {
        Event {
            session: session.into(),
            kind: "tool".into(),
            ts,
            repo: Some(repo.into()),
            ..raw::test_event("{}")
        }
    }

    /// A window op over records `from` to `to`, as the curation phase writes one.
    fn window(from: i64, to: i64, outcome: &str, summary: &str) -> (OpKind, Value) {
        let op = json!({"outcome": outcome, "summary": summary, "from_seq": from,
            "from_offset": null, "to_seq": to, "to_offset": null, "elided": []});
        (OpKind::Window, op)
    }

    fn recent(home: &Path, repo: &str) -> Vec<Card> {
        let raw = raw::open(home).unwrap();
        let k = crate::knowledge::open(home).unwrap();
        cards::recent(&k, &raw, repo, 10, &Rules::default()).unwrap()
    }

    fn two_records(raw: &mut Raw) {
        raw.append(&event("s1", "r", 1_000)).unwrap();
        raw.append(&event("s1", "r", 2_000)).unwrap();
    }

    /// K1, K2: a curated window's summary is one card, with its first sentence as the title, of
    /// the session and repository its records are of.
    #[test]
    fn a_curated_windows_summary_is_one_card_of_its_session() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        let summary = "The parser reads one line at a time. Its tests pass.";
        raw.append_ops(&[window(1, 2, "curated", summary)]).unwrap();
        let device = raw.device().to_owned();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let cards = recent(home.path(), "r");
        assert_eq!(cards.len(), 1, "{cards:?}");
        let c = &cards[0];
        assert_eq!(c.title, "The parser reads one line at a time.");
        assert_eq!(c.narrative, summary);
        assert_eq!((c.device.as_str(), c.op_seq, c.n), (device.as_str(), 1, 0));
        assert_eq!(c.ts, 2_000);
        assert_eq!(c.agent.as_deref(), Some("claude"));
        assert_eq!(c.session.as_deref(), Some("s1"));
        assert_eq!(c.repo.as_deref(), Some("r"));
        assert_eq!(c.kind, None);
    }

    /// K1: a window op's observations are its cards, in their order, with claude-mem's fields;
    /// its summary is then no card, and an op with an empty list has none.
    #[test]
    fn a_window_ops_observations_are_its_cards() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        let (kind, mut op) = window(1, 1, "curated", "The summary.");
        op["observations"] = json!([
            {"type": "bugfix", "title": "The parser no longer drops the last line",
             "subtitle": "A missing newline lost it.", "narrative": "It read up to a newline.",
             "facts": ["read_line returned at EOF.", "The fix reads to the end."],
             "concepts": ["problem-solution", "gotcha"],
             "files_read": ["src/a.rs"], "files_modified": ["src/b.rs"]},
            {"type": null, "title": "Second", "subtitle": "", "narrative": "",
             "facts": [], "concepts": [], "files_read": [], "files_modified": []}
        ]);
        let (_, mut none) = window(2, 2, "curated", "Nothing worth a card.");
        none["observations"] = json!([]);
        raw.append_ops(&[(kind, op), (kind, none)]).unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let cards = recent(home.path(), "r");
        assert_eq!(cards.len(), 2, "{cards:?}");
        let c = &cards[0];
        assert_eq!((c.op_seq, c.n, c.kind.as_deref()), (1, 0, Some("bugfix")));
        assert_eq!(c.title, "The parser no longer drops the last line");
        assert_eq!(c.subtitle, "A missing newline lost it.");
        assert_eq!(c.narrative, "It read up to a newline.");
        assert_eq!(
            c.facts,
            ["read_line returned at EOF.", "The fix reads to the end."]
        );
        assert_eq!(c.concepts, ["problem-solution", "gotcha"]);
        assert_eq!(
            (&c.files_read[..], &c.files_modified[..]),
            (&["src/a.rs".to_owned()][..], &["src/b.rs".to_owned()][..])
        );
        assert_eq!(
            (
                cards[1].n,
                cards[1].kind.as_deref(),
                cards[1].title.as_str()
            ),
            (1, None, "Second")
        );
    }

    /// Every card kept, whoever it is of: (narrative, agent, session, repo), in op order.
    type Kept = (String, Option<String>, Option<String>, Option<String>);
    fn kept(home: &Path) -> Vec<Kept> {
        let k = crate::knowledge::open(home).unwrap();
        let mut st = k
            .prepare("SELECT narrative, agent, session, repo FROM cards ORDER BY device, op_seq, n")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// K2: a window over two sessions is no session's card, and one over two repositories no
    /// repository's: its summary may speak of either.
    #[test]
    fn a_window_of_two_sessions_or_two_repositories_is_no_ones_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        raw.append(&event("s1", "r", 1_000)).unwrap();
        raw.append(&event("s2", "r", 2_000)).unwrap();
        raw.append(&event("s2", "other", 3_000)).unwrap();
        raw.append_ops(&[
            window(1, 2, "curated", "Two sessions."),
            window(2, 3, "curated", "Two repositories."),
        ])
        .unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let some = |s: &str| Some(s.to_owned());
        assert_eq!(
            kept(home.path()),
            [
                ("Two sessions.".to_owned(), None, None, some("r")),
                (
                    "Two repositories.".to_owned(),
                    some("claude"),
                    some("s2"),
                    None
                ),
            ]
        );
        assert_eq!(recent(home.path(), "other"), []);
    }

    fn titles(home: &Path) -> Vec<String> {
        recent(home, "r").into_iter().map(|c| c.title).collect()
    }

    /// K3, K5: a recuration replaces the cards of the windows whose records it curated again, a
    /// window it covers only in part too, and a rewind below it brings them back.
    #[test]
    fn a_recuration_replaces_the_cards_over_its_records_until_a_rewind_below_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        for ts in 1..=6 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let (kind, mut again) = window(2, 3, "curated", "Again.");
        again["recurate"] = true.into();
        raw.append_ops(&[
            window(1, 2, "curated", "First."),
            window(3, 4, "curated", "Second."),
            window(5, 6, "curated", "Third."),
        ])
        .unwrap();
        raw.append_ops(&[(kind, again)]).unwrap();
        let device = raw.device().to_owned();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(titles(home.path()), ["Third.", "Again."]);
        let k = crate::knowledge::open(home.path()).unwrap();
        Cards.rewind(&k, &device, 3).unwrap();
        drop(k);
        assert_eq!(titles(home.path()), ["Third.", "Second.", "First."]);
    }

    /// K3: the records of an excluded session that a recuration kept back were not read again,
    /// so a card over them alone stays.
    #[test]
    fn a_recuration_leaves_the_card_over_records_it_kept_back() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        for ts in 1..=3 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let (kind, mut again) = window(1, 3, "curated", "Again.");
        again["recurate"] = true.into();
        again["excluded"] = json!([2]);
        raw.append_ops(&[
            window(1, 1, "curated", "First."),
            window(2, 2, "curated", "Kept back."),
            window(3, 3, "curated", "Third."),
        ])
        .unwrap();
        raw.append_ops(&[(kind, again)]).unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(titles(home.path()), ["Again.", "Kept back."]);
    }

    /// K4: a card is hidden by a removal its window op does not list: the curator read what was
    /// removed, and the card may say it. What the op lists was gone before the window was cut,
    /// and the same removal made again (a restore brought the text back) is still that one.
    #[test]
    fn a_removal_the_window_op_does_not_list_hides_its_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        for ts in 1..=6 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let whole = |seq: i64| raw::Target::Record {
            device: device.clone(),
            seq,
        };
        let part = |seq: i64, offset: i64| raw::Target::Range {
            device: device.clone(),
            seq,
            offset,
            length: 1,
        };
        // Gone before the windows are cut: record 1, and a part of record 3.
        raw.append_tombstone(whole(1)).unwrap();
        raw.append_tombstone(part(3, 0)).unwrap();
        let listing = |from: i64, to: i64, summary: &str, removed: Value| {
            let (kind, mut op) = window(from, to, "curated", summary);
            op["removed"] = removed;
            (kind, op)
        };
        raw.append_ops(&[
            listing(1, 2, "First.", json!([[1, null, null]])),
            listing(3, 4, "Second.", json!([[3, 0, 1]])),
            window(5, 6, "curated", "Third."),
        ])
        .unwrap();
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(titles(home.path()), ["Third.", "Second.", "First."]);
        // The same part of record 3 again hides nothing.
        raw.append_tombstone(part(3, 0)).unwrap();
        assert_eq!(titles(home.path()), ["Third.", "Second.", "First."]);
        // Another part of it does, and so does a record of a window that lists none.
        raw.append_tombstone(part(3, 1)).unwrap();
        raw.append_tombstone(whole(5)).unwrap();
        assert_eq!(titles(home.path()), ["First."]);
    }

    /// K4: a card's records are its window's and the goal the window carried in (its session's
    /// first prompt, shown to the curator beside the window): a removal from the goal's record
    /// that the op does not list hides the card too.
    #[test]
    fn a_removal_from_the_goal_a_window_carried_in_hides_its_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        for ts in 1..=4 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let (kind, mut first) = window(2, 2, "curated", "First.");
        first["goals"] = json!([1]);
        // Cut after a part of the goal's record was removed, which it lists.
        let (_, mut second) = window(3, 4, "curated", "Second.");
        second["goals"] = json!([1]);
        second["removed"] = json!([[1, 0, 1]]);
        raw.append_ops(&[(kind, first), (kind, second)]).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(titles(home.path()), ["Second.", "First."]);
        let part = raw::Target::Range {
            device,
            seq: 1,
            offset: 0,
            length: 1,
        };
        raw.append_tombstone(part).unwrap();
        assert_eq!(titles(home.path()), ["Second."]);
    }

    /// K6: a summary as long as the op keeps one may have been cut there, before any gate read
    /// it: it is no card.
    #[test]
    fn a_summary_at_the_ops_cap_is_no_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        let cap = crate::curate::MAX_SUMMARY_CHARS;
        raw.append_ops(&[
            window(1, 1, "curated", &"x".repeat(cap)),
            window(2, 2, "curated", &"y".repeat(cap - 1)),
        ])
        .unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let kept = kept(home.path());
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].0.starts_with('y'));
    }

    /// The owner's rules, as `config.toml` gives them.
    fn rules(home: &Path, extra: &str) -> Rules {
        std::fs::write(
            home.join("config.toml"),
            format!("[redaction]\nextra_rules = [{extra}]\n"),
        )
        .unwrap();
        Rules::load(home).unwrap()
    }

    /// K6: a card is gated with the rules as they are when it is read, so a rule the owner adds
    /// after it was written hides its value in every field.
    #[test]
    fn a_rule_added_after_a_card_was_written_masks_its_value() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        let summary = "The gate sent otp=AAAA1111 to the vendor. Its tests pass.";
        raw.append_ops(&[window(1, 2, "curated", summary)]).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        assert!(recent(home.path(), "r")[0].title.contains("AAAA1111"));
        let rules = rules(
            home.path(),
            r#"{ id = "otp", regex = 'otp=([A-Za-z0-9]+)', secret_group = 1 }"#,
        );
        let k = crate::knowledge::open(home.path()).unwrap();
        let cards = cards::recent(&k, &raw, "r", 10, &rules).unwrap();
        let c = &cards[0];
        assert!(c.title.starts_with("The gate sent otp="), "{}", c.title);
        for text in [&c.title, &c.narrative] {
            assert!(!text.contains("AAAA1111"), "{text}");
        }
        assert!(c.narrative.ends_with("Its tests pass."), "{}", c.narrative);
    }

    /// K6 for a curator's card: its title, subtitle, narrative, facts and files are each gated
    /// with the rules as they are when it is read.
    #[test]
    fn a_rule_added_after_a_curators_card_was_written_masks_every_field() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        let (kind, mut op) = window(1, 2, "curated", "The summary.");
        op["observations"] = json!([{"type": "change", "title": "Sent otp=AAAA1111",
            "subtitle": "With otp=AAAA1111.", "narrative": "The otp=AAAA1111 went out.",
            "facts": ["otp=AAAA1111 is the value."], "concepts": ["gotcha"],
            "files_read": ["logs/otp=AAAA1111.txt"], "files_modified": ["otp=AAAA1111.rs"]}]);
        raw.append_ops(&[(kind, op)]).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        let rules = rules(
            home.path(),
            r#"{ id = "otp", regex = 'otp=([A-Za-z0-9]+)', secret_group = 1 }"#,
        );
        let k = crate::knowledge::open(home.path()).unwrap();
        let cards = cards::recent(&k, &raw, "r", 10, &rules).unwrap();
        let c = &cards[0];
        let lists = [&c.facts, &c.files_read, &c.files_modified];
        let texts = [&c.title, &c.subtitle, &c.narrative];
        for text in texts.into_iter().chain(lists.into_iter().flatten()) {
            assert!(text.contains("otp=[REDACTED]"), "{text}");
        }
    }

    /// K6: a title is cut from the narrative as the gate leaves it, so a value the rules hide by
    /// what follows its sentence, or one the title's cut would split, is not in the title either.
    #[test]
    fn a_title_is_cut_from_the_gated_narrative() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        for ts in 1..=4 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let by_what_follows = "The value is AAAA1111. That otp went to the vendor.";
        // The title's cut falls inside the value.
        let split = format!("{}otp=AAAA1111BBBB2222 went out.", "word ".repeat(22));
        raw.append_ops(&[
            window(1, 2, "curated", by_what_follows),
            window(3, 4, "curated", &split),
        ])
        .unwrap();
        crate::worker::run_once(home.path()).unwrap();
        let rules = rules(
            home.path(),
            r#"{ id = "before", regex = '([A-Z0-9]{8})\. That otp', secret_group = 1 },
               { id = "otp", regex = 'otp=([A-Z0-9]{16})', secret_group = 1 }"#,
        );
        let k = crate::knowledge::open(home.path()).unwrap();
        let cards = cards::recent(&k, &raw, "r", 10, &rules).unwrap();
        assert_eq!(cards[1].title, "The value is [REDACTED].");
        assert!(
            cards[0].title.ends_with("word otp=[REDA…"),
            "{}",
            cards[0].title
        );
        assert_eq!(cards[0].title.chars().count(), 120);
    }

    /// K6: the session and the repository a card is shown under are gated as its text is.
    #[test]
    fn a_rule_added_after_a_card_was_written_masks_its_labels() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let repo = "host/otp=BBBB2222";
        raw.append(&event("otp=AAAA1111", repo, 1_000)).unwrap();
        raw.append_ops(&[window(1, 1, "curated", "Done.")]).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        let rules = rules(
            home.path(),
            r#"{ id = "otp", regex = 'otp=([A-Z0-9]+)', secret_group = 1 }"#,
        );
        let k = crate::knowledge::open(home.path()).unwrap();
        let cards = cards::recent(&k, &raw, repo, 10, &rules).unwrap();
        let c = &cards[0];
        assert_eq!(c.agent.as_deref(), Some("claude"));
        assert_eq!(c.session.as_deref(), Some("otp=[REDACTED]"));
        assert_eq!(c.repo.as_deref(), Some("host/otp=[REDACTED]"));
    }

    /// K5: the table is derived from the op log, so a rebuild gives the same cards, replaced ones
    /// too.
    #[test]
    fn a_rebuild_gives_the_same_cards() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        for ts in 1..=4 {
            raw.append(&event("s1", "r", ts * 1_000)).unwrap();
        }
        let (kind, mut again) = window(1, 2, "curated", "Again.");
        again["recurate"] = true.into();
        raw.append_ops(&[
            window(1, 2, "curated", "First."),
            window(3, 4, "curated", "Second."),
        ])
        .unwrap();
        raw.append_ops(&[(kind, again)]).unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let before = (kept(home.path()), titles(home.path()));
        assert_eq!(before.1, ["Second.", "Again."]);
        crate::worker::rebuild(home.path()).unwrap();
        assert_eq!((kept(home.path()), titles(home.path())), before);
    }

    /// K1: only a curated window with a summary has a card.
    #[test]
    fn a_window_that_was_not_curated_or_has_no_summary_has_no_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        two_records(&mut raw);
        raw.append_ops(&[
            window(1, 1, "skipped", "Every provider failed."),
            window(2, 2, "covered", ""),
            window(2, 2, "curated", "  "),
        ])
        .unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(kept(home.path()), []);
    }
}
