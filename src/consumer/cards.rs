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
/// A title made of a summary's first sentence, at most (K1).
const TITLE: usize = 120;

/// A summary's first sentence, within `TITLE` characters and ending in `…` when it was cut: a
/// line's end, a Japanese full stop, or a `.`, `!` or `?` that ends a word ("v1.2" has none).
fn first_sentence(text: &str) -> String {
    let end = text
        .char_indices()
        .find(|&(i, c)| match c {
            '。' | '！' | '？' | '\n' => true,
            '.' | '!' | '?' => text[i + 1..].chars().next().is_none_or(char::is_whitespace),
            _ => false,
        })
        .map_or(
            text.len(),
            |(i, c)| {
                if c == '\n' { i } else { i + c.len_utf8() }
            },
        );
    let sentence = &text[..end];
    if sentence.chars().count() <= TITLE {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(TITLE - 1).collect();
    format!("{}…", cut.trim_end())
}

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
            let summary = op.body["summary"].as_str().unwrap_or("").trim();
            if op.body["outcome"] != "curated" || summary.is_empty() {
                continue;
            }
            let labels = raw.labels_in(device, span.from, span.to)?;
            let (agent, session) = labels.session.unzip();
            k.execute(
                "INSERT INTO cards(device, op_seq, n, from_seq, from_offset, to_seq, to_offset,
                   at, ts, agent, session, repo, title, narrative)
                 VALUES(?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    device,
                    op.op_seq,
                    span.from,
                    span.from_offset,
                    span.to,
                    span.to_offset,
                    op.body["at"].as_i64().unwrap_or(span.to),
                    labels.ts.unwrap_or(op.ts),
                    agent,
                    session,
                    labels.repo,
                    first_sentence(summary),
                    summary
                ],
            )?;
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
        use super::first_sentence;
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

    /// Every card kept, whoever it is of: (title, agent, session, repo), in op order.
    type Kept = (String, Option<String>, Option<String>, Option<String>);
    fn kept(home: &Path) -> Vec<Kept> {
        let k = crate::knowledge::open(home).unwrap();
        let mut st = k
            .prepare("SELECT title, agent, session, repo FROM cards ORDER BY device, op_seq, n")
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

    /// K4: a card whose window lost a record after it was cut is not shown, since its text may
    /// say what was removed; a record removed before the cut was never read. A window op says
    /// where the records stood when it was cut (`at`), and one without it counts its last record.
    #[test]
    fn a_record_removed_after_the_window_was_cut_hides_its_card() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        let remove = |raw: &mut Raw, seq: i64| {
            let device = device.clone();
            raw.append_tombstone(raw::Target::Record { device, seq })
                .unwrap()
        };
        // Records 1 and 2, and record 1 removed (3) before the window over them is cut.
        two_records(&mut raw);
        assert_eq!(remove(&mut raw, 1), 3);
        // Records 4 and 5, cut, then record 4 removed (6).
        raw.append(&event("s1", "r", 4_000)).unwrap();
        raw.append(&event("s1", "r", 5_000)).unwrap();
        assert_eq!(remove(&mut raw, 4), 6);
        // Records 7 and 8, and record 7 removed (9) before a window over 7 and 8 alone is cut.
        raw.append(&event("s1", "r", 7_000)).unwrap();
        raw.append(&event("s1", "r", 8_000)).unwrap();
        assert_eq!(remove(&mut raw, 7), 9);
        let (kind, mut third) = window(7, 8, "curated", "Third.");
        third["at"] = 9.into();
        raw.append_ops(&[
            window(1, 3, "curated", "First."),
            window(4, 5, "curated", "Second."),
            (kind, third),
        ])
        .unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(titles(home.path()), ["Third.", "First."]);
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
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"otp\", regex = 'otp=([A-Za-z0-9]+)', secret_group = 1 }]\n",
        )
        .unwrap();
        let rules = Rules::load(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let cards = cards::recent(&k, &raw, "r", 10, &rules).unwrap();
        let c = &cards[0];
        assert!(c.title.starts_with("The gate sent otp="), "{}", c.title);
        for text in [&c.title, &c.narrative] {
            assert!(!text.contains("AAAA1111"), "{text}");
        }
        assert!(c.narrative.ends_with("Its tests pass."), "{}", c.narrative);
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
