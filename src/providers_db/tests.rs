    use super::*;

    fn call(conn: &Connection, role: &str, outcome: &str) {
        let c = Call {
            provider: "p",
            role,
            span: "s",
            outcome,
            ms: 1,
            detail: None,
            bytes_out: 0,
            est_tokens: None,
            usage: Usage::default(),
            usd: None,
        };
        record(conn, &c).unwrap();
    }

    /// docs/my-work-model.md A7: the bound counts My Work's requests of every entry in a rolling
    /// day, as an entry's daily budget counts its own; a skip sent nothing.
    #[test]
    fn my_works_daily_count_is_its_roles_requests_in_a_rolling_day() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        for outcome in ["ok", "error", "shape", "reserved", "budget", "gate"] {
            call(&conn, "my_work", outcome);
        }
        call(&conn, "curator", "ok");
        call(&conn, "summary", "ok");
        let (calls, oldest) = my_work_calls_in_a_day(&conn).unwrap();
        assert_eq!(calls, 4);
        let first: i64 = conn
            .query_row(
                "SELECT MIN(ts) FROM provider_calls WHERE role = 'my_work' AND outcome = 'ok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(oldest, Some(first));
        conn.execute(
            "UPDATE provider_calls SET ts = ts - ?1 WHERE outcome = 'ok'",
            [DAY_MS + 1],
        )
        .unwrap();
        assert_eq!(my_work_calls_in_a_day(&conn).unwrap().0, 3);
        let plan: String = conn
            .query_row(
                &format!(
                    "EXPLAIN QUERY PLAN SELECT COUNT(*), MIN(ts) FROM provider_calls WHERE role = 'my_work'
                       AND ts >= 0 AND outcome IN {REQUESTS}"
                ),
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("provider_calls_my_work"), "{plan}");
    }

    /// docs/my-work-model.md A8: a waiting job is kept by its key, and `oboete resume` wakes one
    /// held for the owner, a saved key one held for time or the owner; a budget's wait stays.
    #[test]
    fn a_waiting_my_work_job_wakes_on_resume_and_a_saved_key() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        let later = now_ms() + 3_600_000;
        let wait = |key: &str, hold: &str| MyWorkPending {
            key: key.into(),
            prompt: "sha".into(),
            reason: "every provider failed".into(),
            hold: hold.into(),
            attempts: 1,
            next_attempt_at: later,
        };
        for (key, hold) in [
            ("group a", "owner"),
            ("review b", "time"),
            ("goals c", "budget"),
        ] {
            set_my_work_pending(&conn, &wait(key, hold)).unwrap();
        }
        assert_eq!(
            my_work_pending_of(&conn, "group a").unwrap(),
            Some(wait("group a", "owner"))
        );
        let next = |key: &str| {
            my_work_pending_of(&conn, key)
                .unwrap()
                .unwrap()
                .next_attempt_at
        };
        resume(&conn, "p").unwrap();
        assert_eq!(next("group a"), 0);
        assert!(next("review b") > 0);
        key_saved(&conn, &["p"]).unwrap();
        assert_eq!(next("review b"), 0);
        assert!(next("goals c") > 0, "a key lifts no cap");
        clear_my_work_pending(&conn, "group a").unwrap();
        assert_eq!(my_work_pending_of(&conn, "group a").unwrap(), None);
    }

    /// #730: a batch's held subjects wake as `my_work_pending` does: resume ends a wait for the
    /// owner, a saved key one for time or the owner; a budget's wait stays; a clear ends one.
    #[test]
    fn held_subjects_wake_on_resume_and_a_saved_key() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        let later = now_ms() + 3_600_000;
        for (about, hold) in [("a", "owner"), ("b", "time"), ("c", "budget")] {
            set_my_work_held(&conn, ("links", about), hold, later).unwrap();
        }
        let held = |conn: &Connection| -> Vec<String> {
            let rows = my_work_held(conn, now_ms()).unwrap();
            rows.into_iter().map(|(_, about, ..)| about).collect()
        };
        assert_eq!(held(&conn), ["a", "b", "c"]);
        resume(&conn, "p").unwrap();
        assert_eq!(held(&conn), ["b", "c"]);
        key_saved(&conn, &["p"]).unwrap();
        assert_eq!(held(&conn), ["c"], "a key lifts no cap");
        clear_my_work_held(&conn, ("links", "c")).unwrap();
        assert!(held(&conn).is_empty());
    }

    #[test]
    fn probes_consume_limits_without_recalibrating_curation() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        for (role, count, est, prompt, completion, usd) in [
            ("curator", 5, 100, 200, 20, 0.02),
            ("probe", 6, 200, 40, 1, 0.01),
        ] {
            for _ in 0..count {
                record(
                    &conn,
                    &Call {
                        provider: "p",
                        role,
                        span: "synthetic",
                        outcome: "ok",
                        ms: 1,
                        detail: None,
                        bytes_out: 30,
                        est_tokens: Some(est),
                        usage: Usage {
                            prompt: Some(prompt),
                            completion: Some(completion),
                            ..Usage::default()
                        },
                        usd: Some(usd),
                    },
                )
                .unwrap();
            }
        }
        assert_eq!(crate::budget::factor(&conn, "p").unwrap(), 2.0);
        assert_eq!(calls_in_a_day(&conn, "p").unwrap().0, 11);
        assert_eq!(tokens_since(&conn, "p", 0).unwrap().0, 1346);
        assert!((usd_this_month(&conn).unwrap() - 0.16).abs() < 1e-9);
        assert_eq!(last_calls(&conn, 20).unwrap().len(), 11);
    }

    #[test]
    fn an_inflight_curation_keeps_only_metadata_and_its_current_generation() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let reader = Connection::open_with_flags(
            home.path().join("providers.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let hash = "a".repeat(64);
        let started = now_ms();
        let old = start_curation(&db, "device-a", (7, Some(12)), (9, Some(34)), &hash).unwrap();
        let row: (String, i64, i64, i64, i64, i64, i64, String) = reader
            .query_row(
                "SELECT request_id, pid, started_at, from_seq, from_offset, to_seq, to_offset,
                   prompt_sha256 FROM curation_inflight WHERE device='device-a'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0.len(), 32);
        assert!(row.0.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(row.1, i64::from(std::process::id()));
        assert!((started..=now_ms()).contains(&row.2));
        assert_eq!((row.3, row.4, row.5, row.6), (7, 12, 9, 34));
        assert_eq!(row.7, hash);
        let columns: Vec<String> = reader
            .prepare("PRAGMA table_info(curation_inflight)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "device",
                "request_id",
                "pid",
                "started_at",
                "from_seq",
                "from_offset",
                "to_seq",
                "to_offset",
                "prompt_sha256"
            ]
        );
        let another_writer = open(home.path()).unwrap();
        let newer = start_curation(
            &another_writer,
            "device-a",
            (9, Some(34)),
            (11, None),
            &hash,
        )
        .unwrap();
        let other = start_curation(&db, "device-b", (7, None), (9, None), &hash).unwrap();
        drop(old);
        let kept: (String, i64, Option<i64>) = reader
            .query_row(
                "SELECT request_id, from_seq, to_offset FROM curation_inflight WHERE device='device-a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_ne!(kept.0, row.0);
        assert_eq!((kept.1, kept.2), (9, None));
        drop(newer);
        let devices: Vec<String> = reader
            .prepare("SELECT device FROM curation_inflight")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(devices, ["device-b"]);
        drop(other);
        let left: i64 = reader
            .query_row("SELECT count(*) FROM curation_inflight", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert!(last_calls(&db, 10).unwrap().is_empty());
    }

    #[test]
    fn schema_upgrade_is_atomic_and_keeps_the_cooldown_and_call_ledger() {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        set_state(
            &writer,
            "kept",
            State {
                down_until: OWNER_HOLD,
                fails: 2,
                backoff: 1,
                ..Default::default()
            },
        )
        .unwrap();
        record(
            &writer,
            &Call {
                provider: "kept",
                role: "curator",
                span: "schema-test",
                outcome: "ok",
                ms: 7,
                detail: None,
                bytes_out: 1,
                est_tokens: None,
                usage: Usage::default(),
                usd: Some(0.25),
            },
        )
        .unwrap();
        let missing =
            "DROP TABLE key_limits; DROP INDEX provider_calls_day; DROP TABLE curation_inflight;";
        writer.execute_batch(missing).unwrap();
        crate::crash::off();
        let reopened = open(home.path()).unwrap();
        assert_eq!(crate::crash::count(), 1);
        reopened.execute_batch(missing).unwrap();
        crate::crash::at(1);
        let failed = open(home.path());
        crate::crash::off();
        assert!(failed.is_err());
        let remaining: i64 = writer.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('key_limits', 'provider_calls_day', 'curation_inflight')",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(remaining, 0);
        let recovered = open(home.path()).unwrap();
        assert_eq!(state(&recovered, "kept").unwrap().down_until, OWNER_HOLD);
        let kept: (i64, f64) = recovered
            .query_row(
                "SELECT count(*), SUM(usd) FROM provider_calls WHERE provider='kept'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (1, 0.25));
    }

    #[test]
    fn a_non_lock_schema_error_returns_immediately_without_partial_tables() {
        let home = tempfile::tempdir().unwrap();
        let writer = Connection::open(home.path().join("providers.db")).unwrap();
        crate::db::wal(&writer, "NORMAL").unwrap();
        writer
            .execute_batch(
                "CREATE TABLE provider_calls_day(kept TEXT);
                              INSERT INTO provider_calls_day VALUES ('unchanged');",
            )
            .unwrap();
        let started = std::time::Instant::now();
        let error = open(home.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("already a table named provider_calls_day"),
            "{error:#}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let tables: i64 = writer
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1);
        assert_eq!(
            writer
                .query_row("SELECT kept FROM provider_calls_day", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "unchanged"
        );
    }

    /// `oboete resume` makes a window that waited on the owner due now; one that waits on time
    /// keeps its time.
    #[test]
    fn resume_makes_a_window_waiting_on_the_owner_due() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        let row = |device: &str, hold: &str| Pending {
            device: device.into(),
            from_seq: 1,
            from_offset: None,
            to_seq: 2,
            to_offset: None,
            reason: "r".into(),
            hold: hold.into(),
            attempts: 0,
            next_attempt_at: 5_000_000_000_000,
            since: 1,
            prompt: "p".into(),
        };
        set_pending(&conn, &row("a", "owner")).unwrap();
        set_pending(&conn, &row("b", "time")).unwrap();
        resume(&conn, "claude").unwrap();
        let due: Vec<i64> = pending(&conn)
            .unwrap()
            .iter()
            .map(|p| p.next_attempt_at)
            .collect();
        assert_eq!(due, [0, 5_000_000_000_000]);
        // A turn's summary that waits on the owner too.
        let turn = TurnPending {
            device: "a".into(),
            seq: 7,
            agent: "claude".into(),
            session: "s".into(),
            prompt: "p".into(),
            reason: "r".into(),
            hold: "owner".into(),
            attempts: 0,
            next_attempt_at: 5_000_000_000_000,
        };
        set_turn_pending(&conn, &turn).unwrap();
        resume(&conn, "claude").unwrap();
        let got = turn_pending_of(&conn, "a", 7).unwrap();
        assert_eq!(got.unwrap().next_attempt_at, 0);
    }

    /// Parity row 11: a saved key ends the rests of the entries named, and wakes what waits on
    /// time or on the owner, windows and turns alike; a wait on a budget stays.
    #[test]
    fn a_saved_key_ends_its_entries_rests_and_wakes_waits_on_time_or_the_owner() {
        let home = tempfile::tempdir().unwrap();
        let conn = open(home.path()).unwrap();
        let later = 5_000_000_000_000;
        let rest = State {
            down_until: later,
            backoff: 3,
            ..Default::default()
        };
        for p in ["groq", "groq-20b", "nim"] {
            set_state(&conn, p, rest).unwrap();
        }
        let row = |device: &str, hold: &str| Pending {
            device: device.into(),
            from_seq: 1,
            from_offset: None,
            to_seq: 2,
            to_offset: None,
            reason: "r".into(),
            hold: hold.into(),
            attempts: 0,
            next_attempt_at: later,
            since: 1,
            prompt: "p".into(),
        };
        for (device, hold) in [("a", "time"), ("b", "owner"), ("c", "budget")] {
            set_pending(&conn, &row(device, hold)).unwrap();
        }
        let turn = |seq: i64, hold: &str| TurnPending {
            device: "a".into(),
            seq,
            agent: "claude".into(),
            session: "s".into(),
            prompt: "p".into(),
            reason: "r".into(),
            hold: hold.into(),
            attempts: 0,
            next_attempt_at: later,
        };
        set_turn_pending(&conn, &turn(7, "time")).unwrap();
        set_turn_pending(&conn, &turn(8, "budget")).unwrap();
        key_saved(&conn, &["groq", "groq-20b"]).unwrap();
        assert_eq!(state(&conn, "groq").unwrap(), State::default());
        assert_eq!(state(&conn, "groq-20b").unwrap(), State::default());
        assert_eq!(state(&conn, "nim").unwrap(), rest);
        let due: Vec<(String, i64)> = (pending(&conn).unwrap().into_iter())
            .map(|p| (p.hold, p.next_attempt_at))
            .collect();
        let held = |hold: &str, at: i64| (hold.to_owned(), at);
        assert_eq!(
            due,
            [held("time", 0), held("owner", 0), held("budget", later)]
        );
        let turn_at = |seq| {
            turn_pending_of(&conn, "a", seq)
                .unwrap()
                .unwrap()
                .next_attempt_at
        };
        assert_eq!((turn_at(7), turn_at(8)), (0, later));
    }

    #[test]
    fn the_month_starts_on_the_first_at_midnight_utc() {
        // 2026-09-27T02:52:53Z -> 2026-09-01T00:00:00Z; 2024-03-01 after a leap day.
        assert_eq!(
            chrono_free_month_start(1_790_477_573_000),
            1_788_220_800_000
        );
        assert_eq!(
            chrono_free_month_start(1_709_251_200_000),
            1_709_251_200_000
        );
        assert_eq!(
            chrono_free_month_start(1_709_251_199_999),
            1_706_745_600_000
        );
    }
