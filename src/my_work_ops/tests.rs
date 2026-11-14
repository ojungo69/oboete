    use super::*;
    use crate::raw::OpKind;
    use crate::search::b::fixture::Store;
    use crate::worker::Consumer;
    use serde_json::json;

    fn op(job: &str, about: &str, ts: i64, fields: Value) -> MyWorkOp {
        MyWorkOp {
            job: job.into(),
            about: about.into(),
            entry: "anthropic".into(),
            ts,
            read: Read {
                spans: vec![(1, 2)],
                ops: vec![3],
            },
            removed: vec![],
            fields: serde_json::from_value(fields).unwrap(),
            skipped: false,
            forgotten: None,
        }
    }

    fn stamp(path: &std::path::Path) -> i64 {
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// S3: the first My Work op raises the home to format 5 in the transaction that writes it,
    /// above a hide op's 4 in the same batch, and a rebuild from the backup's ops stamps it too.
    #[test]
    fn a_my_work_op_raises_the_home_to_the_format_that_reads_it() {
        let mut s = Store::new();
        let home = s.home.path().to_owned();
        s.run();
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::OPEN_FORMAT);
        let goals = serde_json::to_value(op("goals", "2026-10-26", 1, json!({}))).unwrap();
        s.raw
            .append_ops(&[(OpKind::Hide, json!({})), (OpKind::MyWork, goals)])
            .unwrap();
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::MY_WORK_FORMAT);
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
        assert_eq!(stamp(&rebuilt), crate::raw::MY_WORK_FORMAT);
    }

    /// S4: the newest op of a job and subject is read, whichever device wrote it and in whatever
    /// order the devices are read; a rewind of one device takes its ops away and the one before is
    /// read again; an op of a job this binary does not know is none.
    #[test]
    fn the_newest_op_of_a_job_and_subject_is_read_whichever_device_wrote_it() {
        for (older, newer) in [("aaaa", "zzzz"), ("zzzz", "aaaa")] {
            let s = Store::new();
            let put = |(device, op_seq): (&str, i64), w: &MyWorkOp| {
                rusqlite::Connection::open(s.home.path().join("raw.db"))
                    .unwrap()
                    .execute(
                        "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                         VALUES(?1, ?2, 'my_work', ?3, ?4, ?2)",
                        params![device, op_seq, w.ts, serde_json::to_string(w).unwrap()],
                    )
                    .unwrap();
            };
            // The older op has the higher op seq: only the time orders them.
            put(
                (older, 9),
                &op("review", "R1", 1_000, json!({"summary": "Old."})),
            );
            put(
                (newer, 1),
                &op("review", "R1", 2_000, json!({"summary": "New."})),
            );
            put((newer, 2), &op("unknown", "R1", 3_000, json!({})));
            s.run();
            let k = crate::knowledge::open(s.home.path()).unwrap();
            let kept = newest(&k, "review", "R1").unwrap().unwrap();
            let summary = &kept.op.fields["summary"];
            assert_eq!((kept.device.as_str(), summary), (newer, &json!("New.")));
            assert_eq!(kept.op.read, op("", "", 0, json!({})).read);
            assert_eq!(newest(&k, "unknown", "R1").unwrap(), None);
            crate::consumer::my_work::MyWork
                .rewind(&k, newer, 0)
                .unwrap();
            let kept = newest(&k, "review", "R1").unwrap().unwrap();
            assert_eq!(kept.op.fields["summary"], json!("Old."), "{older} {newer}");
        }
    }

    /// M2 (#719): a goals op over the cap, kept as a skip, still says that its read stopped at
    /// the budget; nothing else of its fields is kept.
    #[test]
    fn a_skip_kept_in_a_goals_ops_place_keeps_more() {
        let big = "x".repeat(crate::raw::MAX_OP_BYTES);
        let w = op(
            "goals",
            "2026-10-27",
            1,
            json!({"items": big, "more": true}),
        );
        let v = fitted(w).unwrap().unwrap();
        assert_eq!(
            (v["skipped"].clone(), v["fields"].clone()),
            (json!(true), json!({"more": true}))
        );
        let w = op("goals", "2026-10-27", 1, json!({"items": big}));
        assert_eq!(fitted(w).unwrap().unwrap()["fields"], json!({}));
    }

    /// S2: an op over the op cap loses its removals first, then is kept as a skip of its labels.
    #[test]
    fn an_op_over_the_cap_is_kept_without_its_removals_then_as_a_skip() {
        let big = "x".repeat(crate::raw::MAX_OP_BYTES);
        let mut w = op("review", "R1", 1, json!({"summary": "Kept."}));
        w.removed = (0..10_000).map(|i| (i, Some(i), Some(i))).collect();
        let v = fitted(w.clone()).unwrap().unwrap();
        assert_eq!(
            (v["removed"].clone(), v["fields"]["summary"].clone()),
            (json!([]), json!("Kept."))
        );
        w.fields.insert("summary".into(), json!(big));
        let v = fitted(w).unwrap().unwrap();
        assert_eq!(
            (v["skipped"].clone(), v["fields"].clone()),
            (json!(true), json!({}))
        );
        assert_eq!(
            (v["job"].clone(), v["about"].clone()),
            (json!("review"), json!("R1"))
        );
        let mut w = op("review", "R1", 1, json!({}));
        w.about = big;
        assert_eq!(
            fitted(w).unwrap(),
            None,
            "labels past the cap are kept nowhere"
        );
    }
