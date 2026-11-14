    use super::*;
    use crate::db;

    fn home(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("oboete-search-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn seed(conn: &mut Connection) {
        db::upsert_session(conn, "s1", "claude", "/r", "/r", 1_700_000_000_000).unwrap();
        db::apply_batch(
            conn,
            &db::PendingSession {
                id: "s1".into(),
                repo: "/r".into(),
                last_event_at: 1_700_000_000_000,
            },
            "test",
            "セッションの要約: 検索を実装した",
            &[
                db::Observation {
                    kind: "change".into(),
                    title: "src/db.rs を更新".into(),
                    body: "DB 接続とクエリを直した\n二行目".into(),
                },
                db::Observation {
                    kind: "decision".into(),
                    title: "use the trigram tokenizer".into(),
                    body: "FTS5 trigram indexes CJK text by character".into(),
                },
                db::Observation {
                    kind: "change".into(),
                    title: "ÉCOLE coverage".into(),
                    body: "coverage 50% done, ÄÖÜ İSTANBUL".into(),
                },
            ],
            i64::MAX,
        )
        .unwrap();
        db::insert_prompt(conn, "s1", 1_699_999_990_000, "trigram 検索を足して").unwrap();
    }

    #[test]
    fn older_database_without_fts_is_backfilled_once() {
        let dir = home("backfill");
        let mut conn = db::open(&dir).unwrap();
        seed(&mut conn);
        conn.execute_batch("DROP TABLE fts").unwrap();
        drop(conn);
        // Several agents' hooks can open the upgraded database at the same moment.
        let openers: Vec<_> = (0..4)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || db::open(&d).map(drop))
            })
            .collect();
        for h in openers {
            h.join().unwrap().unwrap();
        }
        let conn = db::open(&dir).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trigrams_skip_hiragana_split_at_punctuation_and_stop_at_64() {
        assert_eq!(trigrams("検索をしてください。"), ["検索を", "索をし"]);
        assert_eq!(trigrams("abcd, abc"), ["abc", "bcd"]);
        assert_eq!(trigrams("HTTP http"), ["HTT", "TTP"]);
        assert_eq!(trigrams("iii ııı III"), ["iii", "ııı"]);
        assert_eq!(trigrams("db 接続"), Vec::<String>::new());
        let long: String = ('a'..='z').cycle().take(200).collect();
        assert_eq!(trigrams(&long).len(), 26);
        let many: String = (0..100).map(|i| format!("x{i:02} ")).collect();
        assert_eq!(trigrams(&many).len(), 64);
    }

    #[test]
    fn raw_search_finds_events_by_a_two_character_japanese_query_and_an_english_word() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        let mut ja = crate::raw::test_event(r#"{"prompt":"同期の設計を見直して"}"#);
        ja.repo = Some("github.com/o/a".into());
        raw.append(&ja).unwrap();
        let mut en = crate::raw::test_event(r#"{"assistant":"the lease worker panicked"}"#);
        en.kind = "reply".into();
        en.repo = Some("github.com/o/b".into());
        raw.append(&en).unwrap();
        let mut k = crate::knowledge::open(p).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        let device = raw.device().to_owned();

        let hits = raw_search(p, "設計", None);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].device.as_str(), hits[0].seq), (device.as_str(), 1));
        assert!(hits[0].snippet.contains("設計"), "{}", hits[0].snippet);
        let hits = raw_search(p, "worker panicked", None);
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), [2]);
        assert_eq!(hits[0].kind, "reply");
        // Keys are not text: "prompt" and "assistant" are field names here.
        assert!(raw_search(p, "prompt", None).is_empty());
        assert!(raw_search(p, "assistant", None).is_empty());
        // This repository unless --all.
        assert!(raw_search(p, "設計", Some("github.com/o/b")).is_empty());
        assert_eq!(raw_search(p, "設計", Some("github.com/o/a")).len(), 1);
        let repos: Vec<String> = k
            .prepare("SELECT repo FROM session_repos ORDER BY repo")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(repos, ["github.com/o/a", "github.com/o/b"]);
    }

    #[test]
    fn a_short_japanese_word_boosts_raw_hits_beside_a_long_word() {
        for (both_at, long_at) in [(1_000, 2_000), (2_000, 1_000)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            let mut store = crate::raw::open(p).unwrap();
            for (ts, body) in [
                (both_at, "設計 worker"),
                (long_at, "worker"),
                (3_000, "設計"),
            ] {
                store
                    .append(&crate::raw::Event {
                        ts,
                        repo: Some("github.com/o/r".into()),
                        ..crate::raw::test_event(body)
                    })
                    .unwrap();
            }
            crate::worker::run_once(p).unwrap();
            let seqs = |text| -> Vec<i64> {
                raw_search(p, text, Some("github.com/o/r"))
                    .iter()
                    .map(|h| h.seq)
                    .collect()
            };
            assert_eq!(seqs("worker"), [2, 1]);
            assert_eq!(seqs("worker absent"), [2, 1]);
            assert_eq!(seqs("設 worker"), [2, 1]);
            assert_eq!(seqs("設計"), [3, 1]);
            assert_eq!(seqs("設計 worker"), [1, 2]);
        }
    }

    /// A short word of ASCII letters only says nothing as a substring ("is" in "this"): it
    /// reorders nothing, as before short words counted.
    #[test]
    fn a_short_word_of_ascii_letters_reorders_nothing() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for (ts, body) in [(1_000, "this worker"), (2_000, "worker")] {
            store
                .append(&crate::raw::Event {
                    ts,
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        for text in ["worker", "is worker", "IS worker"] {
            let seqs: Vec<i64> = raw_search(p, text, Some("github.com/o/r"))
                .iter()
                .map(|h| h.seq)
                .collect();
            assert_eq!(seqs, [2, 1], "{text}");
        }
    }

    /// Records with `bodies` in one repository, indexed: the seqs a query finds, the best first.
    fn found(bodies: &[&str], query: &str) -> Vec<i64> {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for (n, body) in bodies.iter().enumerate() {
            store
                .append(&crate::raw::Event {
                    ts: 1_000 * (n as i64 + 1),
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        raw(p, query, Some("github.com/o/r"), 10)
            .unwrap()
            .iter()
            .map(|h| h.seq)
            .collect()
    }

    /// A hit below the best `POOL` keeps its place however many hits are asked for: the short
    /// words reorder only the pool (Codex on #360: a limit over `POOL` reordered every hit).
    #[test]
    fn a_hit_below_the_pool_keeps_its_place_under_a_larger_limit() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        // The first and longest ranks last.
        for n in 0..=POOL {
            let body = if n == 0 { "設計 worker" } else { "worker" };
            store
                .append(&crate::raw::Event {
                    ts: 1_000 * (n as i64 + 1),
                    repo: Some("github.com/o/r".into()),
                    ..crate::raw::test_event(body)
                })
                .unwrap();
        }
        crate::worker::run_once(p).unwrap();
        let seqs: Vec<i64> = raw(p, "設計 worker", Some("github.com/o/r"), 2 * POOL)
            .unwrap()
            .iter()
            .map(|h| h.seq)
            .collect();
        assert_eq!(seqs.len(), POOL + 1);
        assert_eq!((seqs[0], seqs[POOL]), (POOL as i64 + 1, 1));
    }

    /// A word of hiragana only has no trigram at any length (particles and endings), and says as
    /// little as a substring: it reorders nothing (Codex on #360).
    #[test]
    fn a_hiragana_word_reorders_nothing() {
        for (body, query) in [
            ("worker ください", "worker ください"),
            ("worker こと", "worker こと"),
            ("worker について", "について worker"),
        ] {
            assert_eq!(found(&[body, "worker"], query), [2, 1], "{query}");
        }
    }

    /// A short word is a run between whitespace and punctuation, as a trigram's is.
    #[test]
    fn a_short_word_ends_at_punctuation() {
        assert_eq!(found(&["設計 worker", "worker"], "worker"), [2, 1]);
        assert_eq!(found(&["設計 worker", "worker"], "worker、設計。"), [1, 2]);
    }

    /// Tool output stays in raw.db, out of the index (#317, decision 42): neither a word with a
    /// trigram nor a short one finds it, before a tombstone's reindex or after, while the prompt
    /// beside it is found; its `raw_docs` row keeps its labels.
    #[test]
    fn tool_output_is_kept_but_not_searched() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let prompt = store
            .append(&crate::raw::Event {
                ts: 1_000,
                ..crate::raw::test_event(r#"{"prompt":"設計 lantern"}"#)
            })
            .unwrap();
        let tool = store
            .append(&crate::raw::Event {
                kind: "tool".into(),
                ts: 2_000,
                ..crate::raw::test_event(r#"{"tool":"Bash","input":"cat","output":"設計 lantern"}"#)
            })
            .unwrap();
        let indexed = || -> Vec<i64> {
            let k = crate::knowledge::open(p).unwrap();
            k.prepare(
                "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid
                 WHERE raw_fts MATCH 'lantern' ORDER BY d.seq",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        crate::worker::run_once(p).unwrap();
        assert_eq!(indexed(), [prompt]);
        let key = format!("{}:{prompt}", store.device());
        for q in ["lantern", "設計"] {
            let seqs: Vec<i64> = raw_search(p, q, None).iter().map(|h| h.seq).collect();
            assert_eq!(seqs, [prompt], "{q}");
            // The scan of a short word reads no tool output either, not only the answer shows none.
            let k = crate::knowledge::open(p).unwrap();
            let raw = crate::raw::open(p).unwrap();
            let order = raw_order(Some(&raw), &k, q, None, (None, None), None, None, 10).unwrap();
            assert_eq!(order, std::slice::from_ref(&key), "{q}");
        }
        let device = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Range {
                device,
                seq: tool,
                offset: 0,
                length: 2,
            })
            .unwrap();
        crate::worker::run_once(p).unwrap();
        assert_eq!(indexed(), [prompt]);
        let k = crate::knowledge::open(p).unwrap();
        let kind: String = k
            .query_row("SELECT kind FROM raw_docs WHERE seq = ?1", [tool], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "tool");
    }

    /// A query of short words alone looks for them in the text raw.db holds now (#317): written
    /// as a `\u` escape too, ASCII letters in either case, never in a key, never under a mask.
    #[test]
    fn a_short_word_is_found_in_the_text_raw_db_holds_now() {
        let none = Vec::<i64>::new();
        assert_eq!(
            found(&[r#"{"prompt":"\u8a2d\u8a08を見直す"}"#, "worker"], "設計"),
            [1]
        );
        assert_eq!(found(&["the M5 plan", "worker"], "m5"), [1]);
        assert_eq!(found(&["the m5 plan", "worker"], "M5"), [1]);
        assert_eq!(found(&[r#"{"設計":"worker"}"#], "設計"), none);
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let seq = store.append(&crate::raw::test_event("M5 plan")).unwrap();
        crate::worker::run_once(p).unwrap();
        assert_eq!(raw_search(p, "M5", None).len(), 1);
        let device = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Range {
                device,
                seq,
                offset: 0,
                length: 2,
            })
            .unwrap();
        assert!(raw_search(p, "M5", None).is_empty());
        crate::worker::run_once(p).unwrap();
        assert!(raw_search(p, "M5", None).is_empty());
        assert_eq!(raw_search(p, "plan", None).len(), 1);
    }

    /// A query of short words alone reads the records a page at a time, the newest first, until
    /// it has enough: one past the first page is found.
    #[test]
    fn a_short_word_is_found_past_the_first_page() {
        let mut bodies = vec!["worker"; 300];
        bodies[0] = "設計 old";
        bodies[299] = "設計 new";
        assert_eq!(found(&bodies, "設計"), [300, 1]);
    }

    /// The short words that count are few and counted once: a pasted page is still one query
    /// that SQLite can prepare (Codex on #360: 1,000 of them made an expression too deep).
    #[test]
    fn a_query_of_a_thousand_short_words_still_runs() {
        let repeated = format!("worker {}", "M5 ".repeat(1_000));
        assert_eq!(found(&["M5 worker", "worker"], &repeated), [1, 2]);
        let distinct: String = (0..1_100)
            .map(|i| format!("{}1 ", char::from_u32(0x4e00 + i).unwrap()))
            .collect();
        assert_eq!(
            found(&["M5 worker", "worker"], &format!("worker M5 {distinct}")),
            [1, 2]
        );
    }

    #[test]
    fn a_rewound_record_leaves_the_raw_index() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("kept lease note"))
            .unwrap();
        raw.append(&crate::raw::test_event("lost lease note"))
            .unwrap();
        let mut k = crate::knowledge::open(p).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        drop(raw);
        let c = Connection::open(p.join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > 1", []).unwrap();
        drop(c);
        let raw = crate::raw::open(p).unwrap();
        crate::knowledge::checkpoint::rewind(&raw, &k, &mut consumers).unwrap();
        let hits = raw_search(p, "lease note", None);
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), [1]);
    }

    /// #506: `session_start` keeps each session's first record with a repository, by time, then
    /// device and seq, as the records are read: an older record read later (an import of
    /// transcripts after the hooks' records) moves it, and a rewind or a forget that takes the
    /// first record away gives it to the next. A store from before the table fills it from the
    /// records it read. Each record's text keeps its own row in the raw index, though the table
    /// makes rows between.
    #[test]
    fn session_start_keeps_where_each_session_started() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        let event = |session: &str, repo: &str, ts: i64, text: &str| crate::raw::Event {
            session: session.into(),
            repo: Some(repo.into()),
            ts,
            ..crate::raw::test_event(text)
        };
        let (one, two) = ("github.com/o/one", "github.com/o/two");
        raw.append(&event("a", one, 2_000, "amber lantern"))
            .unwrap();
        raw.append(&event("a", two, 3_000, "amber quiver")).unwrap();
        raw.append(&event("b", two, 4_000, "cobalt zephyr"))
            .unwrap();
        let mut k = crate::knowledge::open(p).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        let starts = |k: &Connection| -> Vec<(String, String, i64)> {
            k.prepare("SELECT session, repo, seq FROM session_start ORDER BY session")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let row = |session: &str, repo: &str, seq: i64| (session.to_owned(), repo.to_owned(), seq);
        assert_eq!(starts(&k), [row("a", one, 1), row("b", two, 3)]);
        let seqs = |q: &str| {
            raw_search(p, q, None)
                .iter()
                .map(|h| h.seq)
                .collect::<Vec<_>>()
        };
        assert_eq!(seqs("zephyr"), [3]);
        raw.append(&event("b", one, 1_000, "cobalt harbor"))
            .unwrap();
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(starts(&k), [row("a", one, 1), row("b", one, 4)]);
        // A restore that lost the earlier record: the rewind gives b back its first one.
        drop(raw);
        let c = Connection::open(p.join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > 3", []).unwrap();
        drop(c);
        let mut raw = crate::raw::open(p).unwrap();
        crate::knowledge::checkpoint::rewind(&raw, &k, &mut consumers).unwrap();
        assert_eq!(starts(&k), [row("a", one, 1), row("b", two, 3)]);
        // a's first record forgotten: a started where its next record was.
        let device = raw.device().to_owned();
        raw.append_tombstone(crate::raw::Target::Record { device, seq: 1 })
            .unwrap();
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(starts(&k), [row("a", two, 2), row("b", two, 3)]);
        // A store from before the table: the same rows, from the records it read.
        k.execute_batch("DROP TABLE session_start").unwrap();
        crate::consumer::fts::schema(&k).unwrap();
        assert_eq!(starts(&k), [row("a", two, 2), row("b", two, 3)]);
        assert_eq!(seqs("quiver"), [2]);
    }

    /// docs/retention.md: an expired record keeps its labels' row and where its session started,
    /// in a rebuild and when a range tombstone from before is read again, and is found by no
    /// search; a forget of it takes the row.
    #[test]
    fn an_expired_record_keeps_where_its_session_started() {
        use crate::worker::Consumer;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        let repo = "github.com/o/one";
        let event = |ts: i64, text: &str| crate::raw::Event {
            session: "a".into(),
            repo: Some(repo.into()),
            ts,
            ..crate::raw::test_event(text)
        };
        let first = raw.append(&event(1_000, "amber lantern")).unwrap();
        raw.append(&event(2_000, "amber quiver")).unwrap();
        let device = raw.device().to_owned();
        raw.append_tombstone(crate::raw::Target::Range {
            device: device.clone(),
            seq: first,
            offset: 0,
            length: 2,
        })
        .unwrap();
        raw.expire_for_test(&[first]);
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(crate::consumer::fts::Fts)];
        // Read as a rebuild reads it: the expired record, then the tombstone before it expired.
        let mut k = crate::knowledge::open(p).unwrap();
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        let start = |k: &Connection| -> (String, i64) {
            k.query_row(
                "SELECT repo, seq FROM session_start WHERE session = 'a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(start(&k), (repo.to_owned(), first));
        let docs: i64 = k
            .query_row("SELECT COUNT(*) FROM raw_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(docs, 2);
        let seqs = |q: &str| {
            raw_search(p, q, None)
                .iter()
                .map(|h| h.seq)
                .collect::<Vec<_>>()
        };
        assert!(seqs("lantern").is_empty());
        assert_eq!(seqs("quiver"), [first + 1]);
        // A forget of the expired record: its session starts at its next record.
        raw.append_tombstone(crate::raw::Target::Record { device, seq: first })
            .unwrap();
        crate::worker::drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(start(&k), (repo.to_owned(), first + 1));
        let docs: Vec<i64> = k
            .prepare("SELECT seq FROM raw_docs")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(docs, [first + 1]);
    }

    #[test]
    fn a_record_tombstone_hides_it_and_a_range_tombstone_masks_only_its_range() {
        use crate::raw::{Item, Target};
        // Tombstones that arrive before the index reads the records, and after it has.
        for index_first in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            let mut raw = crate::raw::open(p).unwrap();
            let a = raw
                .append(&crate::raw::test_event("alpha zqx-private-words tail"))
                .unwrap();
            let b = raw
                .append(&crate::raw::test_event("bravo visible"))
                .unwrap();
            let dev = raw.device().to_owned();
            if index_first {
                crate::worker::run_once(p).unwrap();
                assert_eq!(raw_search(p, "bravo", None).len(), 1);
            }
            raw.append_tombstone(Target::Record {
                device: dev.clone(),
                seq: b,
            })
            .unwrap();
            raw.append_tombstone(Target::Range {
                device: dev.clone(),
                seq: a,
                offset: 6,
                length: 17,
            })
            .unwrap();
            // Hidden before the index reaches the tombstones too.
            assert!(raw_search(p, "bravo", None).is_empty());
            assert!(raw_search(p, "zqx-private", None).is_empty());
            crate::worker::run_once(p).unwrap();
            assert!(raw_search(p, "bravo", None).is_empty());
            assert!(raw_search(p, "zqx-private", None).is_empty());
            let hits = raw_search(p, "alpha", None);
            assert_eq!(hits.len(), 1);
            assert!(hits[0].snippet.contains("tail") && !hits[0].snippet.contains("zqx"));
            let recs = raw.after(&dev, 0, 10).unwrap();
            assert!(
                matches!(&recs[0].item, Item::Event(e) if e.body == "alpha ***************** tail")
            );
            assert!(matches!(recs[1].item, Item::Removed));
        }
    }

    #[test]
    fn a_copied_home_still_hides_what_an_old_device_tombstoned() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        let seq = store
            .append(&crate::raw::test_event("copied lantern"))
            .unwrap();
        crate::worker::run_once(p).unwrap();
        let old = store.device().to_owned();
        store
            .append_tombstone(crate::raw::Target::Record { device: old, seq })
            .unwrap();
        drop(store);
        // The copy gets a new device id: the tombstone stays under the old one, unindexed.
        let copy = tempfile::tempdir().unwrap();
        for f in ["raw.db", "knowledge.db"] {
            std::fs::copy(p.join(f), copy.path().join(f)).unwrap();
        }
        let moved = crate::raw::open(copy.path()).unwrap();
        assert_eq!(moved.devices().unwrap().len(), 1);
        assert!(raw_search(copy.path(), "lantern", None).is_empty());
    }

    #[test]
    fn a_hidden_hit_never_takes_the_place_of_a_visible_one() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut store = crate::raw::open(p).unwrap();
        for body in ["shared lantern one", "shared lantern two"] {
            store.append(&crate::raw::test_event(body)).unwrap();
        }
        crate::worker::run_once(p).unwrap();
        let both = raw_search(p, "lantern", None);
        assert_eq!(both.len(), 2);
        store
            .append_tombstone(crate::raw::Target::Record {
                device: both[0].device.clone(),
                seq: both[0].seq,
            })
            .unwrap();
        // The index has not reached the tombstone: the top hit is hidden, the next one shown.
        let one = raw(p, "lantern", None, 1).unwrap();
        assert_eq!(one.iter().map(|h| h.seq).collect::<Vec<_>>(), [both[1].seq]);
    }

    fn raw_search(home: &std::path::Path, q: &str, repo: Option<&str>) -> Vec<RawHit> {
        raw(home, q, repo, 10).unwrap()
    }

    #[test]
    fn snippet_shows_the_passage_with_the_most_terms() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let body = "aaaaaaaaaa bbbbbbbbbb cccccccccc TARGET dddddddddd eeeeeeeeee";
        let s = snippet(body, &t(&["zzz", "target"]), 24);
        assert!(
            s.contains("TARGET") && s.starts_with('…') && s.ends_with('…'),
            "{s}"
        );
        assert_eq!(snippet("short\nline", &t(&["nothing"]), 40), "short line");
        assert_eq!(snippet("日本語の本文です", &t(&["本文"]), 4), "…の本文で…");
        // A common trigram early on loses to the passage where the rarer ones meet.
        let body = format!(
            "the start {} the trigram tokenizer indexes CJK",
            "x".repeat(200)
        );
        let s = snippet(&body, &terms("the trigram tokenizer"), 40);
        assert!(s.contains("trigram tokenizer"), "{s}");
    }
