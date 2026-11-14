    use super::super::fixture::Store;
    use super::*;
    use serde_json::json;

    const R: &str = "github.com/o/r";
    /// Nov 14, 2023, 10:13 PM UTC.
    const DAY: i64 = 1_700_000_000_000;

    /// N5 (docs/memory-lookup.md): a repository's prompts are read by the consumer's index
    /// `raw_docs_prompts` on both sides; every repository's are not.
    #[test]
    fn a_repositorys_prompts_are_read_by_their_index() {
        let mut s = Store::new();
        s.said("s", R, DAY, "Deploys wait for the canary.");
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let plan = |sql: &str| -> String {
            let mut st = k.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let nulls = vec![rusqlite::types::Value::Null; st.parameter_count()];
            let rows = st.query_map(rusqlite::params_from_iter(nulls), |r| r.get::<_, String>(3));
            rows.unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(" | ")
        };
        for older in [true, false] {
            let named = plan(&prompts_sql(true, older));
            assert!(named.contains("INDEX raw_docs_prompts (repo=?"), "{named}");
        }
    }

    /// A home of two days: a decision the day before; a prompt, the three cards and the summary
    /// of its window; two later decisions, each said in a prompt.
    struct Days {
        s: Store,
        older: (String, String),
        prompt: String,
        cards: Vec<String>,
        summary: String,
        later: Vec<(String, String)>,
    }

    /// A decision the user said in a prompt at `ts`: the prompt's key and the claim's uid.
    fn decided(s: &mut Store, ts: i64, text: &str) -> (String, String) {
        let seq = s.said("s", R, ts, text);
        let uid = s.claim(seq, text, ("decision", "decided", "user"), &[]);
        (s.key(seq), uid)
    }

    fn days() -> Days {
        let mut s = Store::new();
        let older = decided(&mut s, DAY - 86_400_000, "Use the trigram tokenizer.");
        let seq = s.said("work", R, DAY, "Fix the Quartz parser in the reader.");
        let cards = s.cards(
            seq,
            seq,
            json!([
                {"type": "bugfix", "title": "Quartz parser", "files_modified": ["/w/src/reader.rs"]},
                {"type": "discovery", "title": "Reader buffers", "files_read": ["/w/src/reader.rs"]},
                {"type": "change", "title": "Docs note"},
            ]),
            false,
        );
        let summary = s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 3_000, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Quartz progress"}, "skipped": false}));
        let later = vec![
            decided(&mut s, DAY + 60_000, "Keep the reader streaming."),
            decided(&mut s, DAY + 120_000, "Ship on Friday."),
        ];
        s.run();
        let prompt = s.key(seq);
        Days {
            s,
            older,
            prompt,
            cards,
            summary,
            later,
        }
    }

    fn text(s: &Store, around: Around, depths: (usize, usize)) -> Result<String> {
        timeline_text(
            s.home.path(),
            Some(R),
            Path::new("/w"),
            around,
            depths,
            &chrono::Utc,
        )
    }

    /// The text with each card row's read cost as `~N` (Q7's count, which slice 1 tests).
    fn costless(text: &str) -> String {
        text.lines()
            .map(|l| match l.rfind("| ~") {
                Some(i) if l.starts_with("| ") && l.ends_with(" |") => format!("{}| ~N |", &l[..i]),
                _ => l.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// docs/tools.md slice 2, test 1 (T2-T4): around a card, the cards, the summary and the prompt
    /// of its window and the claim of the day before, by day and by file, the anchor marked; a side
    /// with fewer entries than its depth has fewer.
    #[test]
    fn a_timeline_around_a_card_shows_its_window_by_day_and_file() {
        let d = days();
        let got = text(&d.s, Around::Anchor(d.cards[0].clone()), (2, 3)).unwrap();
        let expected = format!(
            "# Timeline around anchor: {card0}
**Window:** 2 records before -> 3 records after | **Items:** 6

### Nov 13, 2023

**{older}** decision decided Use the trigram tokenizer. (Nov 13, 10:13 PM)

### Nov 14, 2023

**💬 Prompt {prompt}** (Nov 14, 10:13 PM)
> Fix the Quartz parser in the reader.

**src/reader.rs**
| ID | Time | T | Title | Tokens |
|----|------|---|-------|--------|
| {card0} | 10:13 PM | ● | Quartz parser <- **ANCHOR** | ~N |
| {card1} | \" | ○ | Reader buffers | ~N |

**General**
| ID | Time | T | Title | Tokens |
|----|------|---|-------|--------|
| {card2} | 10:13 PM | ✓ | Docs note | ~N |

**🎯 {summary}** Quartz progress (Nov 14, 10:13 PM)",
            card0 = d.cards[0],
            card1 = d.cards[1],
            card2 = d.cards[2],
            older = &d.older.1[..12],
            prompt = d.prompt,
            summary = d.summary,
        );
        assert_eq!(costless(&got), expected, "{got}");
        // Each row's cost is its card's.
        assert!(
            got.lines()
                .filter(|l| l.contains(" | ~"))
                .all(|l| !l.contains("~0 "))
        );
        // The newest claim: one entry before it (its prompt), none after.
        let (prompt, uid) = &d.later[1];
        let got = text(&d.s, Around::Anchor(uid[..12].into()), (1, 3)).unwrap();
        assert_eq!(
            got,
            format!(
                "# Timeline around anchor: {short}
**Window:** 1 records before -> 3 records after | **Items:** 2

### Nov 14, 2023

**💬 Prompt {prompt}** (Nov 14, 10:15 PM)
> Ship on Friday.

**{short}** decision decided Ship on Friday. (Nov 14, 10:15 PM) <- **ANCHOR**
",
                short = &uid[..12]
            )
        );
    }

    /// T3: an anchor among more cards of its own time than its depths keeps its place, the cards of
    /// one window (which share their time) keep their order past ten, and a summary of their time
    /// follows them; T4: a prompt shows its first 100 characters.
    #[test]
    fn an_anchor_among_many_cards_of_its_time_keeps_its_place() {
        let mut s = Store::new();
        let long = format!("{}end", "Sweep the parser. ".repeat(8));
        let seq = s.said("work", R, DAY, &long);
        let steps = (0..12)
            .map(|n| json!({"type": "change", "title": format!("Step {n}")}))
            .collect();
        let cards = s.cards(seq, seq, serde_json::Value::Array(steps), false);
        let summary = s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Sweep done"}, "skipped": false}));
        s.run();
        let rows = |anchor: usize, depths| -> Vec<String> {
            let got = text(&s, Around::Anchor(cards[anchor].clone()), depths).unwrap();
            got.lines()
                .filter(|l| {
                    ["**💬", "**🎯", "> "].iter().any(|p| l.starts_with(p)) || l.contains(" | ✓ | ")
                })
                .map(|l| l.split(" | ~").next().unwrap().to_owned())
                .collect()
        };
        let card = |n: usize, time: &str, anchor: &str| {
            format!("| {} | {time} | ✓ | Step {n}{anchor}", cards[n])
        };
        let mark = " <- **ANCHOR**";
        assert_eq!(
            rows(0, (1, 2)),
            [
                format!("**💬 Prompt {}** (Nov 14, 10:13 PM)", s.key(seq)),
                format!("> {}...", &long[..100]),
                card(0, "10:13 PM", mark),
                card(1, "\"", ""),
                card(2, "\"", ""),
            ]
        );
        assert_eq!(
            rows(0, (0, 2)),
            [
                card(0, "10:13 PM", mark),
                card(1, "\"", ""),
                card(2, "\"", "")
            ]
        );
        assert_eq!(
            rows(10, (2, 1)),
            [
                card(8, "10:13 PM", ""),
                card(9, "\"", ""),
                card(10, "\"", mark),
                card(11, "\"", ""),
            ]
        );
        assert_eq!(
            rows(11, (1, 1)),
            [
                card(10, "10:13 PM", ""),
                card(11, "\"", mark),
                format!("**🎯 {summary}** Sweep done (Nov 14, 10:13 PM)"),
            ]
        );
    }

    /// T1: depths past 50 are cut to it, however large (Codex and Greptile on #450).
    #[test]
    fn depths_past_their_cap_are_cut_to_it() {
        let d = days();
        let got = text(
            &d.s,
            Around::Anchor(d.cards[0].clone()),
            (1_000, usize::MAX),
        )
        .unwrap();
        assert_eq!(
            got.lines().nth(1),
            Some("**Window:** 50 records before -> 50 records after | **Items:** 11")
        );
    }

    /// T3 (Greptile on #450): of more entries of one time than are asked for, the timeline keeps
    /// the nearest: claims of one time come from their reader by key, the other way round.
    #[test]
    fn of_one_times_entries_the_nearest_are_kept() {
        let mut s = Store::new();
        let mut uids: Vec<String> = ["Alpha rule.", "Bravo rule.", "Charlie rule."]
            .iter()
            .map(|text| decided(&mut s, DAY, text).1)
            .collect();
        let seq = s.said("work", R, DAY + 1_000, "Later prompt.");
        s.run();
        uids.sort();
        let last = &uids[2][..12];
        let newest = text(&s, Around::Newest(2), (3, 3)).unwrap();
        assert!(
            newest.contains(last) && !newest.contains(&uids[0][..12]),
            "{newest}"
        );
        let got = text(&s, Around::Anchor(s.key(seq)), (1, 0)).unwrap();
        assert!(got.contains(last) && !got.contains(&uids[1][..12]), "{got}");
    }

    /// T1 (Greptile on #450): a query's anchor is search's first hit when that hit is one of a
    /// pair search keeps together (D2), which one place would drop.
    #[test]
    fn a_query_anchors_on_the_first_of_a_pair() {
        let mut s = Store::new();
        let quote = "Indent widgets with tabs.";
        let source = s.said("work", R, DAY, quote);
        let earlier = s.claim(source, quote, ("preference", "decided", "user"), &[]);
        let later = s.decided(
            R,
            DAY + 1_000,
            "Indent widgets with spaces, never tabs.",
            &[&earlier],
        );
        s.run();
        let q = Query {
            text: "indent widgets".into(),
            caller: Some(R.into()),
            raw: RawArm::Off,
            ..Default::default()
        };
        let got = text(&s, Around::Query(q), (1, 1)).unwrap();
        assert!(
            got.contains(&format!("**Anchor:** {} - ", &later[..12])),
            "{got}"
        );
    }

    /// T4 (Codex and Greptile on #450): a `|` in a card's title stays in its cell.
    #[test]
    fn a_pipe_in_a_title_stays_in_its_cell() {
        let mut s = Store::new();
        let seq = s.said("work", R, DAY, "Pipes.");
        let card = s.cards(
            seq,
            seq,
            json!([{"type": "change", "title": "Support A | B"}]),
            false,
        )[0]
        .clone();
        s.run();
        let got = text(&s, Around::Anchor(card.clone()), (0, 0)).unwrap();
        assert!(
            got.contains(&format!(
                "| {card} | 10:13 PM | ✓ | Support A \\| B <- **ANCHOR** | ~"
            )),
            "{got}"
        );
    }

    /// T4 (Greptile on #450): with every repository, each line and table names its repository.
    #[test]
    fn every_repository_names_each_entrys_repository() {
        let d = days();
        let got = timeline_text(
            d.s.home.path(),
            None,
            Path::new("/w"),
            Around::Anchor(d.cards[0].clone()),
            (2, 3),
            &chrono::Utc,
        )
        .unwrap();
        let named = format!(" [{R}]");
        for line in [
            format!(
                "**{}** decision decided Use the trigram tokenizer. (Nov 13, 10:13 PM){named}",
                &d.older.1[..12]
            ),
            format!("**💬 Prompt {}** (Nov 14, 10:13 PM){named}", d.prompt),
            format!("**src/reader.rs**{named}"),
            format!(
                "**🎯 {}** Quartz progress (Nov 14, 10:13 PM){named}",
                d.summary
            ),
        ] {
            assert!(got.lines().any(|l| l == line), "{line}: {got}");
        }
        let one = text(&d.s, Around::Anchor(d.cards[0].clone()), (2, 3)).unwrap();
        assert!(!one.contains(&named), "{one}");
    }

    /// Test 2 (T1): a query's anchor is search's first hit, named in the header.
    #[test]
    fn a_query_anchors_on_searchs_first_hit() {
        let d = days();
        let q = Query {
            text: "buffers".into(),
            caller: Some(R.into()),
            raw: RawArm::Below,
            limit: 1,
            ..Default::default()
        };
        let got = text(&d.s, Around::Query(q.clone()), (1, 1)).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "# Timeline for query: \"buffers\"");
        assert_eq!(
            lines[1],
            format!("**Anchor:** {} - Reader buffers", d.cards[1])
        );
        assert_eq!(
            lines[2],
            "**Window:** 1 records before -> 1 records after | **Items:** 3"
        );
        assert!(
            got.contains(&format!(
                "| {} | \" | ○ | Reader buffers <- **ANCHOR** |",
                d.cards[1]
            )),
            "{got}"
        );
        let none = Query {
            text: "zeppelin".into(),
            ..q
        };
        let err = text(&d.s, Around::Query(none), (1, 1)).unwrap_err();
        assert_eq!(err.to_string(), "nothing matches \"zeppelin\" to anchor on");
    }

    /// Test 3 (T2, Q3): a hidden card (K4), a skipped summary and a forgotten claim are no entries
    /// and take no place; an anchor on the hidden card is an error, as `get` reports it.
    #[test]
    fn hidden_items_take_no_place() {
        let mut d = days();
        let gone = d.s.said("gone", R, DAY + 1_000, "Old notes.");
        let hidden = d.s.cards(
            gone,
            gone,
            json!([{"type": "bugfix", "title": "Hidden card"}]),
            false,
        )[0]
        .clone();
        d.s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 2_000, "from": gone, "through": gone, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Skipped turn"}, "skipped": true}));
        d.s.run();
        let device = d.s.raw.device().to_owned();
        d.s.raw
            .append_tombstone(crate::raw::Target::Record { device, seq: gone })
            .unwrap();
        let home = d.s.home.path();
        let target = crate::forget::Target::parse_uid(&d.later[0].1).unwrap();
        crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
        d.s.run();
        // Before the summary, the card before the hidden ones; after it, the prompt that said the
        // forgotten decision (the record stays, D5) and the next prompt, not the decision.
        let got = text(&d.s, Around::Anchor(d.summary.clone()), (1, 2)).unwrap();
        for gone in [
            "Hidden card",
            "Skipped turn",
            "Old notes",
            &d.later[0].1[..12],
        ] {
            assert!(!got.contains(gone), "{gone}: {got}");
        }
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(
            lines[1],
            "**Window:** 1 records before -> 2 records after | **Items:** 4"
        );
        let shown = [
            format!("| {} |", d.cards[2]),
            format!("**💬 Prompt {}**", d.later[0].0),
            format!("**💬 Prompt {}**", d.later[1].0),
        ];
        for line in shown {
            assert!(got.contains(&line), "{line}: {got}");
        }
        for anchor in [hidden, d.later[0].1.clone()] {
            let err = text(&d.s, Around::Anchor(anchor.clone()), (1, 1)).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("no document {anchor} to anchor on")
            );
        }
    }

    /// Test 3 (spec 6.4): a rule added after the entries were written masks their lines, the
    /// card's file among them. In a child process: egress reads the rules of the home
    /// `redact::set_home` names, the process's.
    #[test]
    fn a_rule_added_after_masks_every_line() {
        const HOME: &str = "OBOETE_TEST_TIMELINE_RULE_HOME";
        const SECRET: &str = "QUILL-7731";
        if let Ok(home) = std::env::var(HOME) {
            let home = std::path::PathBuf::from(home);
            crate::redact::set_home(&home).unwrap();
            let got = timeline_text(
                &home,
                Some(R),
                Path::new("/w"),
                Around::Newest(20),
                (3, 3),
                &chrono::Utc,
            )
            .unwrap();
            assert!(!got.contains(SECRET), "{got}");
            // The two prompts, the claim, the card's file and row, the summary.
            assert_eq!(got.matches("[REDACTED]").count(), 6, "{got}");
            return;
        }
        let mut s = Store::new();
        decided(&mut s, DAY, &format!("Use {SECRET} for the parser."));
        let seq = s.said("work", R, DAY + 1_000, &format!("Fix {SECRET} now."));
        s.cards(
            seq,
            seq,
            json!([{"type": "bugfix", "title": format!("Card {SECRET}"),
                "files_modified": [format!("/w/src/{SECRET}.rs")]}]),
            false,
        );
        s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 3_000, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": format!("Turn {SECRET}")}, "skipped": false}));
        s.run();
        std::fs::write(
            s.home.path().join("config.toml"),
            format!("[redaction]\nextra_rules = [{{ id = \"quill\", regex = '{SECRET}' }}]\n"),
        )
        .unwrap();
        let name = "search::b::timeline::tests::a_rule_added_after_masks_every_line";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name])
            .env(HOME, s.home.path())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{said}");
        assert!(said.contains("1 passed"), "{said}");
    }

    /// Test 4 (T1, T4): without an anchor, the newest entries, no window line; the viewer's list
    /// keeps today's entries.
    #[test]
    fn without_an_anchor_the_newest_entries() {
        let d = days();
        let got = text(&d.s, Around::Newest(3), (3, 3)).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "# Timeline: the newest 3");
        assert_eq!(lines[1], "");
        assert!(got.contains(&d.later[0].1[..12]) && got.contains(&d.later[1].0));
        assert!(!got.contains("Window") && !got.contains("Quartz"), "{got}");
        assert_eq!(text(&Store::new(), Around::Newest(3), (3, 3)).unwrap(), "");
        let listed = super::super::timeline(d.s.home.path(), Some(R), None, None, 20).unwrap();
        let classes: Vec<&str> = listed.iter().map(|i| i.class.as_str()).collect();
        assert_eq!(classes, ["claim"; 3]);
    }

    /// Test 5 (T3): a record anchor of another kind than a prompt gives the entries around its
    /// time, none marked, the entries of its own time before it; a query whose first hit it is
    /// names it; so does a card of another repository than the one asked; an anchor of nothing is
    /// an error.
    #[test]
    fn a_record_anchor_that_is_no_prompt_splits_at_its_time() {
        let mut d = days();
        let tool = d.s.event(
            "assistant",
            "work",
            (R, "main"),
            DAY,
            json!({"message": "Parser verified."}),
        );
        d.s.run();
        let key = d.s.key(tool);
        let got = text(&d.s, Around::Anchor(key.clone()), (1, 1)).unwrap();
        let q = Query {
            text: "verified".into(),
            caller: Some(R.into()),
            raw: RawArm::Below,
            limit: 1,
            ..Default::default()
        };
        let found = text(&d.s, Around::Query(q), (1, 1)).unwrap();
        assert!(
            found.starts_with(&format!(
                "# Timeline for query: \"verified\"\n**Anchor:** {key} - Unknown\n"
            )),
            "{found}"
        );
        // The anchor's window, under the query's two header lines.
        let window = |t: &str, header: usize| {
            costless(t)
                .lines()
                .skip(header)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(window(&found, 2), window(&got, 1));
        assert!(!got.contains("ANCHOR"), "{got}");
        assert!(
            got.starts_with(&format!("# Timeline around anchor: {key}\n")),
            "{got}"
        );
        assert!(got.contains("**Items:** 2"), "{got}");
        assert!(got.contains(&format!("| {} |", d.cards[2])), "{got}");
        assert!(
            got.contains(&format!("**🎯 {}** Quartz progress", d.summary)),
            "{got}"
        );
        let elsewhere = d.s.said("other", "github.com/o/other", DAY, "Elsewhere.");
        let card = d.s.cards(
            elsewhere,
            elsewhere,
            json!([{"type": "change", "title": "Other repository"}]),
            false,
        )[0]
        .clone();
        d.s.run();
        let other = text(&d.s, Around::Anchor(card.clone()), (1, 1)).unwrap();
        assert!(
            !other.contains("Other repository") && !other.contains("ANCHOR"),
            "{other}"
        );
        assert_eq!(
            window(&other, 1),
            window(&got, 1),
            "the same entries around the same time"
        );
        let err = text(&d.s, Around::Anchor("o9".into()), (1, 1)).unwrap_err();
        assert_eq!(err.to_string(), "no document o9 to anchor on");
    }
