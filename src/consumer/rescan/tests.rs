    use super::*;
    use crate::{raw, worker};

    fn tombstones(raw: &Raw) -> usize {
        raw.after(raw.device(), 0, 1000)
            .unwrap()
            .iter()
            .filter(|r| matches!(r.item, Item::Tombstone(_)))
            .count()
    }

    fn body(raw: &Raw, seq: i64) -> String {
        match &raw.after(raw.device(), seq - 1, 1).unwrap()[0].item {
            Item::Event(e) => e.body.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_new_rule_tombstones_old_records_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let old = raw
            .append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        // Masked at capture: no finding of its own again.
        let masked = raw
            .append(&raw::test_event(r#"{"prompt":"token=[REDACTED] here"}"#))
            .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0); // the rules the records were stored under
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert_eq!(body(&raw, old), r#"{"prompt":"deploy *********** now"}"#);
        assert_eq!(body(&raw, masked), r#"{"prompt":"token=[REDACTED] here"}"#);
        assert!(
            crate::search::raw(p, "acme-123", None, 5)
                .unwrap()
                .is_empty()
        );
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1); // once
        // Lost commits took the tombstone (MUST-M14): the rewind scans again from seq 1.
        let top = raw.max_seq().unwrap();
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top])
            .unwrap();
        assert!(body(&raw, old).contains("acme-123456"));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, old).contains("acme"));
    }

    /// Owner decision 43: a record stored while redaction is off, and read by the rescan then, is
    /// masked once redaction is on again.
    #[test]
    fn turning_redaction_on_again_masks_what_was_stored_while_it_was_off() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        worker::run_once(p).unwrap(); // the rescan's version is the bundled rules'
        std::fs::write(p.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        // As capture stores it while off: as typed.
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0);
        assert!(body(&raw, seq).contains(&key));
        std::fs::write(p.join("config.toml"), "[redaction]\nenabled = true\n").unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
    }

    /// Owner decision 43 (Codex on #502): rules turned off while a batch is read write no
    /// tombstone for it, so a record stored as typed under the switch off stays so.
    #[test]
    fn rules_turned_off_while_a_batch_is_read_write_no_tombstone() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        let config = p.join("config.toml");
        BEFORE_TOMBSTONES.set(Some(Box::new(move || {
            std::fs::write(&config, "[redaction]\nenabled = false\n").unwrap();
        })));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0);
        assert!(body(&raw, seq).contains(&key));
    }

    /// Owner decision 43 (CodeRabbit on #502): from its check of the rules to its last tombstone, a
    /// step holds what a save that changes them takes, so a save made in between waits until the
    /// tombstones of the rules it replaces are written.
    #[test]
    fn a_save_made_while_tombstones_are_written_waits_for_them() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_owned();
        let mut raw = raw::open(&p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        let saving = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (started, at) = (std::rc::Rc::clone(&saving), p.clone());
        WHILE_TOMBSTONES.set(Some(Box::new(move || {
            crate::dispatch::blocked(&at); // no earlier wait is this save's
            let home = at.clone();
            // A save of the page turning redaction off: the hold, then config.toml.
            *started.borrow_mut() = Some(std::thread::spawn(move || {
                let _held = crate::dispatch::exclusive(&home).unwrap();
                std::fs::write(home.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
            }));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !crate::dispatch::blocked(&at) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the save did not wait"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })));
        worker::run_once(&p).unwrap();
        saving.borrow_mut().take().unwrap().join().unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
        let config = std::fs::read_to_string(p.join("config.toml")).unwrap();
        assert!(config.contains("enabled = false"), "{config}");
    }

    /// Windows CI on #491: a hold that does not come within its wait (readers holding dispatch one
    /// after another) writes no tombstone and fails no worker run; the next pass masks the record.
    #[test]
    fn a_busy_hold_leaves_the_batch_to_the_next_pass() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        // A reader holds dispatch until the step has waited for it past its wait, then lets it go:
        // the first attempt is busy whatever the machine's speed (Codex on #507).
        let reader = crate::dispatch::Admission::shared(p).unwrap();
        crate::dispatch::blocked(p); // no earlier wait is this step's
        let at = p.to_owned();
        let released = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !crate::dispatch::blocked(&at) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the step never waited"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(HOLD_WAIT * 2);
            drop(reader);
        });
        worker::run_once(p).unwrap();
        released.join().unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
    }

    #[test]
    fn settings_that_do_not_load_neither_hold_the_worker_nor_skip_a_record() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(p.join("config.toml"), rule).unwrap();
        worker::run_once(p).unwrap(); // the rescan's version is this rule's
        // Stored under other rules (appended here without capture's gate).
        let seq = raw
            .append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        std::fs::write(p.join("config.toml"), "[redaction\n").unwrap();
        worker::run_once(p).unwrap(); // returns: the rescan passes over the batch
        assert_eq!(tombstones(&raw), 0);
        // The same rule back: the same version, and still the record passed over is scanned.
        std::fs::write(p.join("config.toml"), rule).unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(body(&raw, seq), r#"{"prompt":"deploy *********** now"}"#);
    }

    #[test]
    fn a_rule_anchored_to_a_field_matches_as_capture_would() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let seq = raw
            .append(&raw::test_event(
                r#"{"prompt":"deploy acme-123456","cwd":"/r"}"#,
            ))
            .unwrap();
        worker::run_once(p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}$' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(
            body(&raw, seq),
            r#"{"prompt":"deploy ***********","cwd":"/r"}"#
        );
    }

    /// A value a tombstone masks reads as stars (`Raw::after`), which a user's rule can take for a
    /// secret again: when the rules change, the rescan reads it again and adds no second tombstone
    /// over it (Codex on #535).
    #[test]
    fn a_tombstoned_value_is_not_tombstoned_again_when_the_rules_change() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let pin = format!("Zq8v{}", "N3kL7pW2"); // split: scanners
        raw.append(&raw::test_event(&format!(
            r#"{{"prompt":"pin={pin} here"}}"#
        )))
        .unwrap();
        let rule = r#"{ id = "pin", regex = 'pin=(\S+)' }"#;
        let rules = |more: &str| format!("[redaction]\nextra_rules = [{rule}{more}]\n");
        std::fs::write(p.join("config.toml"), rules("")).unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, 1).contains(&pin), "{}", body(&raw, 1));
        let acme = r#", { id = "acme", regex = 'acme-[0-9]{6}' }"#;
        std::fs::write(p.join("config.toml"), rules(acme)).unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
    }

    /// #932: a rule that takes a secret whole finds what the rules before it left in a stored
    /// record, also over the tombstone an earlier rescan wrote inside it. The new range holds the
    /// old one and is no part of it, so it gets its tombstone: the record reads with all of it
    /// starred, its body is the JSON it was, and the next pass adds none.
    #[test]
    fn a_range_over_an_earlier_tombstone_is_tombstoned_and_reads_starred_whole() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let guid = |first: &str| format!("{first}-4e5f-6a7b-8c9d-{}", "0e1f2a3b4c5d");
        let (user, last) = (guid("9f8e7d6c"), guid("0a9b8c7d"));
        let key = format!("{}{}", "0123456789abcdef", "fedcba9876543210");
        // As a binary before `masked-remainder` stored a Teams webhook: five characters of the
        // first GUID masked and its end glued to the mask, which an earlier rescan then took.
        let url = "curl https://contoso.webhook.office.com/webhookb2/";
        let stored = format!(
            "{url}1a2b3c4d-4e5f-6a7b-{}0e1f2a3b4c5d@{user}/IncomingWebhook/{key}/{last} -v\nnext line",
            crate::redact::MASK
        );
        let event = serde_json::json!({ "prompt": stored }).to_string();
        let seq = raw.append(&raw::test_event(&event)).unwrap();
        let device = raw.device().to_owned();
        raw.append_tombstone(Target::Range {
            device,
            seq,
            offset: event.find("0e1f2a3b4c5d@").unwrap() as i64,
            length: 12,
        })
        .unwrap();
        assert!(body(&raw, seq).contains(&format!("{}************@", crate::redact::MASK)));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 2);
        let read = body(&raw, seq);
        for part in [user.as_str(), key.as_str(), last.as_str()] {
            assert!(!read.contains(part), "{read}");
        }
        let read: serde_json::Value = serde_json::from_str(&read).unwrap();
        let prompt = read["prompt"].as_str().unwrap();
        assert!(
            prompt.starts_with(&format!("{url}**")) && prompt.ends_with("** -v\nnext line"),
            "{prompt}"
        );
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 2); // once
    }

    /// #539: `covered` is by the tombstones between them, in any order, and by nothing less.
    #[test]
    fn covered_is_by_the_union_of_the_ranges_and_no_gap() {
        // (offset, length): 10..16 and 16..22 touch; 30..40 holds 32..34; 50..53 and 54..60 leave 53.
        let ranges = [(16, 6), (54, 6), (32, 2), (10, 6), (30, 10), (50, 3)];
        for (start, end, want) in [
            (10, 22, true),  // across the two that touch
            (12, 20, true),  // inside them
            (10, 23, false), // one byte past them
            (9, 16, false),  // one byte before the first
            (30, 40, true),  // the one that holds another
            (33, 40, true),
            (50, 60, false), // the gap of one byte
            (50, 53, true),
            (54, 60, true),
            (22, 30, false), // between tombstones
            (0, 5, false),   // before any
            (70, 80, false), // after all
        ] {
            assert_eq!(covered(&union(&ranges), start, end), want, "{start}..{end}");
        }
        assert!(!covered(&union(&[]), 0, 1));
        // A range that is no range covers nothing, and takes nothing from the others.
        assert!(covered(
            &union(&[(-4, 9), (14, 2), (16, -1), (i64::MAX, i64::MAX)]),
            14,
            16
        ));
        assert!(!covered(
            &union(&[(-4, 9), (14, 2), (16, -1), (i64::MAX, i64::MAX)]),
            0,
            5
        ));
        // The two that overlap, as a wider range over an earlier one is stored (#932).
        assert!(covered(&union(&[(5, 10), (0, 20)]), 0, 20));
        assert!(!covered(&union(&[(5, 10), (0, 20)]), 0, 21));
        // The union itself: in order, the touching and the overlapping ones made one.
        assert_eq!(union(&ranges), [(10, 22), (30, 40), (50, 53), (54, 60)]);
    }

    /// #539: two masks next to each other read as one run of stars to a rule added later. The
    /// range it finds is covered by the two tombstones between them, so it gets none of its own;
    /// one that reaches past them does.
    #[test]
    fn a_range_two_adjacent_tombstones_cover_is_not_tombstoned_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let event = r#"{"prompt":"id aaaaaabbbbbb end, id ccccccdddddd-7 end"}"#;
        let seq = raw.append(&raw::test_event(event)).unwrap();
        let device = raw.device().to_owned();
        for half in ["aaaaaa", "bbbbbb", "cccccc", "dddddd"] {
            raw.append_tombstone(Target::Range {
                device: device.clone(),
                seq,
                offset: event.find(half).unwrap() as i64,
                length: 6,
            })
            .unwrap();
        }
        assert!(body(&raw, seq).contains("id ************ end, id ************-7 end"));
        // The same text with one tombstone, elsewhere: each record's ranges are held against
        // its own tombstones.
        let other = raw.append(&raw::test_event(event)).unwrap();
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: other,
            offset: event.find("prompt").unwrap() as i64,
            length: 6,
        })
        .unwrap();
        let rule = r#"{ id = "id", regex = 'id (\S+) end' }"#;
        std::fs::write(
            p.join("config.toml"),
            format!("[redaction]\nextra_rules = [{rule}]\n"),
        )
        .unwrap();
        worker::run_once(p).unwrap();
        // The second value reaches two characters past its two tombstones: one more, for it.
        // The other record gets one for each value.
        assert_eq!(tombstones(&raw), 8);
        assert!(body(&raw, seq).contains("id ************ end, id ************** end"));
        assert!(body(&raw, other).contains("id ************ end, id ************** end"));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 8);
    }

    /// #538: a store last scanned by a build from before the matching code had a number holds
    /// that build's version, the rule files' hash. The same rules read by this build are another
    /// version, so the store is scanned again from its first record, and a value the older
    /// matching left is found. (Here it is left by rules that were off: there is no older
    /// matching to run.)
    #[test]
    fn a_store_scanned_before_the_matching_code_had_a_number_is_scanned_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        std::fs::write(p.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
        let pw = format!("Zq8v{}", "N3kL7pW2"); // split: scanners
        let event = format!(r#"{{"prompt":"curl -u admin:{pw} https://h.test"}}"#);
        let seq = raw.append(&raw::test_event(&event)).unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0);
        // The rules on again, and the row as the build before left it, at this record.
        std::fs::remove_file(p.join("config.toml")).unwrap();
        let k = crate::knowledge::open(p).unwrap();
        let before = crate::redact::version_before_matching();
        assert_eq!(
            k.execute("UPDATE rescan SET version = ?1", [&before])
                .unwrap(),
            1
        );
        drop(k);
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains(&pw), "{}", body(&raw, seq));
    }

    #[test]
    fn a_restore_that_skips_the_tombstone_masks_its_target_again() {
        // #83, raised on #110: the tombstone is sealed in a later segment than its target.
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        crate::backup::export(p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        crate::backup::export(p).unwrap();
        drop(raw);
        let later = std::fs::read_dir(p.join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|f| f.to_string_lossy().ends_with("-000000000002.seg.zst"))
            .unwrap();
        std::fs::write(&later, b"damaged").unwrap();
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        worker::run_once(p).unwrap(); // restores seq 1 only, then the rescan runs from seq 1
        let raw = raw::open(p).unwrap();
        assert!(!body(&raw, 1).contains("acme"), "{}", body(&raw, 1));
        assert!(
            crate::search::raw(p, "acme-123", None, 5)
                .unwrap()
                .is_empty()
        );
    }
