    use super::*;
    use chrono::{FixedOffset, TimeZone};

    /// W4: a page of cards and the cards on either side of a time are read through the index in
    /// their order, whatever is asked: every repository's through `cards_newest`, one
    /// repository's through `cards_repo`, one session's through `cards_session`. None reads every
    /// card to sort it: a repository's and a session's sort the cards of one `ts` alone. A page
    /// after a card starts at that card, not at the newest.
    #[test]
    fn the_newest_cards_are_read_in_the_order_of_an_index() {
        let k = Connection::open_in_memory().unwrap();
        schema(&k).unwrap();
        let shown = crate::hide::shown(&k, "cards", "").unwrap();
        assert!(
            shown.contains("hidden"),
            "the store has the column the readers filter on"
        );
        // A plan is the statement's, whatever is bound to it: every parameter is left NULL.
        let plan = |sql: &str| -> Vec<String> {
            let mut st = k.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let unbound = vec![rusqlite::types::Null; st.parameter_count()];
            st.query_map(rusqlite::params_from_iter(unbound), |r| r.get(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let pages = [false, true].into_iter().flat_map(|before| {
            [(false, false), (true, false), (false, true), (true, true)]
                .map(|(repo, session)| (repo, session, before))
        });
        for (repo, session, before) in pages {
            let plan = plan(&page_sql(&shown, repo, session, before));
            // Asked for both, either of the two is its order.
            let read: &[&str] = match (repo, session) {
                (false, false) => &["INDEX cards_newest"],
                (true, false) => &["INDEX cards_repo (repo=?"],
                (false, true) => &["INDEX cards_session (agent=? AND session=?"],
                (true, true) => &[
                    "INDEX cards_repo (repo=?",
                    "INDEX cards_session (agent=? AND session=?",
                ],
            };
            assert!(
                plan.iter().any(|p| read.iter().any(|r| p.contains(r)))
                    && !plan.iter().any(|p| p.contains("TEMP B-TREE FOR ORDER BY")),
                "{repo} {session} {before}: {plan:?}"
            );
            // Every repository's page sorts nothing at all.
            if !repo && !session {
                assert!(!plan.iter().any(|p| p.contains("TEMP B-TREE")), "{plan:?}");
            }
            // A page after a card seeks: every repository's to the card, on its four columns;
            // one repository's and one session's to the card's `ts`, where their indexes end.
            let seek = match (repo, session) {
                (false, false) => "((ts,device,op_seq,n)<(?,?,?,?))",
                _ => " AND ts<?)",
            };
            assert_eq!(plan[0].contains(seek), before, "{plan:?}");
        }
        for (repo, older) in [(false, true), (false, false), (true, true), (true, false)] {
            let plan = plan(&around_sql(&shown, repo, older));
            let read = if repo {
                "SEARCH cards USING INDEX cards_repo (repo=? AND ts"
            } else {
                "SEARCH cards USING INDEX cards_newest (ts"
            };
            assert!(
                plan.iter().any(|p| p.contains(read))
                    && !plan.iter().any(|p| p.contains("TEMP B-TREE FOR ORDER BY")),
                "{repo} {older}: {plan:?}"
            );
        }
    }

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    /// Unix ms of a time in Japan.
    fn at(d: u32, h: u32, m: u32, s: u32) -> i64 {
        jst()
            .with_ymd_and_hms(2026, 10, d, h, m, s)
            .unwrap()
            .timestamp_millis()
    }

    /// Parity row 31 (Codex on #571): a line names a document by its text alone only when the text
    /// holds all of its title: an empty one, one cut from the text's start, or one that ends where a
    /// word or a sentence ends; a title that only begins a word of the text is kept.
    #[test]
    fn a_text_begins_with_its_title_only_at_a_words_end() {
        assert!(begins_with_title("Planet-scale storage.", ""));
        assert!(begins_with_title(
            "The lexer reads one line. Then more.",
            "The lexer reads one line."
        ));
        assert!(begins_with_title("Fix the parser now", "Fix the parser"));
        assert!(begins_with_title("One line\nand another", "One line"));
        assert!(begins_with_title(
            "Use tabs in Makefiles, always",
            "Use tabs in Make..."
        ));
        assert!(begins_with_title(
            "Use tabs in Makefiles, always",
            "Use tabs in Make…"
        ));
        assert!(begins_with_title(
            "設定を一つにまとめた。次は表。",
            "設定を一つにまとめた。"
        ));
        assert!(!begins_with_title(
            "Planet-scale storage was picked.",
            "Plan"
        ));
        assert!(!begins_with_title("設定画面を直した。", "設定"));
        assert!(!begins_with_title("Other words.", "Plan"));
    }

    fn card(op_seq: i64, n: i64, ts: i64, kind: Option<&str>, title: &str) -> Card {
        Card {
            device: "d".into(),
            op_seq,
            n,
            ts,
            agent: None,
            session: None,
            repo: Some("r".into()),
            kind: kind.map(str::to_owned),
            title: title.into(),
            row_title: title.into(),
            subtitle: String::new(),
            narrative: String::new(),
            facts: Vec::new(),
            concepts: Vec::new(),
            files_read: Vec::new(),
            files_modified: Vec::new(),
            window: Some((op_seq, op_seq)),
        }
    }

    /// A card of Claude Code's `session` naming `read` and `modified`, as `by_file` finds it for
    /// a file it modified when `modified` is not empty (X5, docs/file-note.md).
    fn filed(op_seq: i64, ts: i64, session: &str, read: &[&str], modified: &[&str]) -> Filed {
        let card = Card {
            session: Some(session.into()),
            files_read: read.iter().map(|s| (*s).to_owned()).collect(),
            files_modified: modified.iter().map(|s| (*s).to_owned()).collect(),
            ..card(op_seq, 0, ts, Some("bugfix"), &format!("Card {op_seq}"))
        };
        Filed {
            named: read.len() + modified.len(),
            card,
            session: Some((Some("claude".into()), session.into())),
            modified: !modified.is_empty(),
            uid: None,
        }
    }

    /// `f` with its card changed by `change`.
    fn with(mut f: Filed, change: impl FnOnce(&mut Card)) -> Filed {
        change(&mut f.card);
        f
    }

    /// The IDs of a note's rows, in their order.
    fn rows(note: &str) -> Vec<&str> {
        note.lines()
            .filter(|l| l.contains(" 🔴 ") || l.contains(" ⚖️ "))
            .map(|l| l.split(' ').next().unwrap())
            .collect()
    }

    /// X5 F7: claude-mem's file context, with oboete's IDs and `get`: its days in order, each
    /// day's rows the oldest first, a title on one line of at most 160 characters.
    #[test]
    fn the_note_on_a_file_is_claude_mems_text() {
        let long = format!("{}\n  tail", "x".repeat(200));
        let cards = [
            with(filed(3, at(3, 18, 14, 0), "s3", &["src/a.rs"], &[]), |c| {
                c.kind = Some("decision".into());
            }),
            with(filed(2, at(2, 21, 41, 0), "s2", &["src/a.rs"], &[]), |c| {
                c.row_title = long;
            }),
            with(filed(1, at(2, 9, 5, 0), "s1", &["src/a.rs"], &[]), |c| {
                c.row_title = " ".into();
            }),
        ];
        let note = file_note(&cards, 0, "d", at(3, 18, 16, 0), &jst()).unwrap();
        let expected = [
            "Current: 2026-10-03 6:16pm GMT+9",
            "This file has prior observations — supplementary context follows. The Read result below is the full requested section.",
            "- **Need details on a past observation?** get(ID) — ~300 tokens each.",
            "### Oct 2, 2026",
            "1.0 9:05a 🔴 Untitled",
            &format!("2.0 9:41p 🔴 {}", "x".repeat(160)),
            "### Oct 3, 2026",
            "3.0 6:14p ⚖️ Card 3",
        ]
        .join("\n");
        assert_eq!(note, expected);
    }

    /// X5 F6: one card a session, the newest; ranked +2 for the file modified, +2 for at most 3
    /// files named, +1 for at most 8; the 15 best.
    #[test]
    fn the_note_keeps_one_card_a_session_ranked_as_claude_mem_ranks() {
        let wide = ["a", "b", "c", "d", "e", "f", "g", "h", "src/a.rs"];
        let now = at(3, 12, 0, 0);
        // Newest first, as the reader gives them: the newest names nine files and only reads
        // this one (0), the next modifies it among nine (2), fourteen more read it alone (2).
        let mut cards = vec![
            filed(50, at(3, 11, 50, 0), "wide", &wide, &[]),
            filed(49, at(3, 11, 49, 0), "changed", &wide[..8], &["src/a.rs"]),
        ];
        cards.extend((0..14).map(|i| {
            filed(
                30 - i,
                at(3, 11, 30 - i as u32, 0),
                &format!("s{i}"),
                &["src/a.rs"],
                &[],
            )
        }));
        let note = file_note(&cards, 0, "d", now, &jst()).unwrap();
        assert_eq!(rows(&note).len(), 15);
        assert!(!rows(&note).contains(&"50.0"), "{note}");
        assert!(rows(&note).contains(&"49.0"), "{note}");
        // Two cards of one session: the newer only.
        let same = [
            filed(2, at(3, 11, 2, 0), "s", &["src/a.rs"], &[]),
            filed(1, at(3, 11, 1, 0), "s", &["src/a.rs"], &[]),
        ];
        let note = file_note(&same, 0, "d", now, &jst()).unwrap();
        assert_eq!(rows(&note), ["2.0"]);
        // The same session text of two agents is two sessions (Codex on #387).
        let mut codex = same[1].clone();
        codex.session = Some((Some("codex".into()), "s".into()));
        let note = file_note(&[same[0].clone(), codex], 0, "d", now, &jst()).unwrap();
        assert_eq!(rows(&note), ["1.0", "2.0"]);
    }

    /// docs/file-note.md F14: an imported observation's row shows its uid, and its session, which
    /// has no agent, is not a card's of the same session text.
    #[test]
    fn an_imported_rows_id_is_its_uid() {
        let card = filed(2, at(3, 11, 2, 0), "s", &["src/a.rs"], &[]);
        let mut imported = filed(1, at(3, 11, 1, 0), "s", &["src/a.rs"], &[]);
        imported.uid = Some("claude-mem:abc:o9".into());
        imported.session = Some((None, "s".into()));
        let note = file_note(&[card, imported], 0, "d", at(3, 12, 0, 0), &jst()).unwrap();
        assert_eq!(rows(&note), ["claude-mem:abc:o9", "2.0"]);
    }

    /// F14 (test 20): an imported row is ranked by its own number of files and flag (its card
    /// names none), one a session, and dates the note as a card does: F4 takes the newest of both.
    #[test]
    fn an_imported_row_ranks_and_dates_the_note_as_a_card_does() {
        let imported = |id: i64, ts: i64, session: &str, named: usize, modified: bool| Filed {
            named,
            modified,
            uid: Some(format!("claude-mem:abc:o{id}")),
            session: Some((None, session.into())),
            ..filed(id, ts, session, &[], &[])
        };
        let nine = ["a", "b", "c", "d", "e", "f", "g", "h", "src/a.rs"];
        // Newest first, as `on_file` gives them: 15 cards of nine files (no points), then older
        // imported rows: one modified it and names one file, one names 20, and two of a session.
        let mut all: Vec<Filed> = (0..15)
            .rev()
            .map(|i| {
                filed(
                    100 + i,
                    at(3, 10, i as u32, 0),
                    &format!("s{i}"),
                    &nine,
                    &[],
                )
            })
            .collect();
        all.extend([
            imported(1, at(3, 9, 30, 0), "x", 3, false),
            imported(2, at(3, 9, 20, 0), "x", 1, true),
            imported(3, at(3, 9, 0, 0), "y", 1, true),
            imported(4, at(3, 8, 0, 0), "z", 20, false),
        ]);
        let note = file_note(&all, 0, "d", at(3, 12, 0, 0), &jst()).unwrap();
        let shown = rows(&note);
        assert_eq!(shown.len(), 15);
        for id in ["claude-mem:abc:o1", "claude-mem:abc:o3"] {
            assert!(shown.contains(&id), "{id}: {shown:?}");
        }
        for id in ["claude-mem:abc:o2", "claude-mem:abc:o4", "100.0"] {
            assert!(!shown.contains(&id), "{id}: {shown:?}");
        }
        // F4: the newest of both.
        let pair = [
            imported(5, at(3, 11, 0, 0), "w", 1, false),
            filed(6, at(3, 9, 0, 0), "s", &["src/a.rs"], &[]),
        ];
        assert!(file_note(&pair, at(3, 10, 0, 0), "d", at(3, 12, 0, 0), &jst()).is_some());
        assert!(file_note(&pair, at(3, 11, 0, 0), "d", at(3, 12, 0, 0), &jst()).is_none());
    }

    /// X5 F4: no note when the file changed at or after its newest card, or no card names it.
    #[test]
    fn no_note_when_the_file_changed_since_its_newest_card() {
        let cards = [filed(1, at(3, 10, 0, 0), "s1", &["src/a.rs"], &[])];
        let note = |mtime| file_note(&cards, mtime, "d", at(3, 11, 0, 0), &jst());
        assert!(note(at(3, 10, 0, 0)).is_none());
        assert!(note(at(3, 9, 59, 0)).is_some());
        assert!(file_note(&[], 0, "d", 0, &jst()).is_none());
    }

    /// Newest first, as `recent` gives them.
    fn four() -> Vec<Card> {
        vec![
            card(
                420,
                1,
                at(3, 6, 5, 30),
                Some("bugfix"),
                "Two workers no longer race for one lock",
            ),
            card(
                413,
                0,
                at(2, 21, 41, 50),
                Some("change"),
                "The lock file is removed on exit",
            ),
            card(
                412,
                0,
                at(2, 21, 41, 10),
                Some("discovery"),
                "The worker leaves a lock it no longer holds",
            ),
            Card {
                device: "e".into(),
                ..card(400, 0, at(2, 9, 5, 0), None, "A summary card")
            },
        ]
    }

    /// docs/cards.md S2, S3: claude-mem's recent context, in local time, the oldest first by day;
    /// another device's card is named with its device.
    #[test]
    fn the_block_is_claude_mems_recent_context_in_local_time() {
        let block = block(
            &four(),
            &[],
            None,
            "d",
            "oboete",
            &HashMap::new(),
            at(3, 7, 37, 0),
            &jst(),
        );
        assert_eq!(
            block,
            "# [oboete] recent context, 2026-10-03 7:37am GMT+9\n\
             \n\
             Legend: 🎯session ●bugfix ◆feature ↻refactor ✓change ○discovery ⚖decision \
             ⚠security_alert ⚷security_note ⊘sensitive\n\
             Format: ID TIME TYPE TITLE\n\
             Fetch details: get(ID) | Search: search(query)\n\
             \n\
             Stats: 4 obs (36t read) | 0t work\n\
             \n\
             ### Oct 2, 2026\n\
             e.400.0 9:05a 📝 A summary card\n\
             412.0 9:41p ○ The worker leaves a lock it no longer holds\n\
             413.0 \" ✓ The lock file is removed on exit\n\
             ### Oct 3, 2026\n\
             420.1 6:05a ● Two workers no longer race for one lock\n"
        );
        let utc = FixedOffset::east_opt(0).unwrap();
        assert!(block_of(&utc).starts_with("# [oboete] recent context, 2026-10-02 10:37pm UTC\n"));
        let india = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        assert!(block_of(&india).contains(" GMT+5:30\n"));
        let west = FixedOffset::west_opt(7 * 3600).unwrap();
        assert!(block_of(&west).contains(" GMT-7\n"));
    }

    fn block_of(tz: &FixedOffset) -> String {
        block(
            &four(),
            &[],
            None,
            "d",
            "oboete",
            &HashMap::new(),
            at(3, 7, 37, 0),
            tz,
        )
    }

    /// A summary of turn op `op_seq` at `ts` that asked `request`, with `fields`.
    fn summary(op_seq: i64, ts: i64, request: &str, fields: &[(&str, &str)]) -> TurnSummary {
        let mut all: std::collections::BTreeMap<String, String> = fields
            .iter()
            .map(|(f, t)| ((*f).to_owned(), (*t).to_owned()))
            .collect();
        if !request.is_empty() {
            all.insert("request".into(), request.into());
        }
        TurnSummary {
            device: "d".into(),
            op_seq,
            ts,
            agent: "claude".into(),
            session: "s".into(),
            repo: Some("r".into()),
            row: request.into(),
            fields: all,
        }
    }

    /// docs/summaries.md S7, S8: the newest summaries are rows among the cards by their own time,
    /// after a card of the same time, `S<op seq>` (another device's with its device) and
    /// `Session started` without a request; the newest one's fields follow the timeline when it is
    /// not older than the newest card shown, notes last, as claude-mem 13.34.2 shows them (#403).
    #[test]
    fn the_block_shows_the_session_summaries_among_the_cards() {
        let newest = summary(
            500,
            at(3, 6, 20, 0),
            "Fix the lock race",
            &[
                ("investigated", "How two workers take the lock."),
                ("learned", "The lock was never released."),
                ("completed", "Workers release it on exit."),
                ("next_steps", "Measure the wait."),
                ("notes", "The agent says the wait was not measured."),
            ],
        );
        let other = TurnSummary {
            device: "e".into(),
            ..summary(450, at(2, 21, 41, 10), "", &[])
        };
        let sessions = [newest.clone(), other];
        let shown = block(
            &four(),
            &sessions,
            Some(&newest),
            "d",
            "oboete",
            &HashMap::new(),
            at(3, 7, 37, 0),
            &jst(),
        );
        let timeline = shown.split_once("### ").unwrap().1;
        assert_eq!(
            timeline,
            "Oct 2, 2026\n\
             e.400.0 9:05a 📝 A summary card\n\
             412.0 9:41p ○ The worker leaves a lock it no longer holds\n\
             Se.450 Session started (Oct 2, 9:41 PM)\n\
             413.0 \" ✓ The lock file is removed on exit\n\
             ### Oct 3, 2026\n\
             420.1 6:05a ● Two workers no longer race for one lock\n\
             S500 Fix the lock race (Oct 3, 6:20 AM)\n\
             **Investigated**: How two workers take the lock.\n\
             \n\
             **Learned**: The lock was never released.\n\
             \n\
             **Completed**: Workers release it on exit.\n\
             \n\
             **Next Steps**: Measure the wait.\n\
             \n\
             **Notes**: The agent says the wait was not measured.\n\
             \n"
        );
        // Older than the newest card shown: no fields.
        let older = summary(501, at(3, 6, 0, 0), "Earlier", &[("learned", "Something.")]);
        let shown = block(
            &four(),
            std::slice::from_ref(&older),
            Some(&older),
            "d",
            "oboete",
            &HashMap::new(),
            at(3, 7, 37, 0),
            &jst(),
        );
        assert!(
            shown.contains("S501 Earlier (Oct 3, 6:00 AM)\n") && !shown.contains("**Learned**")
        );
    }

    /// Parity row 13: claude-mem's economics line at its default settings, read as its
    /// TokenCalculator reads an observation (UTF-16 units of title, subtitle, narrative and the
    /// facts' JSON, four to a token, rounded up per card), against the curator's spend on the
    /// windows shown, each window once, another device's never; its footer when that saved some.
    #[test]
    fn the_stats_line_counts_each_window_once() {
        let mut first = card(10, 0, at(3, 6, 0, 0), Some("bugfix"), "Twelve chars");
        first.window = Some((1, 3));
        first.facts = vec!["ab".into()];
        // Eight UTF-16 units, twenty-four bytes.
        let second = Card {
            n: 1,
            title: "設定画面を直した".into(),
            ..first.clone()
        };
        let mut third = card(11, 0, at(3, 6, 1, 0), Some("change"), "x");
        third.window = Some((4, 4));
        third.narrative = "y".repeat(10);
        // The next window of a record split in two: the same records, its own op and call.
        let mut split = card(13, 0, at(3, 6, 2, 0), Some("change"), "z");
        split.window = Some((4, 4));
        let other = Card {
            device: "e".into(),
            op_seq: 12,
            ..first.clone()
        };
        let work = HashMap::from([(10, 9_400), (11, 3_000), (12, 50), (13, 1_000)]);
        let shown = block(
            &[first, second, third, split, other],
            &[],
            None,
            "d",
            "oboete",
            &work,
            at(3, 7, 37, 0),
            &jst(),
        );
        // 12 + 6 ("[\"ab\"]") = 18 -> 5; 8 + 6 -> 4; 1 + 10 + 2 -> 4; 1 + 2 -> 1; 18 -> 5.
        assert!(
            shown.contains(
                "Fetch details: get(ID) | Search: search(query)\n\n\
                 Stats: 5 obs (19t read) | 13,400t work | 100% savings\n\n### "
            ),
            "{shown}"
        );
        assert!(
            shown.ends_with("\n\nAccess 13k tokens of past work via get(ID) or search(query).\n"),
            "{shown}"
        );
        // Nothing spent that this device knows of: claude-mem's line without its percentage, and
        // no footer.
        let none = block(
            &four(),
            &[],
            None,
            "d",
            "oboete",
            &HashMap::new(),
            at(3, 7, 37, 0),
            &jst(),
        );
        assert!(none.contains("\nStats: 4 obs (") && none.contains("t read) | 0t work\n\n"));
        assert!(
            !none.contains("savings") && !none.contains("Access "),
            "{none}"
        );
        // Less spent than the cards cost to read: claude-mem's negative percentage, and no footer.
        let mut cheap = card(10, 0, at(3, 6, 0, 0), Some("bugfix"), "Twelve chars");
        cheap.window = Some((1, 3));
        let little = block(
            &[cheap],
            &[],
            None,
            "d",
            "oboete",
            &HashMap::from([(10, 3)]),
            at(3, 7, 37, 0),
            &jst(),
        );
        // 12 + 2 ("[]") -> 4 tokens read against 3 spent: -33.3%, rounded as Math.round rounds.
        assert!(
            little.contains("Stats: 1 obs (4t read) | 3t work | -33% savings\n"),
            "{little}"
        );
        assert!(!little.contains("Access "), "{little}");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(193_065), "193,065");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    /// Parity row 13: a window op's spend is the call that made its cards: the last before the op
    /// was written, by the provider it names, that made cards or whose refused answer it keeps;
    /// not a later recuration's, another provider's, a failed call's or a summary's. The windows
    /// a long record is split into, which share their records, each have their own; a CLI that
    /// reports no usage counts its estimate; another device's card and an op that names no
    /// provider have none; a missing ledger is none, never made.
    #[test]
    fn work_is_the_call_that_made_each_window_ops_cards() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let mut window = |from: i64, to: i64, provider: Option<&str>, refused: bool| {
            let mut op = serde_json::json!({"outcome": "curated", "summary": "s", "from_seq": from,
                "from_offset": null, "to_seq": to, "to_offset": null, "elided": [],
                "observations": []});
            if let Some(p) = provider {
                op["provider"] = p.into();
            }
            if refused {
                op["refused"] = true.into();
            }
            raw.append_ops(&[(crate::raw::OpKind::Window, op)]).unwrap();
            // Each op a millisecond apart, as a curated window's op follows its call.
            std::thread::sleep(std::time::Duration::from_millis(5));
            let op_seq = raw.max_op_seq().unwrap();
            let (at, _) = raw.op_stamp(raw.device(), op_seq).unwrap().unwrap();
            let card = Card {
                device: raw.device().to_owned(),
                op_seq,
                window: Some((from, to)),
                ..card(op_seq, 0, 0, None, "t")
            };
            (card, at)
        };
        let (whole, a) = window(1, 3, Some("p"), false);
        let (first_part, b1) = window(4, 4, Some("p"), false);
        let (second_part, b2) = window(4, 4, Some("p"), false);
        let (refused, c) = window(5, 5, Some("q"), true);
        let (estimated, d) = window(6, 6, Some("p"), false);
        let (unnamed, _) = window(7, 7, None, false);
        let other = Card {
            device: "e".into(),
            ..whole.clone()
        };
        let cards = vec![
            whole.clone(),
            Card {
                n: 1,
                ..whole.clone()
            },
            first_part.clone(),
            second_part.clone(),
            refused.clone(),
            estimated.clone(),
            unnamed,
            other,
        ];
        assert!(work(home.path(), &raw, &cards).is_empty());
        assert!(!home.path().join("providers.db").exists());
        let db = crate::providers_db::open(home.path()).unwrap();
        for (ts, provider, role, span, outcome, est, prompt, completion) in [
            (a - 20, "p", "curator", "1-3", "ok", 90, Some(100), Some(20)),
            (
                a - 10,
                "p",
                "curator",
                "1-3",
                "ok",
                190,
                Some(200),
                Some(30),
            ),
            (a - 5, "r", "curator", "1-3", "ok", 700, Some(700), Some(77)),
            (a - 1, "p", "summary", "1-3", "ok", 10, Some(10), Some(1)),
            (a - 1, "p", "curator", "1-3", "error", 40, Some(999), None),
            (
                b2 + 10,
                "p",
                "curator",
                "1-3",
                "ok",
                1,
                Some(5_000),
                Some(1),
            ),
            (b1 - 1, "p", "curator", "4-4", "ok", 60, Some(60), Some(10)),
            (b2 - 1, "p", "curator", "4-4", "ok", 80, Some(80), Some(20)),
            (
                c - 2,
                "q",
                "curator",
                "5-5",
                "unanchored",
                50,
                Some(50),
                Some(5),
            ),
            (c - 1, "q", "curator", "5-5", "error", 40, Some(40), None),
            (d - 1, "p", "curator", "6-6", "ok", 70, None, None),
        ] {
            db.execute(
                "INSERT INTO provider_calls(ts, provider, role, span, outcome, ms, est_tokens,
                   prompt_tokens, completion_tokens) VALUES(?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
                rusqlite::params![ts, provider, role, span, outcome, est, prompt, completion],
            )
            .unwrap();
        }
        assert_eq!(
            work(home.path(), &raw, &cards),
            HashMap::from([
                (whole.op_seq, 230),
                (first_part.op_seq, 70),
                (second_part.op_seq, 100),
                (refused.op_seq, 55),
                (estimated.op_seq, 70),
            ])
        );
        // A seek on the partial index, not a scan of every call the ledger keeps.
        let plan: Vec<String> = db
            .prepare(
                "EXPLAIN QUERY PLAN SELECT ts FROM provider_calls
                 WHERE role = 'curator' AND span = '1-3' AND ts <= 5 AND provider = 'p'
                   AND outcome IN ('ok', 'unanchored')
                 ORDER BY ts DESC, id DESC LIMIT 1",
            )
            .unwrap()
            .query_map([], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|p| p.contains("provider_calls_span")),
            "{plan:?}"
        );
    }
