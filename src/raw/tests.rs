    use super::*;

    /// What forget took is read from its denials outward, never by a scan of every record: with
    /// no statistics the planner put `records` first, and each My Work page read the whole of
    /// raw.db (about 3 s on a 4 GB store, 2026-11-01).
    #[test]
    fn forgotten_records_are_read_from_the_denials() {
        let home = tempfile::tempdir().unwrap();
        let raw = open(home.path()).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT o.device, o.seq FROM {FORGOTTEN_RECORDS}
             WHERE r.type = 'event' ORDER BY o.device, o.seq"
        );
        let plan: Vec<String> = (raw.conn.prepare(&sql).unwrap())
            .query_map([], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(plan[0].starts_with("SCAN d "), "{plan:?}");
        assert!(
            !plan.iter().any(|step| step.starts_with("SCAN r")),
            "{plan:?}"
        );
    }

    /// Parity row 31: the newest hook record's seq and time as the viewer reads them, a replayed
    /// record's never; none before a store exists.
    #[test]
    fn newest_hook_reads_the_hooks_last_record() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(newest_hook(home.path()).unwrap(), None);
        let mut raw = open(home.path()).unwrap();
        let a = raw
            .append(&Event {
                ts: 7_000,
                ..test_event("{}")
            })
            .unwrap();
        raw.append(&Event {
            source: "replay".into(),
            ..test_event("{}")
        })
        .unwrap();
        assert_eq!(newest_hook(home.path()).unwrap(), Some((a, 7_000)));
    }

    /// Parity row X1: a span is a subagent's work alone when its tool calls all carry an
    /// `agent_id` and nothing but session starts and ends stands beside them.
    #[test]
    fn subagent_only_reads_the_spans_tool_calls() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let ev = |kind: &str, body: serde_json::Value| Event {
            kind: kind.into(),
            ..test_event(&body.to_string())
        };
        let sub = || {
            ev(
                "tool",
                serde_json::json!({"tool": "Read", "agent_id": "a1"}),
            )
        };
        let start = raw.append(&ev("start", serde_json::json!({}))).unwrap();
        assert!(!raw.subagent_only(&dev, start, start).unwrap());
        let a = raw.append(&sub()).unwrap();
        let b = raw.append(&sub()).unwrap();
        assert!(raw.subagent_only(&dev, start, b).unwrap());
        let main = raw
            .append(&ev(
                "tool",
                serde_json::json!({"tool": "Read", "agent_id": ""}),
            ))
            .unwrap();
        assert!(!raw.subagent_only(&dev, a, main).unwrap());
        let c = raw.append(&sub()).unwrap();
        let reply = raw.append(&ev("reply", serde_json::json!({}))).unwrap();
        assert!(raw.subagent_only(&dev, c, c).unwrap());
        assert!(!raw.subagent_only(&dev, c, reply).unwrap());
    }

    /// X17 (docs/subagents.md): a subagent's stop beside its tool calls leaves the span its work
    /// alone, also once retention has emptied the stop.
    #[test]
    fn a_subagent_stop_leaves_a_span_the_subagents_work() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let ev = |kind: &str, body: serde_json::Value| Event {
            kind: kind.into(),
            ..test_event(&body.to_string())
        };
        let tool = (raw.append(&ev(
            "tool",
            serde_json::json!({"tool": "Read", "agent_id": "a1"}),
        )))
        .unwrap();
        let stop = (raw.append(&ev(
            SUBAGENT_STOP,
            serde_json::json!({"agent_id": "a1", "agent_type": "Explore"}),
        )))
        .unwrap();
        assert!(raw.subagent_only(&dev, tool, stop).unwrap());
        // A stop alone holds no tool call of the subagent's.
        assert!(!raw.subagent_only(&dev, stop, stop).unwrap());
        raw.expire_for_test(&[stop]);
        assert!(raw.subagent_only(&dev, tool, stop).unwrap());
    }

    /// X17 (docs/subagents.md): a subagent's stop, often of another session and after the work, is
    /// none of a span's labels: not its session, its repository or its time.
    #[test]
    fn a_subagent_stop_is_none_of_a_spans_labels() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let work = (raw.append(&Event {
            session: "a".into(),
            repo: Some("github.com/o/one".into()),
            ts: 5_000,
            ..test_event("fix the date parser")
        }))
        .unwrap();
        let stop = (raw.append(&Event {
            session: "b".into(),
            repo: Some("github.com/o/two".into()),
            ts: 9_000,
            kind: SUBAGENT_STOP.into(),
            ..test_event(r#"{"agent_id":"a1"}"#)
        }))
        .unwrap();
        let labels = raw.labels_in(&dev, work, stop).unwrap();
        assert_eq!(labels.session, Some(("claude".into(), "a".into())));
        assert_eq!(labels.repo.as_deref(), Some("github.com/o/one"));
        assert_eq!(labels.ts, Some(5_000));
    }

    /// X17 (docs/subagents.md): a subagent's stop, which may come after the reply it worked for,
    /// begins no turn: the next turn begins at its prompt.
    #[test]
    fn a_subagent_stop_begins_no_turn() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let ev = |kind: &str| Event {
            kind: kind.into(),
            ..test_event("{}")
        };
        for kind in ["prompt", "reply", SUBAGENT_STOP] {
            raw.append(&ev(kind)).unwrap();
        }
        let prompt = raw.append(&ev("prompt")).unwrap();
        let reply = raw.append(&ev("reply")).unwrap();
        assert_eq!(raw.turn_start("claude", "s", reply).unwrap(), prompt);
        assert_eq!(raw.hook_turn_start("claude", "s", reply).unwrap(), prompt);
    }

    /// X17 (docs/subagents.md): the session's previous window is the one that holds its last
    /// work, not one that holds no more of the session than a subagent's stop.
    #[test]
    fn a_subagent_stop_makes_no_previous_window_of_its_session() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let ev = |session: &str, kind: &str| Event {
            session: session.into(),
            kind: kind.into(),
            ..test_event("a line")
        };
        for (session, kind) in [("s", "prompt"), ("s", "reply"), ("s", SUBAGENT_STOP)] {
            raw.append(&ev(session, kind)).unwrap();
        }
        raw.append(&ev("t", "prompt")).unwrap();
        let next = raw.append(&ev("s", "prompt")).unwrap();
        // Windows 1 to 2 and 3 to 4, each with a claim quoting its last record.
        for (from, to, text) in [(1, 2, "of s"), (3, 4, "of t")] {
            let op = serde_json::json!({"from_seq": from, "to_seq": to, "recurate": false});
            let claim = serde_json::json!({"text": text, "evidence": [{"device": raw.device(),
                "seq": to, "offset": 0, "length": 4}]});
            (raw.append_ops(&[(OpKind::Window, op), (OpKind::Claim, claim)])).unwrap();
        }
        let ops = (raw.previous_window_ops("claude", "s", (next, None), |_| true)).unwrap();
        let texts: Vec<_> = ops.iter().map(|o| o.body["text"].as_str()).collect();
        assert_eq!(texts, [Some("of s")]);
    }

    /// X17 (docs/subagents.md): a subagent's stop in another checkout is none of a turn's
    /// repository.
    #[test]
    fn a_subagent_stop_is_none_of_a_turns_repository() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let ev = |kind: &str, repo: &str| Event {
            kind: kind.into(),
            repo: Some(repo.into()),
            ..test_event("{}")
        };
        let first = raw.append(&ev("prompt", "github.com/o/one")).unwrap();
        raw.append(&ev(SUBAGENT_STOP, "github.com/o/two")).unwrap();
        let last = raw.append(&ev("reply", "github.com/o/one")).unwrap();
        let repo = raw.session_repo(("claude", "s"), first, last).unwrap();
        assert_eq!(repo.as_deref(), Some("github.com/o/one"));
    }

    /// The SQL list the statements carry is the list the tests walk.
    #[test]
    fn the_lineless_kinds_are_one_list_in_sql_and_in_rust() {
        let kinds: Vec<String> = LINELESS.iter().map(|kind| format!("'{kind}'")).collect();
        assert_eq!(LINELESS_SQL, format!("({})", kinds.join(", ")));
    }

    /// docs/subagents.md: a removal or a mask of a record a curator is shown no line of (a
    /// session's start or end, a subagent's stop) took nothing a card, a summary or a claim holds;
    /// a prompt's still did.
    #[test]
    fn a_lineless_records_removal_takes_nothing_made() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let ev = |kind: &str| Event {
            kind: kind.into(),
            ..test_event(r#"{"agent_id":"a1"}"#)
        };
        let prompt = raw.append(&ev("prompt")).unwrap();
        let lineless: Vec<i64> = (LINELESS.iter())
            .map(|kind| raw.append(&ev(kind)).unwrap())
            .collect();
        let last = lineless[2];
        let range = |seq: i64| Target::Range {
            device: dev.clone(),
            seq,
            offset: 2,
            length: 8,
        };
        for &seq in &lineless {
            raw.append_tombstone(range(seq)).unwrap();
            (raw.append_tombstone(Target::Record {
                device: dev.clone(),
                seq,
            }))
            .unwrap();
        }
        assert_eq!(
            raw.removed_in(&dev, prompt, last, None).unwrap(),
            Vec::<Removal>::new()
        );
        let masked = raw.forgotten_and_masked().unwrap();
        assert!(
            lineless
                .iter()
                .all(|&seq| masked.entry(&dev, seq).is_none())
        );
        raw.append_tombstone(range(prompt)).unwrap();
        assert_eq!(
            raw.removed_in(&dev, prompt, last, None).unwrap(),
            [(prompt, Some(2), Some(8))]
        );
        let masked = raw.forgotten_and_masked().unwrap();
        assert!(masked.entry(&dev, prompt).is_some());
        let keys = |seqs: &[i64]| -> Vec<(String, i64)> {
            seqs.iter().map(|&seq| (dev.clone(), seq)).collect()
        };
        let all: Vec<i64> = std::iter::once(prompt).chain(lineless.clone()).collect();
        assert_eq!(raw.with_lines(keys(&all)).unwrap(), keys(&[prompt]));
    }

    /// docs/subagents.md: a span's sessions are those of the records its curator could be shown,
    /// not another session's start, end or subagent's stop.
    #[test]
    fn a_spans_sessions_are_not_those_of_its_lineless_records() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let ev = |session: &str, kind: &str| Event {
            session: session.into(),
            kind: kind.into(),
            ..test_event("{}")
        };
        let first = raw.append(&ev("a", "prompt")).unwrap();
        for kind in LINELESS {
            raw.append(&ev("b", kind)).unwrap();
        }
        let last = raw.append(&ev("a", "reply")).unwrap();
        let sessions = raw.sessions_over(&dev, &[(first, last)], &[]).unwrap();
        assert_eq!(sessions, ["claude\u{0}a".to_owned()].into());
        let b: std::collections::HashSet<String> = ["claude\u{0}b".to_owned()].into();
        assert!(!raw.holds_session(&dev, (first, last), &b).unwrap());
        let prompt = raw.append(&ev("b", "prompt")).unwrap();
        assert!(raw.holds_session(&dev, (first, prompt), &b).unwrap());
    }

    /// docs/store-format.md: the first subagent's stop raises the home to format 6 in the
    /// transaction that appends it, and a rebuild that brings one back, live, expired or removed,
    /// stamps it too; a home without one stays where it was.
    #[test]
    fn a_subagent_stop_raises_the_home_to_the_format_that_reads_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let prompt = raw.append(&test_event("{}")).unwrap();
        assert_eq!(format_in(&raw), OPEN_FORMAT);
        let stop = || Event {
            kind: SUBAGENT_STOP.into(),
            ..test_event(r#"{"agent_id":"a1"}"#)
        };
        let stops: Vec<i64> = (0..3).map(|_| raw.append(&stop()).unwrap()).collect();
        assert_eq!(format_in(&raw), SUBAGENT_STOP_FORMAT);
        raw.expire_for_test(&stops[1..2]);
        (raw.conn.execute(
            &format!("UPDATE records SET {REMOVED_ROW} WHERE device = ?1 AND seq = ?2"),
            params![dev, stops[2]],
        ))
        .unwrap();
        let lines = raw.export_lines(0, usize::MAX).unwrap();
        let stamped = |seq: i64| {
            let path = home.path().join(format!("raw.db.rebuilt-{seq}"));
            let mut rebuild = Rebuild::new(&path, &dev).unwrap();
            for (_, line) in lines.iter().filter(|(s, _)| *s == seq) {
                rebuild.add(line).unwrap();
            }
            rebuild.finish().unwrap();
            Connection::open(&path)
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(stamped(prompt), OPEN_FORMAT);
        for seq in stops {
            assert_eq!(stamped(seq), SUBAGENT_STOP_FORMAT, "{seq}");
        }
    }

    fn format_in(raw: &Raw) -> i64 {
        raw.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// docs/store-format.md: a raw.db a newer oboete stamped is refused by the writable open and
    /// the read-only one, and neither changes a byte of it (a reader may leave an empty WAL).
    #[test]
    fn a_raw_db_a_newer_oboete_wrote_is_refused_and_left_as_it_was() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = open(p).unwrap();
        raw.append(&test_event(r#"{"prompt":"kept as it was"}"#))
            .unwrap();
        // Without a column this binary's migration adds back, and in rollback-journal mode, which
        // this binary's open sets to WAL: a refused open changes neither (Codex on #503).
        raw.conn
            .execute_batch(&format!(
                "ALTER TABLE import_origins DROP COLUMN ambiguous;
                 PRAGMA user_version = {};",
                FORMAT + 1
            ))
            .unwrap();
        drop(raw);
        let mode = || {
            Connection::open(p.join("raw.db"))
                .unwrap()
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap()
        };
        Connection::open(p.join("raw.db"))
            .unwrap()
            .execute_batch("PRAGMA journal_mode = DELETE")
            .unwrap();
        assert_eq!(mode(), "delete");
        let bytes = |name: &str| std::fs::read(p.join(name)).ok();
        let before = bytes("raw.db");
        for refused in [open(p).err(), read_only(p).err()] {
            let e = refused.expect("refused");
            let found = e.downcast_ref::<NewerFormat>().map(|n| n.found);
            assert_eq!(found, Some(FORMAT + 1), "{e:#}");
            assert!(e.to_string().contains("update oboete"), "{e:#}");
        }
        assert_eq!(bytes("raw.db"), before);
        assert!(bytes("raw.db-wal").is_none_or(|wal| wal.is_empty()));
        assert_eq!(mode(), "delete");
    }

    /// docs/store-format.md: a raw.db from before the stamp gets it at its first writable open,
    /// and an open of a stamped one writes nothing for it. An open stamps the format the binary
    /// before retention reads, and never lowers one an expiry raised (docs/retention.md).
    #[test]
    fn a_raw_db_from_before_the_stamp_gets_it_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let raw = open(p).unwrap();
        assert_eq!(format_in(&raw), OPEN_FORMAT);
        raw.conn
            .execute_batch(&format!("PRAGMA user_version = {EXPIRED_FORMAT}"))
            .unwrap();
        drop(raw);
        let raw = open(p).unwrap();
        assert_eq!(format_in(&raw), EXPIRED_FORMAT);
        raw.conn.execute_batch("PRAGMA user_version = 0").unwrap();
        drop(raw);
        let mut changed = Vec::new();
        let raw = open_report(p, None, &mut |stage| {
            changed.push(stage);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            (format_in(&raw), changed),
            (OPEN_FORMAT, vec!["stores_changed"])
        );
        assert!(!raw.newer_format());
        let mut changed = Vec::new();
        let again = open_report(p, None, &mut |stage| {
            changed.push(stage);
            Ok(())
        })
        .unwrap();
        assert_eq!((format_in(&again), changed), (OPEN_FORMAT, vec![]));
        // A newer binary stamps it while this handle is open: the handle says so, and appends
        // neither an event nor an op (CodeRabbit on #503).
        again
            .conn
            .execute_batch(&format!("PRAGMA user_version = {}", FORMAT + 1))
            .unwrap();
        assert!(raw.newer_format());
        let mut raw = raw;
        let refused = |e: anyhow::Error| e.is::<NewerFormat>();
        assert!(refused(
            raw.append(&test_event("after the stamp")).unwrap_err()
        ));
        let op = (
            OpKind::Correction,
            serde_json::json!({"uid": "u", "status": "rejected"}),
        );
        assert!(refused(raw.append_ops(&[op]).unwrap_err()));
        // Nor a tombstone or an imported record (Codex on #503).
        let target = Target::Record {
            device: raw.device().to_owned(),
            seq: 1,
        };
        assert!(refused(raw.append_tombstone(target).unwrap_err()));
        assert!(refused(
            raw.append_imported(&[imported("after")], "", None)
                .unwrap_err()
        ));
    }

    /// docs/retention.md: an expired record reads as its labels with no text and stays an event to
    /// the label queries; a range tombstone leaves it, a whole one hides it as `Removed`; it
    /// crosses a backup as a line of its own type, which restores it, labels and ledger kept, into
    /// a raw.db of the format that reads it, while a restore with none stays at the open's format.
    #[test]
    fn an_expired_record_keeps_its_labels_and_crosses_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = open(p).unwrap();
        let mut e = test_event("the expired canary words");
        (e.repo, e.ts) = (Some("github.com/o/r".into()), 1_000);
        let gone = raw.append(&e).unwrap();
        let kept = raw.append(&test_event("kept words")).unwrap();
        let mut imported = test_event("the imported canary words");
        (imported.source, imported.session) = ("transcript".into(), "t".into());
        let identity = ImportIdentity {
            origin: crate::forget::origin("synthetic", "imported"),
            session: crate::forget::session(&imported.agent, &imported.session),
            ambiguous: None,
            unverified: false,
        };
        let other = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event: imported,
                    ledger: Vec::new(),
                }],
                std::slice::from_ref(&identity),
                "",
                None,
            )
            .unwrap()[0];
        raw.conn
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, 'session', 'r', 0, 1, 0, 'v')",
                params![raw.device(), gone],
            )
            .unwrap();
        let device = raw.device().to_owned();
        let rebuilt = |name: &str, lines: &[(i64, String)]| -> Connection {
            let path = p.join(name);
            let mut rebuild = Rebuild::new(&path, &device).unwrap();
            for (_, l) in lines {
                rebuild.add(l).unwrap();
            }
            rebuild.finish().unwrap();
            Connection::open(&path).unwrap()
        };
        let version = |c: &Connection| -> i64 {
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            version(&rebuilt(
                "plain.db",
                &raw.export_lines(0, usize::MAX).unwrap()
            )),
            OPEN_FORMAT
        );
        raw.expire_for_test(&[gone, other]);
        let read = raw.after(&device, 0, 10).unwrap();
        let Item::Expired(labels) = &read[0].item else {
            panic!("{:?}", read[0].item)
        };
        assert_eq!(
            (labels.body.as_str(), labels.repo.as_deref(), labels.ts),
            ("", Some("github.com/o/r"), 1_000)
        );
        assert!(matches!(read[1].item, Item::Event(_)));
        assert_eq!(
            raw.session_key(&device, gone).unwrap().as_deref(),
            Some("claude\u{0}s")
        );
        assert!(raw.holds_expired(&device, gone, kept).unwrap());
        assert!(!raw.holds_expired(&device, kept, kept).unwrap());
        let lines = raw.export_lines(0, usize::MAX).unwrap();
        assert!(lines[0].1.contains(r#""type":"expired""#), "{}", lines[0].1);
        assert!(
            lines.iter().all(|(_, l)| !l.contains("canary")),
            "{lines:?}"
        );
        assert!(lines[2].1.contains("import_identity"), "{}", lines[2].1);
        let c = rebuilt("expired.db", &lines);
        assert_eq!(version(&c), EXPIRED_FORMAT);
        let origin: String = c
            .query_row(
                "SELECT origin FROM import_origins WHERE seq = ?1",
                [other],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, identity.origin);
        let row: (String, Option<String>, Vec<u8>, Option<i64>, i64) = c
            .query_row(
                "SELECT type, repo, body, expired,
                   (SELECT COUNT(*) FROM ledger WHERE seq = records.seq)
                 FROM records WHERE seq = ?1",
                [gone],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (
                row.0.as_str(),
                row.1.as_deref(),
                row.2.len(),
                row.3.is_some(),
                row.4
            ),
            ("event", Some("github.com/o/r"), 0, true, 1)
        );
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: gone,
            offset: 0,
            length: 3,
        })
        .unwrap();
        assert!(matches!(
            raw.after(&device, gone - 1, 1).unwrap()[0].item,
            Item::Expired(_)
        ));
        raw.append_tombstone(Target::Record {
            device: device.clone(),
            seq: gone,
        })
        .unwrap();
        assert!(matches!(
            raw.after(&device, gone - 1, 1).unwrap()[0].item,
            Item::Removed
        ));
    }

    /// CodeRabbit on #503: a newer oboete that stamps raw.db while an open migrates it is refused
    /// before the open returns its handle.
    #[test]
    fn an_open_during_which_a_newer_oboete_stamps_raw_db_refuses_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(open(p).unwrap());
        BEFORE_OPEN_RETURNS.set(Some(|path| {
            Connection::open(path)
                .unwrap()
                .execute_batch(&format!("PRAGMA user_version = {}", FORMAT + 1))
                .unwrap();
        }));
        let e = open(p).err().expect("refused");
        assert!(e.is::<NewerFormat>(), "{e:#}");
    }

    /// Codex on #503: the other stores' opens read the home's format first, so a command that opens
    /// providers.db or knowledge.db before raw.db (`oboete resume`, doctor's provider lines)
    /// creates or changes neither in a home a newer oboete wrote.
    #[test]
    fn the_other_stores_refuse_a_home_a_newer_oboete_wrote() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(open(p).unwrap());
        Connection::open(p.join("raw.db"))
            .unwrap()
            .execute_batch(&format!("PRAGMA user_version = {}", FORMAT + 1))
            .unwrap();
        let refused = [
            crate::providers_db::open(p).err(),
            crate::providers_db::read_only(p).err(),
            crate::knowledge::open(p).err(),
        ];
        for e in refused {
            let e = e.expect("refused");
            assert!(e.is::<NewerFormat>(), "{e:#}");
        }
        assert!(!p.join("providers.db").exists());
        assert!(!p.join("knowledge.db").exists());
    }

    /// Codex on #458: a copy's sidecars are named on its path's own bytes, which need not be
    /// UTF-8.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_under_a_path_that_is_not_utf8_goes_with_its_sidecars() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(std::ffi::OsStr::from_bytes(b"home-\xff"));
        std::fs::create_dir(&home).unwrap();
        let files = [
            "raw.db.quarantined-1",
            "raw.db.quarantined-1-wal",
            "raw.db.quarantined-1-shm",
            "raw.db.quarantined-1-journal",
        ]
        .map(|name| home.join(name));
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        remove_with_sidecars(&files[0]).unwrap();
        for f in &files {
            assert!(!f.exists(), "{f:?}");
        }
    }

    /// G22: a session's start reads the skipped windows through `ops_window_skips`, whose
    /// condition `SKIP_WINDOWS` repeats, and finds the ones curation writes.
    #[test]
    fn skipped_windows_are_read_through_their_index() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let plan: Vec<String> = raw
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {SKIP_WINDOWS}"))
            .unwrap()
            .query_map(params!["d", 0, 10], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|p| p.contains("USING INDEX ops_window_skips")),
            "{plan:?}"
        );
        let window = |more: serde_json::Value| {
            let mut op = serde_json::json!({"from_seq": 1, "to_seq": 1, "outcome": "curated"});
            op.as_object_mut()
                .unwrap()
                .extend(more.as_object().unwrap().clone());
            (OpKind::Window, op)
        };
        raw.append_ops(&[
            window(serde_json::json!({})),
            window(serde_json::json!({"outcome": "skipped", "reason": "x"})),
            window(serde_json::json!({"refused": true})),
            window(serde_json::json!({"recurate": true})),
            window(serde_json::json!({"refused": false, "recurate": false, "reason": "skipped"})),
        ])
        .unwrap();
        let device = raw.device().to_owned();
        let seqs: Vec<i64> = raw
            .skip_window_ops_after(&device, 0, 10)
            .unwrap()
            .iter()
            .map(|o| o.op_seq)
            .collect();
        assert_eq!(seqs, [2, 3, 4]);
    }

    /// docs/my-work-model.md V2: the times of the prompts among the seqs named that no forget
    /// took; a range forget only masks text, and a tool record or another device's is no prompt.
    #[test]
    fn prompt_times_of_lists_the_named_prompts_no_forget_took() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let body = serde_json::json!({"prompt": "Fix the parser."}).to_string();
        let prompt = |ts: i64| Event {
            ts,
            ..test_event(&body)
        };
        let a = raw.append(&prompt(10)).unwrap();
        let b = raw.append(&prompt(20)).unwrap();
        let c = raw.append(&prompt(30)).unwrap();
        let tool = Event {
            kind: "tool".into(),
            ts: 40,
            ..test_event("{}")
        };
        let tool = raw.append(&tool).unwrap();
        let device = raw.device().to_owned();
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: b,
            offset: 0,
            length: 3,
        })
        .unwrap();
        raw.append_tombstone(Target::Record {
            device: device.clone(),
            seq: c,
        })
        .unwrap();
        let times = raw.prompt_times_of(&device, &[c, tool, b, a, 99]).unwrap();
        assert_eq!(times, [(a, 10), (b, 20)]);
        assert!(raw.prompt_times_of("other", &[a]).unwrap().is_empty());
    }

    #[test]
    fn at_reads_the_records_named_in_seq_order_as_after_returns_them() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for body in ["one", "two secret", "three"] {
            raw.append(&test_event(body)).unwrap();
        }
        let device = raw.device().to_owned();
        raw.append_tombstone(Target::Range {
            device: device.clone(),
            seq: 2,
            offset: 4,
            length: 6,
        })
        .unwrap();
        raw.append_tombstone(Target::Record {
            device: device.clone(),
            seq: 3,
        })
        .unwrap();
        let recs = raw.at(&device, &[3, 9, 2, 1]).unwrap();
        assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(matches!(&recs[0].item, Item::Event(e) if e.body == "one"));
        assert!(matches!(&recs[1].item, Item::Event(e) if e.body == "two ******"));
        assert!(matches!(recs[2].item, Item::Removed));
        assert!(raw.at(&device, &[]).unwrap().is_empty());
    }

    #[test]
    fn dispatch_wait_failure_records_no_exclusion() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let held = raw.dispatch().unwrap();

        assert!(
            raw.exclude("x", false).is_err(),
            "a transmitting batch permitted an exclusion acknowledgement"
        );
        assert_eq!(raw.max_op_seq().unwrap(), 0);
        assert!(raw.exclusions().unwrap().is_empty());
        assert!(!home.path().join("providers.db").exists());

        held.release();
        assert_eq!(raw.exclude("x", false).unwrap(), 1);
        assert_eq!(raw.exclusions().unwrap(), vec!["x"]);
        assert_eq!(raw.exclude("x", true).unwrap(), 2);
        assert!(raw.exclusions().unwrap().is_empty());
    }

    #[test]
    fn dispatch_wait_failure_is_bounded_and_registers_no_partial_forget() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut event = test_event(r#"{"prompt":"native-timeout-canary-903"}"#);
        event.source = "transcript".into();
        let identity = ImportIdentity {
            origin: crate::forget::origin("synthetic-dispatch", "timeout-903"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        let preview = crate::forget::preview(
            home.path(),
            crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            },
        )
        .unwrap();
        let held = raw.dispatch().unwrap();
        let began = Instant::now();
        let rejected = crate::forget::start(home.path(), &preview);
        let elapsed = began.elapsed();
        assert!(
            rejected.is_err(),
            "a transmitting batch permitted registration"
        );
        assert!(elapsed >= crate::db::OPEN_WRITE_WAIT && elapsed < Duration::from_secs(15));
        assert_eq!(raw.denied_count().unwrap(), 0);
        assert!(crate::forget::status(home.path()).unwrap().is_empty());
        assert!(!home.path().join("forget.log").exists());
        assert!(!home.path().join("backups/forget.log").exists());
        held.release();
        assert_eq!(
            crate::forget::start(home.path(), &preview)
                .unwrap()
                .0
                .records,
            1
        );
    }

    /// A rescan's whole tombstone hides the sample, but forget must still deny a verified
    /// transcript import in a record or span target and retain its original metadata.
    #[test]
    fn a_tombstoned_import_can_be_forgotten_without_a_sample_and_cannot_be_reimported() {
        for span in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            let mut event = test_event(r#"{"prompt":"tombstoned-native-canary-691"}"#);
            event.source = "transcript".into();
            event.ts = 1_000;
            let captured = || crate::capture::Captured {
                event: event.clone(),
                ledger: Vec::new(),
            };
            let identity = ImportIdentity {
                origin: crate::forget::origin("synthetic", "tombstoned-native"),
                session: crate::forget::session(&event.agent, &event.session),
                ambiguous: None,
                unverified: false,
            };
            let seq = raw
                .append_imported_origins(&[captured()], std::slice::from_ref(&identity), "", None)
                .unwrap()[0];
            let device = raw.device().to_owned();
            let tombstone = raw
                .append_tombstone(Target::Record {
                    device: device.clone(),
                    seq,
                })
                .unwrap();
            let target = if span {
                crate::forget::Target::Span {
                    device: device.clone(),
                    from: seq,
                    to: tombstone,
                }
            } else {
                crate::forget::Target::Record {
                    device: device.clone(),
                    seq,
                }
            };
            let preview = raw.forget_preview(target).unwrap();
            assert_eq!(preview.count(), 1, "the tombstone hid the import identity");
            assert!(preview.sample.is_none(), "a removed sample was shown");
            assert_eq!(preview.records[0].ts, None);
            assert_eq!(preview.sources, ["transcript"]);
            crate::forget::start(home.path(), &preview).unwrap();
            assert!(
                raw.append_imported_origins(
                    &[captured()],
                    std::slice::from_ref(&identity),
                    "",
                    None
                )
                .unwrap()
                .is_empty(),
                "the already tombstoned import was not denied"
            );
            assert_eq!(raw.transcript_cut("claude", "s", None).unwrap(), None);
        }
    }

    /// F2 also covers the consistent snapshot after the first schema transaction, before
    /// the opener returns: it already carries the lineage later forgets will name.
    /// Milestone 5 D5: a forgotten uid's Claim (derived again, whatever its body), Correction and
    /// Import ops are refused, an imported document of it is passed over, and a request applied
    /// again adds no second forget op.
    #[test]
    fn a_forgotten_uids_ops_are_refused_and_its_document_passed_over() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let seq = raw.append(&test_event("We use tabs.")).unwrap();
        let device = raw.device().to_owned();
        let claim = |body: &str| {
            serde_json::to_value(crate::claims::ClaimOp {
                id: "c1".into(),
                kind: "decision".into(),
                status: "decided".into(),
                speaker: "user".into(),
                scope: "repo".into(),
                body: body.into(),
                evidence: vec![crate::claims::Evidence {
                    device: device.clone(),
                    seq,
                    offset: 0,
                    length: 6,
                    sentence: 0,
                    quote: "We use".into(),
                    claim_at: None,
                }],
                supersedes: Vec::new(),
                recipe: "test".into(),
                tier: 1,
                why: String::new(),
                tainted: false,
                support: Vec::new(),
            })
            .unwrap()
        };
        let doc = |id: &str| ImportDoc {
            uid: format!("claude-mem:test:{id}"),
            source: "claude-mem:test".into(),
            source_id: id.into(),
            kind: "decision".into(),
            repo: "claude-mem:r".into(),
            session: "s".into(),
            ts: 1,
            title: "A title".into(),
            body: "A body.".into(),
        };
        let uid = crate::claims::op_uid(&claim("Use tabs.")).unwrap();
        let forget = |raw: &mut Raw, uid: &str, job: char| {
            raw.forget_apply(&[crate::forget::Request {
                v: 2,
                home: raw.home_id().into(),
                job: job.to_string().repeat(32),
                started: 1,
                target: crate::forget::Target::Uid { uid: uid.into() },
                records: Vec::new(),
            }])
            .unwrap()
        };
        assert_eq!(forget(&mut raw, &uid, 'a'), 1);
        assert_eq!(forget(&mut raw, &doc("o1").uid, 'b'), 1);
        // Another job of a uid forgotten already writes its job row and no second op.
        assert_eq!(forget(&mut raw, &uid, 'c'), 1);
        let forgets = |raw: &Raw| {
            raw.ops_after(&device, 0, 100)
                .unwrap()
                .iter()
                .filter(|o| o.kind == OpKind::Forget)
                .count()
        };
        assert_eq!(forgets(&raw), 2);
        let correction = serde_json::json!({"uid": uid, "status": "done",
            "anchor": {"device": device, "seq": seq}});
        for (kind, body) in [
            (OpKind::Claim, claim("Indent with tabs everywhere.")),
            (OpKind::Correction, correction),
            (OpKind::Import, serde_json::to_value(doc("o1")).unwrap()),
        ] {
            assert!(raw.append_ops(&[(kind, body)]).is_err(), "{kind:?}");
        }
        assert_eq!(raw.append_imports(vec![doc("o1"), doc("o2")]).unwrap(), 1);
        assert!(raw.forgotten(&uid).unwrap() && raw.forgotten(&doc("o1").uid).unwrap());
        assert!(!raw.forgotten(&doc("o2").uid).unwrap());
        assert_eq!(raw.forget_requests().unwrap().len(), 3);
    }

    #[test]
    fn a_snapshot_of_first_schema_commit_keeps_forget_lineage() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("raw.db");
        let conn = Connection::open(&path).unwrap();
        crate::db::ensure_schema_until(
            &conn,
            &schema_for_file(&conn, &path).unwrap(),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        drop(conn);
        let old = home.path().join("first-schema.db");
        std::fs::copy(&path, &old).unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut event = test_event(r#"{"prompt":"first-schema-forget-canary-671"}"#);
        event.source = "transcript".into();
        let captured = || crate::capture::Captured {
            event: event.clone(),
            ledger: Vec::new(),
        };
        let identity = ImportIdentity {
            origin: crate::forget::origin("synthetic", "first-schema"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(&[captured()], std::slice::from_ref(&identity), "", None)
            .unwrap()[0];
        let preview = raw
            .forget_preview(crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            })
            .unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        drop(raw);
        std::fs::copy(old, &path).unwrap();
        let mut restored = open(home.path()).unwrap();
        crate::forget::reconcile(home.path(), &mut restored).unwrap();
        assert!(
            restored
                .append_imported_origins(&[captured()], &[identity], "", None)
                .unwrap()
                .is_empty(),
            "the first-schema snapshot changed lineage and reimported a forgotten event"
        );
    }

    /// docs/cards.md K4: what tombstones remove from a span, each once; with `through`, only
    /// what this device's own tombstones up to it remove, as another device's seqs say nothing of
    /// when.
    #[test]
    fn removals_of_a_span_are_listed_once_and_through_a_seq() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let device = raw.device().to_owned();
        for _ in 0..3 {
            raw.append(&test_event("{}")).unwrap();
        }
        let part = |seq: i64, offset: i64| Target::Range {
            device: device.clone(),
            seq,
            offset,
            length: 1,
        };
        let whole = Target::Record {
            device: device.clone(),
            seq: 1,
        };
        raw.append_tombstone(part(2, 0)).unwrap(); // 4
        raw.append_tombstone(part(2, 0)).unwrap(); // 5: the same part again
        raw.append_tombstone(whole).unwrap(); // 6
        raw.append_tombstone(part(3, 0)).unwrap(); // 7: outside the span asked for
        raw.conn
            .execute(
                "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                   target_offset, target_length)
                 VALUES('other', 1, 'tombstone', 0, 'rescan', ?1, 2, 1, 1)",
                [&device],
            )
            .unwrap();
        let first = (2, Some(0), Some(1));
        assert_eq!(
            raw.removed_in(&device, 1, 2, None).unwrap(),
            [(1, None, None), first, (2, Some(1), Some(1))]
        );
        assert_eq!(raw.removed_in(&device, 1, 2, Some(5)).unwrap(), [first]);
        assert_eq!(
            raw.removed_in(&device, 1, 2, Some(6)).unwrap(),
            [(1, None, None), first]
        );
    }

    /// Slice 3e-2 (Codex on #485): ops written apart go in appends within one batch's caps, the
    /// bytes' as well as the count's, in order.
    #[test]
    fn append_batches_keep_each_append_within_its_caps() {
        let op = |n: usize| (OpKind::Claim, serde_json::json!({ "body": "x".repeat(n) }));
        let big: Vec<_> = (0..80).map(|_| op(60_000)).collect();
        let batches = append_batches(&big);
        assert_eq!(batches.len(), 2);
        for batch in &batches {
            within_batch_cap(batch).unwrap();
        }
        assert_eq!(batches.concat(), big);
        assert!(append_batches(&[]).is_empty());
    }

    #[test]
    fn a_range_is_masked_by_whole_characters_and_masking_again_changes_nothing() {
        assert_eq!(masked("abcdef", 1, 2), "a**def");
        // A range that starts or ends inside a character covers all of its bytes.
        assert_eq!(masked("aé日b", 2, 2), "a*****b");
        assert_eq!(masked("abc", 2, 100), "ab*"); // cut at the end
        assert_eq!(masked("abc", 7, 2), "abc");
        let once = masked("aé日b", 2, 2);
        assert_eq!(masked(&once, 2, 2), once);
        assert_eq!(once.len(), "aé日b".len());
        // Ranges that overlap star their union and move nothing, in a read and in a purge's
        // rewrite of the row (#932: a rule that takes a secret whole finds it over the tombstone
        // an earlier rescan wrote inside it).
        assert_eq!(masked(&masked("abcdefgh", 2, 2), 1, 5), "a*****gh");
        let row = masked_row(b"abcdefgh".to_vec(), "plain", &[(2, 2), (1, 5)]).unwrap();
        assert_eq!(row, Some((b"a*****gh".to_vec(), "plain")));
    }

    /// Whether `e` is SQLite's "database is locked".
    fn busy(e: &anyhow::Error) -> bool {
        e.downcast_ref::<rusqlite::Error>()
            .is_some_and(|e| e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy))
    }

    #[test]
    fn two_writers_get_consecutive_seqs_and_both_land() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        open(p).unwrap(); // create the file first
        let seqs: Vec<i64> = std::thread::scope(|s| {
            let h: Vec<_> = (0..2)
                .map(|i| {
                    s.spawn(move || {
                        let mut r = open(p).unwrap();
                        // A writer in a tight loop can keep the other out past the busy timeout
                        // on a slow disk (#362): this is about the seqs, so it asks again.
                        (0..50)
                            .map(|n| {
                                loop {
                                    match r.append(&test_event(&format!("{i}-{n}"))) {
                                        Ok(seq) => break seq,
                                        Err(e) if busy(&e) => continue,
                                        Err(e) => panic!("{e:#}"),
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            h.into_iter().flat_map(|t| t.join().unwrap()).collect()
        });
        let mut sorted = seqs.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=100).collect::<Vec<_>>());
        let r = open(p).unwrap();
        assert_eq!(r.max_seq().unwrap(), 100);
        let bodies: std::collections::HashSet<String> = r
            .after(r.device(), 0, 1000)
            .unwrap()
            .into_iter()
            .map(|rec| match rec.item {
                Item::Event(e) => e.body,
                Item::Removed | Item::Tombstone(_) | Item::Expired(_) => unreachable!(),
            })
            .collect();
        assert_eq!(bodies.len(), 100);
    }

    #[test]
    fn an_initialized_store_opens_without_waiting_for_a_writer() {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| sent.send(open(home.path())).unwrap());
            let opened = received.recv_timeout(std::time::Duration::from_secs(2));
            writer.conn.execute_batch("ROLLBACK").unwrap();
            let opened = opened.expect("opening an initialized store must not need a write lock");
            assert_eq!(opened.unwrap().device(), writer.device());
        });
    }

    #[test]
    fn missing_schema_objects_share_one_write_transaction_and_roll_back_together() {
        let home = tempfile::tempdir().unwrap();
        let store = open(home.path()).unwrap();
        let drop_tables = "DROP TABLE ledger; DROP TABLE ops;";
        store.conn.execute_batch(drop_tables).unwrap();
        crate::crash::off();
        let reopened = open(home.path()).unwrap();
        // One commit means no writer can interleave between the schema's CREATE statements.
        assert_eq!(crate::crash::count(), 1);
        let tables = || {
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                     AND name IN ('ledger', 'ops')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(tables(), 2);
        reopened.conn.execute_batch(drop_tables).unwrap();
        crate::crash::at(1);
        let failed = open(home.path());
        crate::crash::off();
        assert!(failed.is_err());
        assert_eq!(tables(), 0);
    }

    #[test]
    fn missing_open_state_waits_past_the_busy_timeout_for_a_writer() {
        // Each case needs a different open-time write. Observe each opener's first actual
        // lock retry before holding the locks a little longer, rather than timing thread start.
        let changes = [
            "DROP INDEX ops_exclusions",
            "DROP TABLE ops",
            "ALTER TABLE ledger DROP COLUMN field",
            "DELETE FROM meta",
            "PRAGMA journal_mode=DELETE",
        ];
        let homes: Vec<_> = changes
            .iter()
            .map(|_| tempfile::tempdir().unwrap())
            .collect();
        let writers: Vec<_> = homes
            .iter()
            .zip(changes)
            .map(|(home, sql)| {
                let writer = open(home.path()).unwrap();
                writer.conn.execute_batch(sql).unwrap();
                writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                writer
            })
            .collect();
        std::thread::scope(|scope| {
            let (retried, received) = std::sync::mpsc::channel();
            let threads: Vec<_> = homes
                .iter()
                .map(|home| {
                    let retried = retried.clone();
                    scope.spawn(move || {
                        crate::db::BUSY_RETRY_NOTICE.with(|notice| notice.set(Some(retried)));
                        let opened = open(home.path());
                        assert!(
                            crate::db::BUSY_RETRY_NOTICE.with(|notice| notice.take().is_none()),
                            "opening a contended store must retry"
                        );
                        opened
                    })
                })
                .collect();
            drop(retried);
            let observed: Result<Vec<_>, _> = threads
                .iter()
                .map(|_| received.recv_timeout(std::time::Duration::from_secs(5)))
                .collect();
            if observed.is_ok() {
                // The ordinary write attempts have already exhausted SQLite's 2 s timeout.
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            for writer in &writers {
                writer.conn.execute_batch("ROLLBACK").unwrap();
            }
            observed.expect("every opener must retry before the write locks are released");
            for (index, thread) in threads.into_iter().enumerate() {
                let mut opened = thread.join().unwrap().unwrap();
                if index != 3 {
                    assert_eq!(opened.device(), writers[index].device());
                }
                assert_eq!(opened.append(&test_event("after the lock")).unwrap(), 1);
                let timeout: i64 = opened
                    .conn
                    .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(timeout, 2_000);
                let synchronous: i64 = opened
                    .conn
                    .query_row("PRAGMA synchronous", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(synchronous, 2);
            }
        });
    }

    #[test]
    fn raw_db_is_full_and_has_no_label_index() {
        let home = tempfile::tempdir().unwrap();
        let r = open(home.path()).unwrap();
        let sync: i64 = r
            .conn
            .query_row("PRAGMA synchronous", [], |x| x.get(0))
            .unwrap();
        assert_eq!(sync, 2); // FULL
        // spec 1.6: session, repo and branch are labels, never an index root.
        let idx: i64 = r
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND
                 (sql LIKE '%session%' OR sql LIKE '%repo%' OR sql LIKE '%branch%')",
                [],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn after_returns_what_was_appended_in_seq_order() {
        let home = tempfile::tempdir().unwrap();
        let mut r = open(home.path()).unwrap();
        let mut e = test_event("ロボットの記録");
        e.repo = Some("github.com/o/r".into());
        e.original_bytes = Some(300_000);
        for _ in 0..3 {
            r.append(&test_event("x")).unwrap();
        }
        assert_eq!(r.append(&e).unwrap(), 4);
        let recs = r.after(r.device(), 2, 10).unwrap();
        assert_eq!(recs.iter().map(|x| x.seq).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(recs[1].item, Item::Event(Box::new(e)));
        assert_eq!(recs[1].device, r.device());
        assert!(r.after("other-device", 0, 10).unwrap().is_empty());
    }

    #[test]
    fn a_ledger_from_task_1_gains_its_field_column() {
        let home = tempfile::tempdir().unwrap();
        let c = Connection::open(home.path().join("raw.db")).unwrap();
        c.execute_batch(
            "CREATE TABLE ledger (device TEXT NOT NULL, seq INTEGER NOT NULL, rule TEXT NOT NULL,
             offset INTEGER NOT NULL, length INTEGER NOT NULL, ts INTEGER NOT NULL, ruleset TEXT NOT NULL);",
        )
        .unwrap();
        drop(c);
        let mut r = open(home.path()).unwrap();
        let f = crate::redact::Finding {
            rule: "r".into(),
            offset: 0,
            length: 1,
        };
        r.append_with_ledger(&test_event("x"), &[("/prompt".into(), f)], "test")
            .unwrap();
        let field: String = r
            .conn
            .query_row("SELECT field FROM ledger", [], |x| x.get(0))
            .unwrap();
        assert_eq!(field, "/prompt");
    }

    #[test]
    fn bodies_larger_than_a_batch_together_are_compressed_over_several() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let big = "c".repeat(BATCH_BYTES as usize / 2 + 1);
        for _ in 0..3 {
            raw.append(&test_event(&big)).unwrap();
        }
        let device = raw.device().to_owned();
        assert_eq!(raw.compress_through(&device, 0, 3).unwrap(), 3);
        let recs = raw.after(&device, 0, 3).unwrap();
        assert!(
            recs.iter()
                .all(|r| matches!(&r.item, Item::Event(e) if e.body == big))
        );
    }

    #[test]
    fn a_body_above_the_decode_limit_stays_plain_and_reads_back() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let big = "a".repeat(MAX_BODY_BYTES as usize + 1);
        raw.append(&test_event(&big)).unwrap();
        raw.append(&test_event(&"b".repeat(1000))).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(raw.compress_through(&device, 0, 2).unwrap(), 1); // only the second
        let recs = raw.after(&device, 0, 2).unwrap();
        assert!(matches!(&recs[0].item, Item::Event(e) if e.body.len() == big.len()));
    }

    #[test]
    fn a_body_that_decompresses_past_the_limit_is_an_error() {
        let bomb = zstd::bulk::compress(&vec![0u8; (MAX_BODY_BYTES + 1) as usize], 3).unwrap();
        assert!(bomb.len() < 64 * 1024, "{}", bomb.len());
        assert!(unzstd(&bomb).is_err());
        let ok = zstd::bulk::compress(b"fine", 3).unwrap();
        assert_eq!(unzstd(&ok).unwrap(), b"fine");
    }

    /// Codex on #364: a span's time is its last event's by seq, not the largest: records are not
    /// always appended in time order (a replay, a hook that writes late).
    #[test]
    fn a_spans_time_is_its_last_events() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for ts in [5_000, 9_000, 7_000] {
            raw.append(&Event {
                ts,
                ..test_event("{}")
            })
            .unwrap();
        }
        let dev = raw.device().to_owned();
        assert_eq!(raw.labels_in(&dev, 1, 3).unwrap().ts, Some(7_000));
        assert_eq!(raw.labels_in(&dev, 1, 2).unwrap().ts, Some(9_000));
    }

    #[test]
    fn device_id_stays_with_the_file_and_changes_on_a_copy() {
        let home = tempfile::tempdir().unwrap();
        let first = open(home.path()).unwrap().device().to_owned();
        assert!(
            first.len() == 8 && first.bytes().all(|b| b.is_ascii_hexdigit()),
            "{first}"
        );
        assert_eq!(open(home.path()).unwrap().device(), first);
        // The same store copied elsewhere (another machine's home) is another device.
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let other = open(copy.path()).unwrap();
        assert_ne!(other.device(), first);
        assert_eq!(open(copy.path()).unwrap().device(), other.device());
    }

    #[test]
    fn a_page_is_bounded_in_bytes_and_always_holds_one_record() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        for _ in 0..3 {
            raw.append(&test_event(&"x".repeat(1000))).unwrap();
        }
        assert_eq!(raw.after_within(&dev, 0, 10, 1).unwrap().len(), 1);
        assert_eq!(raw.after_within(&dev, 0, 10, 1 << 20).unwrap().len(), 3);
        assert_eq!(raw.after_within(&dev, 0, 2, 1 << 20).unwrap().len(), 2);
        // Bodies that compress well are counted as read, not as stored.
        raw.compress_through(&dev, 0, 3).unwrap();
        let stored: i64 = raw
            .conn
            .query_row("SELECT SUM(length(body)) FROM records", [], |r| r.get(0))
            .unwrap();
        assert!(
            stored < 1000,
            "the bodies were not compressed: {stored} bytes"
        );
        assert_eq!(raw.after_within(&dev, 0, 10, 1500).unwrap().len(), 1);
    }

    /// Slice 3e-4 (CodeRabbit on #487): an exclusion op's `folder` and `level`, slice 4a's, and its
    /// `pattern` (parity row 14) are names no rule masks, so an excluded folder keeps matching its
    /// events' `cwd` labels.
    #[test]
    fn an_exclusion_ops_folder_and_level_are_names() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'acme-[0-9]{6}|capture' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let body = serde_json::json!({
            "folder": "/w/acme-482915", "level": "capture", "undo": false, "clock": 1,
        });
        assert_eq!(
            masked_op_text(OpKind::Exclusion, &body.to_string(), &rules),
            None
        );
        // Parity row 14 (Codex on #563): a pattern too, whose `*` a mask would make another one.
        let body = serde_json::json!({
            "pattern": "acme-482915-*", "level": "capture", "undo": false, "clock": 1,
        });
        assert_eq!(
            masked_op_text(OpKind::Exclusion, &body.to_string(), &rules),
            None
        );
        let other = serde_json::json!({"note": "/w/acme-482915"}).to_string();
        assert!(masked_op_text(OpKind::Exclusion, &other, &rules).is_some());
    }

    /// #751: My Focus's ids are kept in a My Focus op's own fields only, of jobs `focus` and
    /// `place`; the same keys of another op, or nested deeper, are text.
    #[test]
    fn my_focus_ids_are_kept_in_a_my_focus_ops_own_fields() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"hex\", regex = '[0-9a-f]{12}' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let id = "fa_0123456789ab";
        let op = |job: &str| {
            serde_json::json!({"job": job, "about": "c:claude:s1", "fields": {"area": id,
                "merged_into": id, "nested": {"area": id}}})
        };
        let masked = |kind, body: serde_json::Value| -> serde_json::Value {
            let text = masked_op_text(kind, &body.to_string(), &rules).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        for job in [crate::my_work_focus::NAME, crate::my_work_place::NAME] {
            let fields = &masked(OpKind::MyWork, op(job))["fields"];
            assert_eq!(
                (&fields["area"], &fields["merged_into"]),
                (&serde_json::json!(id), &serde_json::json!(id))
            );
            assert_ne!(fields["nested"]["area"], serde_json::json!(id), "{job}");
        }
        assert_ne!(
            masked(OpKind::MyWork, op("rules"))["fields"]["area"],
            serde_json::json!(id)
        );
        assert_ne!(
            masked(OpKind::WorkState, op("focus"))["fields"]["area"],
            serde_json::json!(id)
        );
    }

    /// docs/my-work-model.md S6: a My Work op's labels (its job, subject and entry) are names no
    /// rule masks; the fields it kept are text, masked as any op's.
    #[test]
    fn a_my_work_ops_labels_are_names_and_its_fields_text() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let body = serde_json::json!({"job": "group", "about": "github.com/o/acme-482915",
            "entry": "acme-482915", "ts": 1, "read": {"spans": [[1, 2]], "ops": []},
            "removed": [], "fields": {"title": "Ship acme-482915"}, "skipped": false});
        let now = masked_op_text(OpKind::MyWork, &body.to_string(), &rules).unwrap();
        let now: serde_json::Value = serde_json::from_str(&now).unwrap();
        for label in ["job", "about", "entry"] {
            assert_eq!(now[label], body[label], "{label}");
        }
        assert_eq!(
            now["fields"]["title"],
            serde_json::json!("Ship ***********")
        );
    }

    /// A hide op holds no text a rule masks: its target is oboete's word, so a rule that finds
    /// `card` or `summary` leaves it naming what it removed (Codex on #658).
    #[test]
    fn a_hide_op_holds_no_text_a_rule_masks() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'card|summary' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        for target in ["card", "summary"] {
            let body = serde_json::json!({
                "target": target, "device": "d", "op_seq": 3, "n": 0, "hidden": true,
            });
            assert_eq!(
                masked_op_text(OpKind::Hide, &body.to_string(), &rules),
                None
            );
            assert!(masked_op_text(OpKind::Window, &body.to_string(), &rules).is_some());
        }
    }

    /// Slice 3e-4 (Codex on #487): a window's `reason` is kept only when oboete set the window
    /// aside, which `oboete recurate --skipped` and `--source` read; a provider's failure is free
    /// text and a rule masks it.
    #[test]
    fn a_windows_reason_is_masked_unless_oboete_set_it_aside() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'acme-[0-9]{6}|excluded|transcript' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        for kept in ["excluded", "imported:transcript"] {
            let body = serde_json::json!({"outcome": "skipped", "reason": kept});
            assert_eq!(
                masked_op_text(OpKind::Window, &body.to_string(), &rules),
                None
            );
        }
        let failed = "every provider failed: groq: key file /w/acme-482915/key.md unreadable";
        let body = serde_json::json!({"outcome": "skipped", "reason": failed});
        let now = masked_op_text(OpKind::Window, &body.to_string(), &rules).unwrap();
        let now: serde_json::Value = serde_json::from_str(&now).unwrap();
        let reason = now["reason"].as_str().unwrap();
        assert!(!reason.contains("acme-482915") && reason.contains("/w/***********/key.md"));
        assert!(!crate::curate::set_aside(reason));
    }

    /// Slice 3e-4 (Codex on #487): a correction at its 1,000-character cap, in characters of
    /// three bytes, keeps its status and stays a correction once a rule masks part of its body:
    /// one `*` for each character.
    #[test]
    fn a_masked_correction_stays_under_its_character_cap() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = '機密[あ-ん]{8}' }]\n",
        )
        .unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let text = format!("機密{}{}", "あ".repeat(8), "い".repeat(990));
        assert_eq!(text.chars().count(), crate::claims::MAX_BODY_CHARS);
        let body = serde_json::json!({
            "uid": "a".repeat(64), "status": "retracted", "body": text,
            "anchor": {"device": "d", "seq": 1},
        });
        let now = masked_op_text(OpKind::Correction, &body.to_string(), &rules).unwrap();
        let op: crate::claims::CorrectionOp = serde_json::from_str(&now).unwrap();
        assert_eq!(op.fault(), None);
        assert_eq!(op.status.as_deref(), Some("retracted"));
        let masked = op.body.unwrap();
        assert_eq!(masked.chars().count(), crate::claims::MAX_BODY_CHARS);
        assert!(masked.starts_with(&"*".repeat(10)) && !masked.contains("機密"));
    }

    /// Codex on #304: the list folds a device's ops in their own order, so a clock set back
    /// between two of them never puts the newer one first.
    #[test]
    fn a_clock_set_back_never_puts_a_newer_exclusion_first() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let op = |undo: bool| {
            let body = serde_json::json!({"repo": "x", "undo": undo});
            (OpKind::Exclusion, body)
        };
        raw.append_ops(&[op(false)]).unwrap();
        raw.append_ops(&[op(true)]).unwrap();
        let last = raw.append_ops(&[op(false)]).unwrap()[0];
        raw.conn
            .execute("UPDATE ops SET ts = 0 WHERE op_seq = ?1", [last])
            .unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["x"]);
    }

    /// Codex on #304: a copied store keeps the old device's ops under a new device, and an
    /// exclusion appended there comes after every op it holds, even when the old device's clock
    /// ran ahead of the new one's.
    #[test]
    fn an_exclusion_comes_after_every_op_a_copied_store_holds() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude("x", false).unwrap();
        raw.exclude("x", true).unwrap();
        raw.conn
            .execute(
                "UPDATE ops SET ts = ts + 3600000,
                   body = json_set(body, '$.clock', json_extract(body, '$.clock') + 3600000)",
                [],
            )
            .unwrap();
        drop(raw);
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let mut other = open(copy.path()).unwrap();
        other.exclude("x", false).unwrap();
        assert_eq!(other.exclusions().unwrap(), ["x"]);
    }

    /// Codex on #489: a folder excluded under one spelling covers another where the platform's file
    /// systems ignore letter case (Windows, macOS), and only there.
    #[test]
    fn a_folder_covers_another_spelling_of_itself_only_where_case_is_ignored() {
        let (stored, event) = if cfg!(windows) {
            (r"C:\Work\Private", r"c:\work\private\sub")
        } else {
            ("/Users/x/Work/Private", "/users/x/work/private/sub")
        };
        let list = CaptureExclusions {
            folders: [stored.to_owned()].into(),
            ..Default::default()
        };
        assert_eq!(
            list.covers(None, Some(event)),
            cfg!(any(windows, target_os = "macos"))
        );
    }

    /// Parity row 14: an event in a folder a pattern names is refused by an append, and a pattern
    /// taken back records it again; every pattern ever named is known, taken back or not.
    #[test]
    fn a_pattern_keeps_the_folders_it_names_from_every_append() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let at = |cwd: &str| Event {
            repo: Some("github.com/o/open".into()),
            cwd: Some(cwd.into()),
            ..test_event("canary-14")
        };
        raw.exclude_capture(("pattern", "secret-*"), false).unwrap();
        for cwd in ["/w/secret-a", "/w/secret-a/src"] {
            assert_eq!(raw.append_with_ledger(&at(cwd), &[], "").unwrap(), None);
        }
        assert!(
            raw.append_with_ledger(&at("/w/open"), &[], "")
                .unwrap()
                .is_some()
        );
        raw.exclude_capture(("pattern", "secret-*"), true).unwrap();
        assert!(
            raw.append_with_ledger(&at("/w/secret-a"), &[], "")
                .unwrap()
                .is_some()
        );
        assert!(raw.capture_exclusions().unwrap().patterns.is_empty());
        assert_eq!(
            Vec::from_iter(raw.patterns_ever_named().unwrap()),
            ["secret-*"]
        );
    }

    /// Slice 4a (spec 6.1): what is never recorded is never sent either, and each undo takes its
    /// entry out of its own list only: a capture undo leaves `oboete exclude`'s exclusion, and an
    /// undo of that leaves what is never recorded. A folder is never a repository to send.
    #[test]
    fn what_is_never_recorded_and_what_is_not_sent_keep_their_own_undo() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude("a", false).unwrap();
        raw.exclude_capture(("repo", "a"), false).unwrap();
        raw.exclude_capture(("repo", "b"), false).unwrap();
        raw.exclude_capture(("folder", "/x/y"), false).unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["a", "b"]);
        raw.exclude_capture(("repo", "a"), true).unwrap();
        raw.exclude("b", true).unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["a", "b"]);
        let never = raw.capture_exclusions().unwrap();
        assert_eq!(
            (Vec::from_iter(never.repos), Vec::from_iter(never.folders)),
            (vec!["b".to_owned()], vec!["/x/y".to_owned()])
        );
        raw.exclude_capture(("folder", "/x/y"), true).unwrap();
        raw.exclude_capture(("repo", "b"), true).unwrap();
        assert_eq!(
            raw.capture_exclusions().unwrap(),
            CaptureExclusions::default()
        );
        assert_eq!(raw.exclusions().unwrap(), ["a"]);
    }

    /// Codex on #491 (owner, 2026-10-13): a repository stored as given is matched in the hook's
    /// write transaction as the rules now read it too, so an event the hook labelled under a rule
    /// that masks the name is refused there; an append with no settings compares the list as stored.
    #[test]
    fn a_hooks_append_matches_the_list_as_the_rules_now_read_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let given = "github.com/o/acme-482915";
        raw.exclude_capture(("repo", given), false).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let labelled = Event {
            repo: Some(crate::capture::exclusion_label("repo", given, &settings)),
            ..test_event("After the rule.")
        };
        assert_ne!(labelled.repo.as_deref(), Some(given));
        let elsewhere = ("github.com/o/open", None);
        assert_eq!(
            raw.append_captured(&labelled, &[], &settings, elsewhere)
                .unwrap(),
            None
        );
        assert!(
            raw.append_with_ledger(&labelled, &[], "")
                .unwrap()
                .is_some()
        );
    }

    /// Security review of #489: the hook's write transaction also asks about the folder as the hook
    /// read it, so an exclusion committed after the hook's check refuses an event below it though a
    /// rule that takes the folder's separator keeps the labels from matching.
    #[test]
    fn a_hooks_append_matches_the_folder_as_the_hook_read_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let below = "/x/private/sub";
        let labelled = Event {
            cwd: Some(crate::capture::exclusion_label("cwd", below, &settings)),
            ..test_event("Below the folder.")
        };
        let read = ("github.com/o/r", Some(below));
        assert_eq!(
            raw.append_captured(&labelled, &[], &settings, read)
                .unwrap(),
            None
        );
        let elsewhere = ("github.com/o/r", Some("/x/open/sub"));
        assert!(
            raw.append_captured(&labelled, &[], &settings, elsewhere)
                .unwrap()
                .is_some()
        );
    }

    /// `save_memory`: a note asks in its transaction what a work state write asks, of its own
    /// repository's key and of the writer's folder as read; a note forget names, or one already
    /// kept, appends nothing.
    #[test]
    fn a_note_asks_in_its_transaction_what_a_work_state_write_asks() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let note = |repo: &str, id: &str| ImportDoc {
            uid: format!("{NOTE_SOURCE}:{id}"),
            source: NOTE_SOURCE.into(),
            source_id: id.into(),
            kind: "note".into(),
            repo: repo.into(),
            session: "note".into(),
            ts: 1,
            title: "Title".into(),
            body: "Body.".into(),
        };
        raw.exclude_capture(("repo", "github.com/o/secret"), false)
            .unwrap();
        raw.exclude_capture(("folder", "/w/private"), false)
            .unwrap();
        let open_place = ("github.com/o/open", Some("/w/open"));
        let none = std::collections::HashMap::new();
        let refused = raw.append_note(
            &note("github.com/o/secret", "a"),
            (&[], &none),
            "/w/open",
            &settings,
            open_place,
        );
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        let below = ("github.com/o/open", Some("/w/private/sub"));
        let refused = raw.append_note(
            &note("github.com/o/open", "b"),
            (&[], &none),
            "/w/private/sub",
            &settings,
            below,
        );
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        // An imported document of the same repository is no note: the import is not fenced so.
        let imported = ImportDoc {
            source: "claude-mem:abc".into(),
            ..note("github.com/o/secret", "c")
        };
        assert_eq!(raw.append_imports(vec![imported]).unwrap(), 1);
        let kept = note("github.com/o/open", "d");
        let device = raw.device().to_owned();
        // An entry the op log no longer holds (a restore lost it) keeps nothing away.
        let stale = [(device.clone(), 9_999)];
        let Noted::Appended(seq) = raw
            .append_note(&kept, (&stale, &none), "/w/open", &settings, open_place)
            .unwrap()
        else {
            panic!("appended");
        };
        let held = [(device.clone(), seq)];
        let past = std::collections::HashMap::from([(device.clone(), seq)]);
        assert_eq!(
            raw.append_note(&kept, (&held, &past), "/w/open", &settings, open_place)
                .unwrap(),
            Noted::Kept
        );
        // Codex on #629: the same uid again before the index read the first, its text another
        // (a repeated `sourceFingerprint`), appends nothing; past what the index read, the log
        // alone is asked.
        let again = ImportDoc {
            body: "Another text.".into(),
            ..kept.clone()
        };
        assert_eq!(
            raw.append_note(&again, (&[], &none), "/w/open", &settings, open_place)
                .unwrap(),
            Noted::Kept
        );
        // #632: the first save indexed after the checkpoints were read and before the index rows
        // were: no row holds it, and the checkpoint read is from before it.
        let earlier = std::collections::HashMap::from([(device.clone(), seq - 1)]);
        assert_eq!(
            raw.append_note(&again, (&[], &earlier), "/w/open", &settings, open_place)
                .unwrap(),
            Noted::Kept
        );
        assert!(matches!(
            raw.append_note(&again, (&[], &past), "/w/open", &settings, open_place)
                .unwrap(),
            Noted::Appended(_)
        ));
        let forget = serde_json::json!({"uid": kept.uid});
        raw.append_ops(&[(OpKind::Forget, forget)]).unwrap();
        assert_eq!(
            raw.append_note(&kept, (&[], &none), "/w/open", &settings, open_place)
                .unwrap(),
            Noted::Forgotten
        );
    }

    /// Security review of #489: a work state write the MCP server makes asks in its transaction
    /// what the server's own check asks: the folder as read, and the labels against the list as
    /// given and as the rules now read it.
    #[test]
    fn a_work_state_write_asks_in_its_transaction_what_its_writer_asks() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let given = "github.com/o/acme-482915";
        raw.exclude_capture(("repo", given), false).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let repo = crate::capture::exclusion_label("repo", given, &settings);
        let fields = serde_json::json!({ "task": "Plan." });
        let fields = fields.as_object().unwrap();
        let read = (given, Some("/w/acme"));
        let refused = raw.work_state_under(&repo, "plan", fields, "/w/[x]", &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        // The list as the rules now read it, with the place read elsewhere.
        let elsewhere = ("github.com/o/open", Some("/w/open"));
        let refused = raw.work_state_under(&repo, "plan", fields, "/w/open", &settings, elsewhere);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        assert!(raw.work_state(&repo, "plan", fields, None).is_ok());
        // A folder whose separator a rule takes: only the place as read matches it.
        raw.exclude_capture(("folder", "/w/private"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"p\", regex = 'private(?:/|$)' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let cwd = crate::capture::exclusion_label("cwd", "/w/private/sub", &settings);
        let read = ("github.com/o/open", Some("/w/private/sub"));
        let refused =
            raw.work_state_under("github.com/o/open", "plan", fields, &cwd, &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
        // Another folder the rules read as an excluded one: only the list as read now matches it,
        // as the hook's check reads it.
        raw.exclude_capture(("folder", "/v/acme-111111"), false)
            .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let cwd = crate::capture::exclusion_label("cwd", "/v/acme-222222", &settings);
        let read = ("github.com/o/open", Some("/v/acme-222222"));
        let refused =
            raw.work_state_under("github.com/o/open", "plan", fields, &cwd, &settings, read);
        assert!(refused.unwrap_err().is::<NeverRecorded>());
    }

    /// Codex on #489: a repository never recorded under the label a rule now makes of it keeps its
    /// records from before the rule, which hold the label unmasked, from every provider too.
    #[test]
    fn records_from_before_a_rule_are_not_sent_once_their_repository_is_never_recorded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let secret = "github.com/o/acme-482915";
        raw.append(&Event {
            repo: Some(secret.into()),
            session: "old".into(),
            ..test_event("Old work.")
        })
        .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let now = crate::capture::exclusion_label("repo", secret, &settings);
        assert_ne!(now, secret);
        raw.exclude_capture(("repo", &now), false).unwrap();
        let reading = crate::curate::Reading::now(&raw, crate::curate::Reads::Live).unwrap();
        assert!(reading.excluded.contains("claude\0old"), "{reading:?}");
        assert_eq!(raw.exclusions().unwrap(), [now]);
    }

    /// Slice 4a: an event of a repository or folder never recorded is refused by every append that
    /// writes one, once the exclusion is committed: a hook's, a kept call's written back, an
    /// import's (whose batch records the rest); a folder covers what is in it, component by
    /// component, and an undo records what comes after it.
    #[test]
    fn an_event_never_recorded_is_refused_by_every_append() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let at = |repo: &str, cwd: &str| Event {
            repo: Some(repo.into()),
            cwd: Some(cwd.into()),
            ..test_event("canary-4a")
        };
        raw.exclude_capture(("repo", "github.com/o/secret"), false)
            .unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        for e in [
            at("github.com/o/secret", "/w/secret"),
            at("/x/private", "/x/private"),
            at("/x/private/sub", "/x/private/sub/dir"),
        ] {
            assert_eq!(raw.append_with_ledger(&e, &[], "").unwrap(), None);
        }
        assert!(
            raw.append_with_ledger(&at("/x/privateer", "/x/privateer"), &[], "")
                .unwrap()
                .is_some()
        );
        let kept = |e: Event| crate::capture::Captured {
            event: e,
            ledger: Vec::new(),
        };
        assert!(
            raw.append_kept(
                &[kept(at("github.com/o/secret", "/w/secret"))],
                "",
                None,
                &Default::default()
            )
            .unwrap()
        );
        let (mut secret, mut open) = (imported("canary-4a"), imported("canary-4a"));
        secret.event.repo = Some("github.com/o/secret".into());
        open.event.repo = Some("github.com/o/open".into());
        assert_eq!(
            raw.append_imported(&[secret, open], "", None)
                .unwrap()
                .len(),
            1
        );
        raw.exclude_capture(("repo", "github.com/o/secret"), true)
            .unwrap();
        assert!(
            raw.append_with_ledger(&at("github.com/o/secret", "/w/secret"), &[], "")
                .unwrap()
                .is_some()
        );
        let repos: Vec<Option<String>> = raw
            .after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                Item::Event(e) => Some(e.repo),
                _ => None,
            })
            .collect();
        assert_eq!(
            repos,
            [
                Some("/x/privateer".to_owned()),
                Some("github.com/o/open".to_owned()),
                Some("github.com/o/secret".to_owned())
            ]
        );
    }

    /// Slice 4a (Codex on #489): a work state write is refused inside its transaction once its
    /// repository, the repository of the claude-mem project an imported list is filed under, or
    /// the folder it is written from is never recorded; elsewhere it is kept, and a list written
    /// here is matched by its own key alone.
    #[test]
    fn a_work_state_write_is_refused_in_its_transaction_where_nothing_is_recorded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let task = serde_json::json!({ "task": "canary-4a" });
        let task = task.as_object().unwrap();
        raw.exclude_capture(("repo", "github.com/o/secret"), false)
            .unwrap();
        raw.exclude_capture(("folder", "/x/private"), false)
            .unwrap();
        let refused = |written: Result<Vec<i64>>| written.unwrap_err().is::<NeverRecorded>();
        let imported = |repo: &str| {
            let op = serde_json::json!({"repo": repo, "list": "plan", "fields": task,
                "clock": 1, "at": 1, "source": "claude-mem", "source_id": "w1"});
            vec![(OpKind::WorkState, op)]
        };
        let written = |raw: &mut Raw, repo: &str, cwd: Option<&str>| {
            raw.work_state(repo, "plan", task, cwd).map(|seq| vec![seq])
        };
        assert!(refused(written(&mut raw, "github.com/o/secret", None)));
        assert!(refused(written(
            &mut raw,
            "github.com/o/open",
            Some("/x/private/app")
        )));
        assert!(refused(raw.append_ops(&imported("claude-mem:secret/wt"))));
        assert!(written(&mut raw, "github.com/o/open", Some("/x/privateer")).is_ok());
        assert!(raw.append_ops(&imported("claude-mem:open")).is_ok());
        // A list written here by its own key alone, though that key has the shape of a claude-mem
        // project's (`ssh://claude-mem:secret/x`).
        assert!(written(&mut raw, "claude-mem:secret/x", None).is_ok());
        let lists = raw
            .ops_after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == OpKind::WorkState)
            .count();
        assert_eq!(lists, 3);
    }

    /// Slice 4a (Codex on #489): an imported list is refused inside its transaction also when its
    /// project's name, labelled under the import's settings, is what an exclusion committed during
    /// the import stored: a rule masks part of that name.
    #[test]
    fn an_imported_list_is_refused_by_an_exclusion_its_rules_mask() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = 'secret-[0-9]{4}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let mut raw = open(home.path()).unwrap();
        let stored = crate::capture::exclusion_label("repo", "github.com/o/secret-1234", &settings);
        assert!(!stored.contains("secret-1234"), "{stored}");
        raw.exclude_capture(("repo", &stored), false).unwrap();
        let op = serde_json::json!({"repo": "claude-mem:secret-1234", "list": "plan",
            "fields": {"task": "canary-4a"}, "clock": 1, "at": 1, "source": "claude-mem",
            "source_id": "w1"});
        let ops = [(OpKind::WorkState, op)];
        let refused = raw.append_imported_ops(&ops, &settings).unwrap_err();
        assert!(refused.is::<NeverRecorded>(), "{refused}");
    }

    /// docs/work-state.md L4: work state folds in its clock's order: a write on a copied store
    /// (another device) comes after every write it holds, even when the old device's clock ran
    /// ahead of the new one's.
    #[test]
    fn work_state_writes_fold_in_clock_order_across_devices() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let phase = |v: &str| {
            serde_json::json!({ "phase": v })
                .as_object()
                .unwrap()
                .clone()
        };
        raw.work_state("r", "release", &phase("one"), None).unwrap();
        raw.work_state("r", "release", &phase("two"), None).unwrap();
        raw.conn
            .execute(
                "UPDATE ops SET ts = ts + 3600000,
                   body = json_set(body, '$.clock', json_extract(body, '$.clock') + 3600000)",
                [],
            )
            .unwrap();
        drop(raw);
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(home.path().join("raw.db"), copy.path().join("raw.db")).unwrap();
        let mut other = open(copy.path()).unwrap();
        other
            .work_state("r", "release", &phase("three"), None)
            .unwrap();
        other
            .work_state("s", "release", &phase("elsewhere"), None)
            .unwrap();
        let phases: Vec<_> = other
            .work_state_entries("r")
            .unwrap()
            .into_iter()
            .map(|e| e.fields["phase"].clone())
            .collect();
        assert_eq!(phases, ["one", "two", "three"]);
    }

    fn import_doc(id: usize, body: String) -> ImportDoc {
        ImportDoc {
            uid: format!("claude-mem:abc:o{id}"),
            source: "claude-mem:abc".into(),
            source_id: format!("o{id}"),
            kind: "decision".into(),
            repo: "claude-mem:r".into(),
            session: "s".into(),
            ts: 1,
            title: "t".into(),
            body,
        }
    }

    /// D5: a body that would take its op over the cap is cut to fit, the marker saying how long
    /// it was, JSON escapes counted.
    #[test]
    fn an_op_over_the_cap_is_clipped_with_a_marker() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let body = "a \"quoted\" line\n".repeat(8_000);
        let chars = body.chars().count();
        assert_eq!(raw.append_imports(vec![import_doc(1, body)]).unwrap(), 1);
        let op = raw.ops_after(raw.device(), 0, 1).unwrap().remove(0);
        assert!(op.body.to_string().len() <= MAX_OP_BYTES);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        let marker = format!("\n…[clipped, {chars} chars in full]");
        assert!(
            kept.body.ends_with(&marker),
            "{}",
            &kept.body[kept.body.len() - 80..]
        );
        assert!(kept.body.starts_with("a \"quoted\" line\n"));
    }

    /// What a cut made to fit the cap leaves is scanned once more, as it is stored: a line-scoped
    /// allowlist judged the line whole, and the cut takes away what it allowed the value for
    /// (Codex's security review of the import's gate, 2026-11-12). With the rules the handle was
    /// told (`gates_with`): the bundled ones when it was told none, a home's kept value kept, and
    /// nothing masked while they are off.
    #[test]
    fn what_a_cut_to_the_op_cap_leaves_is_scanned_again() {
        use sha2::Digest;
        let value = format!("q9Zx8mL2vB4nR7tY1wK3{}", "pS6dJ0aF5hU2cE8g");
        // One line, which the bundled generic-api-key rule passes over for its ending.
        let line = format!(
            "auth = \"{value}\" {}--mount=type=secret,id=x",
            "lorem ".repeat(12_000)
        );
        let bundled = crate::redact::Rules::default();
        assert!(
            crate::redact::outbound_with(&line, &bundled).contains(&value),
            "the line whole is passed over"
        );
        let rules = |toml: &str| {
            crate::redact::Rules::new(&crate::config::parse_capture(Some(toml)).unwrap().redaction)
                .unwrap()
        };
        let keeps = rules(&format!(
            "[redaction]\nallowlist = [\"{:x}\"]\n",
            sha2::Sha256::digest(value.as_bytes())
        ));
        let off = rules("[redaction]\nenabled = false\n");
        for (told, masked) in [(None, true), (Some(&keeps), false), (Some(&off), false)] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            if let Some(rules) = told {
                raw.gates_with(rules);
            }
            raw.append_imports(vec![import_doc(1, line.clone())])
                .unwrap();
            let op = raw.ops_after(raw.device(), 0, 1).unwrap().remove(0);
            assert!(op.body.to_string().len() <= MAX_OP_BYTES);
            let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
            assert!(kept.body.contains("…[clipped, "), "cut to fit");
            assert_eq!(!kept.body.contains(&value), masked, "{}", &kept.body[..60]);
            assert_eq!(kept.body.starts_with("auth = \"[REDACTED]\""), masked);
        }
    }

    /// cubic on the Task 3 PR: a title as long as a body is cut too, and a document whose other
    /// fields alone are over the cap is an error, never an endless loop.
    #[test]
    fn a_title_over_the_cap_is_cut_and_other_fields_over_it_are_an_error() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let doc = ImportDoc {
            title: "t".repeat(100_000),
            ..import_doc(1, "b".repeat(100_000))
        };
        raw.append_imports(vec![doc]).unwrap();
        let op = raw.ops_after(raw.device(), 0, 1).unwrap().remove(0);
        assert!(op.body.to_string().len() <= MAX_OP_BYTES);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        assert!(kept.title.ends_with("…[clipped, 100000 chars in full]"));
        // A title that alone took the op over the cap leaves its body whole (Codex on #305).
        let doc = ImportDoc {
            title: "t".repeat(100_000),
            ..import_doc(3, "b".repeat(1_000))
        };
        raw.append_imports(vec![doc]).unwrap();
        let op = raw.ops_after(raw.device(), 1, 1).unwrap().remove(0);
        let kept: ImportDoc = serde_json::from_value(op.body).unwrap();
        assert_eq!(kept.body, "b".repeat(1_000));
        let doc = ImportDoc {
            session: "s".repeat(100_000),
            ..import_doc(2, "b".into())
        };
        assert!(raw.append_imports(vec![doc]).is_err());
    }

    /// D5: large documents go in more than one append, each within the byte cap, and none lost.
    #[test]
    fn a_batch_of_large_documents_splits_under_the_byte_cap() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let docs: Vec<ImportDoc> = (0..100)
            .map(|i| import_doc(i, "x".repeat(60_000)))
            .collect();
        assert_eq!(raw.append_imports(docs).unwrap(), 100);
        let ops = raw.ops_after(raw.device(), 0, 1_000).unwrap();
        let mut batches: std::collections::BTreeMap<i64, usize> = Default::default();
        for op in &ops {
            *batches.entry(op.batch).or_default() += op.body.to_string().len();
        }
        assert_eq!(ops.len(), 100);
        assert!(batches.len() > 1);
        assert!(batches.values().all(|&bytes| bytes <= MAX_BATCH_BYTES));
    }

    #[test]
    fn counted_documents_report_only_committed_batches_on_interruption() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let docs = (0..100)
            .map(|i| import_doc(i, "x".repeat(60_000)))
            .collect();
        crate::crash::at(2);
        let mut committed = Vec::new();
        let result = raw.append_imports_counted(docs, |n| committed.push(n));
        crate::crash::off();
        assert!(result.is_err());
        let durable = raw.ops_after(raw.device(), 0, 1_000).unwrap().len();
        assert!(durable > 0 && durable < 100);
        assert_eq!(committed, [durable as u64]);
    }

    /// OpenCodeReview on #305: an import op without a source id is passed over, not an error.
    #[test]
    fn import_keys_pass_over_an_op_without_a_text_source_id() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let ops = [
            serde_json::json!({"source": "claude-mem"}),
            serde_json::json!({"source": "claude-mem", "source_id": 7}),
            serde_json::json!({"source": "claude-mem", "source_id": "o1"}),
        ];
        raw.append_ops(&ops.map(|op| (OpKind::Import, op))).unwrap();
        let keys = raw.import_keys("claude-mem").unwrap();
        assert_eq!(keys, ["o1".to_owned()].into_iter().collect());
    }

    fn imported(body: &str) -> crate::capture::Captured {
        crate::capture::Captured {
            event: Event {
                source: "oboete-v1".into(),
                ..test_event(body)
            },
            ledger: Vec::new(),
        }
    }

    fn v1_checkpoint(through: i64) -> Checkpoint {
        Checkpoint {
            key: "oboete-v1:d1".into(),
            through,
            row: Some(V1Row {
                id: through,
                ts: 7,
                session_id: "s1".into(),
            }),
            prefix: None,
        }
    }

    #[test]
    fn batch_lock_failures_restore_the_timeout_without_committing() {
        for records in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            let append = |raw: &mut Raw| {
                if records {
                    raw.append_imported(&[imported("kept")], "v", Some(&v1_checkpoint(1)))
                } else {
                    raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "kept"}))])
                }
            };
            let timeout = |raw: &Raw| {
                raw.conn
                    .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0))
                    .unwrap()
            };
            assert_eq!(timeout(&raw), 2_000);
            // An op's batch gives up at the connection's timeout; an import waits longer
            // (`an_imported_batch_waits_past_the_busy_timeout_for_a_writer`).
            if !records {
                raw.conn.busy_timeout(Duration::from_millis(100)).unwrap();
                let writer = Connection::open(home.path().join("raw.db")).unwrap();
                writer.execute_batch("BEGIN IMMEDIATE").unwrap();
                let started = Instant::now();
                let error = append(&mut raw).unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<rusqlite::Error>()
                        .unwrap()
                        .sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                assert!(started.elapsed() >= Duration::from_millis(100));
                assert!(started.elapsed() < Duration::from_secs(1));
                assert_eq!(timeout(&raw), 100);
                assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
                writer.execute_batch("ROLLBACK").unwrap();
            }

            // A non-lock error returns promptly and restores the original timeout too.
            raw.conn.busy_timeout(Duration::from_secs(2)).unwrap();
            raw.conn.execute_batch("PRAGMA query_only=ON").unwrap();
            let started = Instant::now();
            let error = append(&mut raw).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<rusqlite::Error>()
                    .unwrap()
                    .sqlite_error_code(),
                Some(rusqlite::ErrorCode::ReadOnly)
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(timeout(&raw), 2_000);
            raw.conn.execute_batch("PRAGMA query_only=OFF").unwrap();

            // A failed commit is attempted once; neither the rows nor their checkpoint lands.
            crate::crash::at(1);
            let failed = append(&mut raw);
            let commits = crate::crash::count();
            crate::crash::off();
            assert!(failed.is_err());
            assert_eq!(commits, 1);
            assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
            assert!(raw.migration_checkpoints("oboete-v1:").unwrap().is_empty());
            assert_eq!(append(&mut raw).unwrap(), [1]);
            assert_eq!(timeout(&raw), 2_000);
            assert_eq!(raw.max_op_seq().unwrap(), 1);
            if records {
                assert_eq!(raw.max_seq().unwrap(), 1);
                assert_eq!(
                    raw.migration_checkpoints("oboete-v1:").unwrap()["oboete-v1:d1"],
                    v1_checkpoint(1)
                );
            }
        }
    }

    /// D6: a batch of imported records, their ledger rows and the checkpoint op commit together;
    /// the op says through which seq the batch went, and each key's furthest checkpoint reads
    /// back by its prefix.
    /// #362: an import is no hook, so its batch waits for another writer as a non-hook open does
    /// (`db::OPEN_WRITE_WAIT`), not for the connection's short busy timeout: a pass beside a busy
    /// agent stopped with "database is locked".
    #[test]
    fn an_imported_batch_waits_past_the_busy_timeout_for_a_writer() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.conn.busy_timeout(Duration::from_millis(50)).unwrap();
        let writer = open(home.path()).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let seqs = std::thread::scope(|s| {
            let import = s.spawn(|| raw.append_imported(&[imported("a")], "v9", None));
            std::thread::sleep(Duration::from_millis(400));
            assert!(!import.is_finished(), "it gave up at the busy timeout");
            writer.conn.execute_batch("ROLLBACK").unwrap();
            import.join().unwrap().unwrap()
        });
        assert_eq!(seqs, [1]);
        // Its own timeout is back for what follows.
        let timeout: u32 = raw
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 50);
    }

    /// docs/file-note.md F11 (test 24): a files op of a uid forgotten after the import's check is
    /// refused with its whole batch, and the import's check keeps it out of the next one.
    #[test]
    fn a_forgotten_uids_files_op_is_refused_with_its_batch() {
        let mut s = crate::search::b::fixture::Store::new();
        let a = s.files_of("o1", ("p", "cm"), &["src/a.rs"], &[]);
        let b = s.files_of("o2", ("p", "cm"), &["src/b.rs"], &[]);
        s.raw
            .append_ops(&[(
                OpKind::Forget,
                serde_json::json!({"uid": a.uid, "job": "j"}),
            )])
            .unwrap();
        let device = s.raw.device().to_owned();
        let count = |raw: &Raw| raw.ops_after(&device, 0, 100).unwrap().len();
        let before = count(&s.raw);
        let batch =
            [a.clone(), b.clone()].map(|f| (OpKind::Import, serde_json::to_value(f).unwrap()));
        assert!(s.raw.append_ops(&batch).is_err());
        assert_eq!(count(&s.raw), before);
        let items = vec![Import::Files(a), Import::Files(b.clone())];
        assert_eq!(s.raw.append_import_items(items, |_| {}).unwrap(), 1);
        let last = s.raw.ops_after(&device, 0, 100).unwrap().pop().unwrap();
        assert_eq!(last.body, serde_json::to_value(b).unwrap());
    }

    #[test]
    fn an_imported_batch_lands_with_its_ledger_and_checkpoint() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        raw.append(&test_event("live")).unwrap();
        let mut masked = imported("b");
        let finding = crate::redact::Finding {
            rule: "r".into(),
            offset: 1,
            length: 2,
        };
        masked.ledger.push(("/prompt".into(), finding));
        let seqs = raw
            .append_imported(&[imported("a"), masked], "v9", Some(&v1_checkpoint(42)))
            .unwrap();
        assert_eq!(seqs, [2, 3]);
        let sources: Vec<String> = raw
            .after(raw.device(), 1, 10)
            .unwrap()
            .into_iter()
            .map(|r| match r.item {
                Item::Event(e) => e.source,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(sources, ["oboete-v1", "oboete-v1"]);
        let ledger: (i64, String) = raw
            .conn
            .query_row("SELECT seq, ruleset FROM ledger", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(ledger, (3, "v9".to_owned()));
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Migration);
        assert_eq!(
            ops[0].body,
            serde_json::json!({"key": "oboete-v1:d1", "through": 42, "from_seq": 2, "to_seq": 3,
                "row": {"id": 42, "ts": 7, "session_id": "s1"}})
        );
        // A transcript's checkpoint has no row; one with nothing to record holds the last seq.
        let t = Checkpoint {
            key: "transcript:claude:s1".into(),
            through: 9,
            row: None,
            prefix: None,
        };
        assert!(raw.append_imported(&[], "v9", Some(&t)).unwrap().is_empty());
        raw.append_imported(&[imported("c")], "v9", Some(&v1_checkpoint(50)))
            .unwrap();
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(
            ops[1].body,
            // No record: an empty range, which no restore can lose.
            serde_json::json!({"key": "transcript:claude:s1", "through": 9, "from_seq": 4,
                "to_seq": 3})
        );
        assert_eq!(
            raw.migration_checkpoints("oboete-v1:").unwrap(),
            [("oboete-v1:d1".to_owned(), v1_checkpoint(50))]
                .into_iter()
                .collect()
        );
        assert_eq!(
            raw.migration_checkpoints("transcript:").unwrap(),
            [(t.key.clone(), t)].into_iter().collect()
        );
    }

    /// D6: an append of imported records is one transaction, bounded as an op batch is, and it
    /// never takes a record of a live source, which curation and the manifest read.
    #[test]
    fn an_imported_batch_over_a_cap_or_with_a_live_record_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let many: Vec<_> = (0..=IMPORT_BATCH).map(|_| imported("x")).collect();
        assert!(raw.append_imported(&many, "v", None).is_err());
        let big: Vec<_> = (0..5).map(|_| imported(&"x".repeat(1 << 20))).collect();
        assert!(raw.append_imported(&big, "v", None).is_err());
        for source in LIVE {
            let mut live = imported("x");
            live.event.source = source.into();
            let batch = [imported("y"), live];
            assert!(
                raw.append_imported(&batch, "v", Some(&v1_checkpoint(1)))
                    .is_err()
            );
        }
        assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), (0, 0));
        // At the caps it lands.
        assert_eq!(raw.append_imported(&big[1..], "v", None).unwrap().len(), 4);
        let seqs = raw.append_imported(&many[1..], "v", None).unwrap();
        assert_eq!(seqs.len(), IMPORT_BATCH);
    }

    /// spec 8.4, A104: both import paths ask `denied` of each item and leave a denied one out. A
    /// batch left with nothing still moves its checkpoint, so the next pass does not read it again.
    #[test]
    fn every_append_path_calls_denied() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let forgotten = format!("x {DENIED_IN_TESTS} y");
        let batch = [imported(&forgotten), imported("kept")];
        let seqs = raw
            .append_imported(&batch, "v", Some(&v1_checkpoint(2)))
            .unwrap();
        assert_eq!(seqs, [1]);
        let batch = [imported(&forgotten)];
        let seqs = raw
            .append_imported(&batch, "v", Some(&v1_checkpoint(3)))
            .unwrap();
        assert!(seqs.is_empty());
        let kept = raw.after(raw.device(), 0, 10).unwrap();
        assert!(
            matches!(&kept[..], [r] if r.item == Item::Event(Box::new(imported("kept").event)))
        );
        let checkpoints = raw.migration_checkpoints("oboete-v1:").unwrap();
        assert_eq!(checkpoints["oboete-v1:d1"].through, 3);
        let docs = vec![
            import_doc(1, forgotten.clone()),
            ImportDoc {
                title: forgotten,
                ..import_doc(2, "b".into())
            },
            import_doc(3, "kept".into()),
        ];
        assert_eq!(raw.append_imports(docs).unwrap(), 1);
        let keys = raw.import_keys("claude-mem:abc").unwrap();
        assert_eq!(keys, ["o3".to_owned()].into_iter().collect());
    }

    /// D6, the transcript cut: each session's earliest record that is neither the transcript's
    /// own nor a v1 repository label.
    #[test]
    fn earliest_by_session_leaves_out_transcript_and_touch_records() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let at = |agent: &str, session: &str, ts: i64, source: &str, kind: &str| Event {
            agent: agent.into(),
            session: session.into(),
            ts,
            source: source.into(),
            kind: kind.into(),
            ..test_event("x")
        };
        for e in [
            at("claude", "s1", 50, "hook", "prompt"),
            at("claude", "s1", 30, "oboete-v1", "prompt"),
            at("claude", "s1", 10, "transcript", "prompt"),
            at("claude", "s1", 5, "oboete-v1", "touch"),
            at("codex", "s1", 70, "replay", "tool"),
            at("codex", "s2", 3, "transcript", "prompt"),
        ] {
            raw.append(&e).unwrap();
        }
        let key = |a: &str, s: &str| (a.to_owned(), s.to_owned());
        assert_eq!(
            raw.earliest_by_session().unwrap(),
            [(key("claude", "s1"), 30), (key("codex", "s1"), 70)]
                .into_iter()
                .collect()
        );
    }

    /// D6: v1's repository labels are records, never counted as imported events.
    #[test]
    fn imported_counts_leave_touch_records_out() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let mut touch = imported("{}");
        touch.event.kind = "touch".into();
        raw.append_imported(&[imported("a"), touch], "v", None)
            .unwrap();
        let dev = raw.device().to_owned();
        assert_eq!(
            raw.imported_counts(&dev, &[(1, 10)]).unwrap(),
            [("oboete-v1".to_owned(), 1)].into_iter().collect()
        );
    }

    /// Owner decision 43 (Codex on #502): a page of op text read under rules changed since (the
    /// switch turned off, say) is not masked: under the dispatch hold a save takes too, the rules
    /// are checked again, and nothing more is written.
    #[test]
    fn a_page_masked_after_the_rules_changed_is_left_as_it_was() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(&config, rule).unwrap();
        let mut raw = open(home.path()).unwrap();
        let body = serde_json::json!({"text": "deploy acme-482915"});
        raw.append_ops(&[(OpKind::Claim, body.clone())]).unwrap();
        let rules = crate::redact::Rules::load(home.path()).unwrap();
        let page = raw.op_text_masks(&rules, None, 10).unwrap();
        assert_eq!(page.len(), 1);
        let off = rule.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        std::fs::write(&config, off).unwrap();
        assert_eq!(raw.mask_op_text(&page, &rules).unwrap(), None);
        let ops = raw.ops_after(raw.device(), 0, 10).unwrap();
        assert_eq!(ops[0].body, body);
    }

    /// Owner decision 43 (Codex's security review of the switch): a scan whose mark an append
    /// under rules turned off took away records nothing, also when a scan begun after has put a
    /// mark of its own there, which that later scan alone records by.
    #[test]
    fn a_scan_records_only_under_its_own_mark() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let off = crate::redact::Rules::new(
            &crate::config::parse_capture(Some("[redaction]\nenabled = false\n"))
                .unwrap()
                .redaction,
        )
        .unwrap();
        let first = raw.op_text_scan_begun().unwrap();
        raw.gates_with(&off);
        raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "deploy notes"}))])
            .unwrap();
        let second = raw.op_text_scan_begun().unwrap();
        assert!(!raw.ops_text_scanned("v", &first).unwrap());
        assert!(raw.ops_text_owed("v").unwrap());
        assert!(raw.ops_text_scanned("v", &second).unwrap());
        assert!(!raw.ops_text_owed("v").unwrap());
    }

    /// Owner decision 43 (Codex's security review of the switch): a handle whose ops' text was
    /// gated with the rules off owes the op text scan in each append's own transaction, the
    /// fenced one of curation too; a handle gated with rules on owes nothing.
    #[test]
    fn an_append_gated_off_owes_the_scan_in_its_transaction() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let rules = |toml: &str| {
            crate::redact::Rules::new(&crate::config::parse_capture(Some(toml)).unwrap().redaction)
                .unwrap()
        };
        let claim = || [(OpKind::Claim, serde_json::json!({"text": "deploy notes"}))];
        let scanned = |raw: &Raw| {
            let mark = raw.op_text_scan_begun().unwrap();
            assert!(raw.ops_text_scanned("v", &mark).unwrap());
        };
        scanned(&raw);
        raw.gates_with(&rules("[redaction]\nenabled = true\n"));
        raw.append_ops(&claim()).unwrap();
        assert!(!raw.ops_text_owed("v").unwrap());
        raw.gates_with(&rules("[redaction]\nenabled = false\n"));
        raw.append_ops(&claim()).unwrap();
        assert!(raw.ops_text_owed("v").unwrap());
        scanned(&raw);
        let fence = raw.fence_count().unwrap();
        raw.append_ops_fenced(&claim(), fence).unwrap();
        assert!(raw.ops_text_owed("v").unwrap());
    }

    /// Codex's security review of #489: a repository label masked under a rule removed since
    /// cannot be read again, so its records are held back from sending once a repository it may
    /// be is never recorded; one of another owner leaves them be.
    #[test]
    fn a_label_masked_under_a_rule_removed_since_is_held_back_when_it_may_be_excluded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let masked = format!("github.com/o/{}", crate::redact::MASK);
        let seq = raw
            .append(&Event {
                repo: Some(masked.clone()),
                ..test_event("deploy notes")
            })
            .unwrap();
        raw.exclude_capture(("repo", "github.com/x/other"), false)
            .unwrap();
        assert!(!raw.exclusions_now().unwrap().contains(&masked));
        raw.exclude_capture(("repo", "github.com/o/acme-482915"), false)
            .unwrap();
        let list = raw.exclusions_now().unwrap();
        assert!(list.contains(&masked), "{list:?}");
        let device = raw.device().to_owned();
        let key = raw.session_key(&device, seq).unwrap().unwrap();
        assert!(raw.sessions_in(&list).unwrap().contains(&key));
    }

    /// OpenCodeReview on #304: an event's agent and session labels may be NULL; `sessions_in`
    /// keys such an event as `session_key` does, never an error that stops the curation phase.
    #[test]
    fn sessions_in_keys_an_event_without_labels_as_session_key_does() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let e = Event {
            repo: Some("x".into()),
            ..test_event("hi")
        };
        let seq = raw.append(&e).unwrap();
        raw.conn
            .execute(
                "UPDATE records SET agent = NULL, session = NULL WHERE seq = ?1",
                [seq],
            )
            .unwrap();
        let device = raw.device().to_owned();
        let key = raw.session_key(&device, seq).unwrap().unwrap();
        let keys = raw.sessions_in(&["x".to_owned()]).unwrap();
        assert_eq!(keys, [key].into_iter().collect());
    }

    #[test]
    fn ops_commit_together_and_read_back_in_order() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        let window = serde_json::json!({"from_seq": 1, "to_seq": 4, "outcome": "curated"});
        let claim = serde_json::json!({"text": "all timestamps on disk stay UTC"});
        let seqs = raw
            .append_ops(&[
                (OpKind::Window, window.clone()),
                (OpKind::Claim, claim.clone()),
            ])
            .unwrap();
        assert_eq!(seqs, [1, 2]);
        let ops = raw.ops_after(&dev, 0, 10).unwrap();
        assert_eq!(
            ops.iter()
                .map(|o| (o.op_seq, o.kind, o.body.clone()))
                .collect::<Vec<_>>(),
            [
                (1, OpKind::Window, window),
                (2, OpKind::Claim, claim.clone())
            ]
        );
        // An op over the cap fails the whole append: the op before it is not written either.
        let huge = serde_json::json!({"text": "x".repeat(MAX_OP_BYTES)});
        assert!(
            raw.append_ops(&[(OpKind::Claim, claim), (OpKind::Claim, huge)])
                .is_err()
        );
        // So does an append whose ops fit one by one but not together.
        let big = serde_json::json!({"text": "x".repeat(MAX_OP_BYTES - 100)});
        let many = vec![(OpKind::Claim, big); MAX_BATCH_BYTES / MAX_OP_BYTES + 1];
        assert!(raw.append_ops(&many).is_err());
        // And one of many small ops, whose lines' own fields would add up.
        let many = vec![(OpKind::Claim, serde_json::json!(0)); MAX_BATCH_OPS + 1];
        assert!(raw.append_ops(&many).is_err());
        assert_eq!(raw.max_op_seq().unwrap(), 2);
        assert_eq!(raw.ops_after(&dev, 1, 10).unwrap().len(), 1);
    }

    #[test]
    fn an_ops_segment_ends_only_between_appends() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let claim = |i: i32| {
            (
                OpKind::Claim,
                serde_json::json!({"text": format!("claim {i}")}),
            )
        };
        let window = (OpKind::Window, serde_json::json!({"to_seq": 1}));
        raw.append_ops(&[window, claim(1), claim(2)]).unwrap();
        raw.append_ops(&[claim(3)]).unwrap();
        // The cap is reached after the first line; the window's claims still come with it.
        let seqs = |lines: Vec<(i64, String)>| lines.into_iter().map(|l| l.0).collect::<Vec<_>>();
        assert_eq!(seqs(raw.export_op_lines(0, 1).unwrap()), [1, 2, 3]);
        assert_eq!(seqs(raw.export_op_lines(3, 1).unwrap()), [4]);
    }

    #[test]
    fn the_curation_checkpoint_is_the_last_window_that_is_not_a_recuration() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        let dev = raw.device().to_owned();
        assert_eq!(raw.curation_checkpoint(&dev).unwrap(), (0, None));
        let mut window = |body: serde_json::Value| {
            raw.append_ops(&[(OpKind::Window, body)]).unwrap();
            raw.curation_checkpoint(&dev).unwrap()
        };
        assert_eq!(
            window(serde_json::json!({"to_seq": 5, "to_offset": null})),
            (5, None)
        );
        // A window that ends inside an event: the next one starts at that offset.
        assert_eq!(
            window(serde_json::json!({"to_seq": 7, "to_offset": 120})),
            (7, Some(120))
        );
        // `oboete recurate` of an older span moves it neither back nor forward.
        let back = serde_json::json!({"to_seq": 2, "to_offset": null, "recurate": true});
        assert_eq!(window(back), (7, Some(120)));
        raw.append_ops(&[(OpKind::Claim, serde_json::json!({"text": "c"}))])
            .unwrap();
        assert_eq!(raw.curation_checkpoint(&dev).unwrap(), (7, Some(120)));
    }

    /// A session's latest event before a window is sought by `SESSION_INDEX` once the worker has
    /// built it, and found the same either way; `open`, as a hook opens raw, builds none.
    #[test]
    fn a_sessions_latest_event_is_sought_by_its_index_once_built() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for (session, body) in [("a", "one"), ("b", "two"), ("a", "three"), ("b", "four")] {
            let e = Event {
                session: session.into(),
                ..test_event(body)
            };
            raw.append(&e).unwrap();
        }
        let plan = |raw: &Raw| -> String {
            raw.conn
                .prepare(&format!("EXPLAIN QUERY PLAN {SESSION_LATEST}"))
                .unwrap()
                .query_map(params![raw.device, 5, "claude", "a"], |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(" ")
        };
        let latest = |raw: &Raw| -> Vec<i64> {
            raw.conn
                .prepare(SESSION_LATEST)
                .unwrap()
                .query_map(params![raw.device, 5, "claude", "a"], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert!(!raw.has_session_index().unwrap());
        assert!(
            !plan(&raw).contains("records_session_seq"),
            "{}",
            plan(&raw)
        );
        assert_eq!(latest(&raw), vec![3, 1]);
        raw.ensure_session_index().unwrap();
        assert!(raw.has_session_index().unwrap());
        assert!(plan(&raw).contains("records_session_seq"), "{}", plan(&raw));
        assert_eq!(latest(&raw), vec![3, 1]);
        drop(raw);
        assert!(open(home.path()).unwrap().has_session_index().unwrap());
    }

    /// #899 W2: the events' repositories read through `records_repo` are those read from every
    /// record: each once, none for a record of no repository or an empty one, a rule's record's
    /// too. Once the index is there it is what is read, by seeks, and no record.
    #[test]
    fn the_events_repositories_are_read_from_their_index_once_it_is_built() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for (repo, kind) in [
            (Some("b/two"), "prompt"),
            (Some("a/one"), "prompt"),
            (Some("a/one"), "tool"),
            (Some(""), "prompt"),
            (None, "prompt"),
            (Some("c/rule"), "directive"),
        ] {
            let e = Event {
                kind: kind.into(),
                repo: repo.map(str::to_owned),
                ..test_event("words")
            };
            raw.append(&e).unwrap();
        }
        let names = |raw: &Raw| {
            let mut names = event_repos(&raw.conn, "main").unwrap();
            names.sort();
            names
        };
        let plan = |raw: &Raw, indexed: bool| -> String {
            raw.conn
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {}",
                    event_repos_sql("main", indexed)
                ))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(" | ")
        };
        assert!(!raw.has_repo_index().unwrap());
        assert_eq!(names(&raw), ["a/one", "b/two", "c/rule"]);
        assert!(
            plan(&raw, false).contains("SCAN main.records"),
            "{}",
            plan(&raw, false)
        );
        raw.repo_index().unwrap();
        assert!(raw.has_repo_index().unwrap());
        assert_eq!(names(&raw), ["a/one", "b/two", "c/rule"]);
        let seeks = plan(&raw, true);
        assert!(
            seeks.contains("SEARCH main.records USING COVERING INDEX records_repo"),
            "{seeks}"
        );
        assert!(!seeks.contains("SCAN main.records"), "{seeks}");
        // A name that sorts after every other, and one a byte above another's prefix.
        for repo in ["zz/last", "a/one-more", "a/on"] {
            let e = Event {
                repo: Some(repo.into()),
                ..test_event("more words")
            };
            raw.append(&e).unwrap();
        }
        assert_eq!(
            names(&raw),
            ["a/on", "a/one", "a/one-more", "b/two", "c/rule", "zz/last"]
        );
        drop(raw);
        assert!(open(home.path()).unwrap().has_repo_index().unwrap());
    }

    /// Forget's `records_sessions` and the seek's `records_session_seq` are two indexes, each as
    /// its own builder makes it, whichever is built first; the seek takes its own.
    #[test]
    fn the_seeks_index_and_forgets_are_two_whichever_comes_first() {
        let indexes = |raw: &Raw| -> Vec<(String, String)> {
            raw.conn
                .prepare(
                    "SELECT name, sql FROM sqlite_master WHERE type = 'index'
                       AND name IN ('records_sessions', 'records_session_seq') ORDER BY name",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let mut seen = Vec::new();
        for forget_first in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = open(home.path()).unwrap();
            raw.append(&test_event("one")).unwrap();
            if forget_first {
                raw.session_indexes().unwrap();
                assert!(!raw.has_session_index().unwrap());
            }
            raw.ensure_session_index().unwrap();
            raw.session_indexes().unwrap();
            let both = indexes(&raw);
            assert_eq!(both.len(), 2, "{both:?}");
            assert!(
                both[0].1.contains("device, agent, session, seq"),
                "{both:?}"
            );
            assert!(both[1].1.contains("agent, session, source"), "{both:?}");
            let plan: String = raw
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {SESSION_LATEST}"))
                .unwrap()
                .query_map(params![raw.device, 2, "claude", "s"], |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(" ");
            assert!(plan.contains("records_session_seq"), "{plan}");
            seen.push(both);
        }
        assert_eq!(seen[0], seen[1]);
    }

    /// A session's previous window (#240) is the one the worker cut before the window at `from`,
    /// up to `from` when it runs past it: not a recuration of another part of a split event, nor
    /// the window at `from` curated before. Its claims come with those of every later window op
    /// quoted inside it, the newest window first.
    #[test]
    fn the_previous_window_is_the_workers_with_the_recurations_of_its_parts() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = open(home.path()).unwrap();
        for i in 1..=4 {
            raw.append(&test_event(&format!("line {i}"))).unwrap();
        }
        // A window op and its claims, each quoted at (seq, offset) for 5 bytes.
        let window = |raw: &mut Raw,
                      span: [Option<i64>; 4],
                      recurate: bool,
                      claims: &[(&str, i64, i64)]| {
            let op = serde_json::json!({"from_seq": span[0], "from_offset": span[1],
                "to_seq": span[2], "to_offset": span[3], "recurate": recurate});
            let mut ops = vec![(OpKind::Window, op)];
            for (text, seq, offset) in claims {
                let claim = serde_json::json!({"text": text, "evidence": [{"device": raw.device(),
                    "seq": seq, "offset": offset, "length": 5}]});
                ops.push((OpKind::Claim, claim));
            }
            raw.append_ops(&ops).unwrap();
        };
        let previous = |raw: &Raw, from: (i64, Option<i64>)| -> Vec<String> {
            let ops = raw
                .previous_window_ops("claude", "s", from, |_| true)
                .unwrap();
            ops.into_iter()
                .map(|o| o.body["text"].as_str().unwrap().to_owned())
                .collect()
        };
        // Event 2 is split at offset 50; its first part is curated again after the worker went on.
        window(
            &mut raw,
            [Some(1), None, Some(2), Some(50)],
            false,
            &[("w1", 1, 0)],
        );
        window(
            &mut raw,
            [Some(2), Some(50), Some(2), None],
            false,
            &[("w2", 2, 60)],
        );
        window(
            &mut raw,
            [Some(3), None, Some(4), None],
            false,
            &[("w3", 3, 0)],
        );
        // A quote a split cuts in two is in neither part, as `curate::anchored_in` reads it.
        window(
            &mut raw,
            [Some(1), None, Some(2), Some(50)],
            true,
            &[("r1", 1, 0), ("r1 cut", 2, 48)],
        );
        assert_eq!(previous(&raw, (3, None)), ["w2"]);
        // The window at event 2's offset 50 curated again: its previous window is the first part.
        assert_eq!(previous(&raw, (2, Some(50))), ["r1", "w1"]);
        // A part of the worker's window curated again: the rest of the window is still its own.
        window(
            &mut raw,
            [Some(4), None, Some(4), None],
            true,
            &[("r4", 4, 0)],
        );
        assert_eq!(previous(&raw, (5, None)), ["r4", "w3"]);
        // Event 3 curated again on its own; then event 4 alone, whose previous window is the
        // worker's 3-4 up to event 4.
        window(
            &mut raw,
            [Some(3), None, Some(3), None],
            true,
            &[("r3", 3, 0)],
        );
        assert_eq!(previous(&raw, (4, None)), ["r3", "w3"]);
        // A recuration of a part that does not hold the session's latest event is the window's too.
        assert_eq!(previous(&raw, (5, None)), ["r3", "r4", "w3"]);
        // Recurations cut another way, over the window's start or into the window at 4: only
        // what they quoted inside the previous window.
        let across = [("r23 before", 2, 60), ("r23", 3, 5), ("r23 after", 4, 0)];
        window(&mut raw, [Some(2), Some(50), Some(4), None], true, &across);
        assert_eq!(previous(&raw, (4, None)), ["r23", "r3", "w3"]);
        assert_eq!(
            previous(&raw, (5, None)),
            ["r23", "r23 after", "r3", "r4", "w3"]
        );
        // A quote of another device's record is in no window of this device, whatever its seq.
        let op = serde_json::json!({"from_seq": 3, "from_offset": null, "to_seq": 4,
            "to_offset": null, "recurate": true});
        let quote = |device: &str, text: &str| {
            let e = serde_json::json!({"device": device, "seq": 3, "offset": 10, "length": 5});
            (
                OpKind::Claim,
                serde_json::json!({"text": text, "evidence": [e]}),
            )
        };
        let (here, other) = (quote(raw.device(), "r34"), quote("other", "elsewhere"));
        raw.append_ops(&[(OpKind::Window, op), other, here])
            .unwrap();
        assert_eq!(
            previous(&raw, (5, None)),
            ["r34", "r23", "r23 after", "r3", "r4", "w3"]
        );
    }

    /// Slice 3d-4: a window that showed a masked record and a forgotten one is the forget's, which
    /// takes the claims it wrote too, whichever comes first in its span.
    #[test]
    fn a_window_that_showed_a_masked_and_a_forgotten_record_is_the_forgets() {
        let forgotten = Forgotten::of([("d".to_owned(), 2)], std::iter::empty())
            .with_masks(vec![("d".to_owned(), 1, Some(100))]);
        let judge = Judge {
            forgotten,
            ..Judge::default()
        };
        let body = serde_json::json!({"outcome": "curated", "from_seq": 1, "to_seq": 2});
        let p = crate::provenance::of(OpKind::Window, &body);
        let job = judge.job(OpKind::Window, "d", 50, &p, &Taken::new());
        assert_eq!(job.as_deref(), Some(""));
    }

    /// Decision 107 (docs/curator-conversation.md): a window op that lists `read`, the spans of
    /// the stretches its curator was sent again before its own, showed their records as it showed
    /// its own (rule a). A forgotten one takes its text; a masked one too when the op came before
    /// the mask; the forget's comes first; and the forgotten records it showed (3d-4) are those of
    /// what it read too. A record outside its span and what it read, a mask the op was written
    /// well after, an op that lists no `read` and a skipped one take nothing.
    #[test]
    fn a_window_showed_the_records_of_the_stretches_it_read() {
        use serde_json::json;
        let forgotten = Forgotten::of([("d".to_owned(), 2)], std::iter::empty())
            .with_masks(vec![("d".to_owned(), 4, Some(100))]);
        let judge = Judge {
            forgotten,
            ..Judge::default()
        };
        let job = |body: &serde_json::Value, ts: i64| {
            let p = crate::provenance::of(OpKind::Window, body);
            judge.job(OpKind::Window, "d", ts, &p, &Taken::new())
        };
        let window = |read: serde_json::Value| json!({"outcome": "curated", "from_seq": 6, "to_seq": 7, "read": read});
        assert_eq!(job(&window(json!([[1, 3]])), 50).as_deref(), Some(""));
        assert_eq!(job(&window(json!([[4, 5]])), 50).as_deref(), Some(MASK_JOB));
        assert_eq!(
            job(&window(json!([[4, 5], [1, 3]])), 50).as_deref(),
            Some("")
        );
        let late = 100 + SHOWN_MARGIN_MS + 1;
        assert_eq!(job(&window(json!([[4, 5]])), late), None);
        assert_eq!(job(&window(json!([[5, 5]])), 50), None);
        assert_eq!(job(&window(json!([])), 50), None);
        let none = json!({"outcome": "curated", "from_seq": 6, "to_seq": 7});
        assert_eq!(job(&none, 50), None);
        let skipped = json!({"outcome": "skipped", "from_seq": 6, "to_seq": 7, "read": [[1, 3]]});
        assert_eq!(job(&skipped, 50), None);
        let body = window(json!([[4, 5], [1, 3]]));
        let p = crate::provenance::of(OpKind::Window, &body);
        assert_eq!(judge.shown_records("d", 50, &p), [2]);
    }

    /// Decision 107: a window op whose text a purge takes keeps `read`, spans and no text, as it
    /// keeps its own span and its goals: a restore's cut still reads the records it names.
    #[test]
    fn a_window_whose_text_is_taken_keeps_what_it_read() {
        use serde_json::json;
        let body = json!({"outcome": "curated", "from_seq": 6, "to_seq": 7, "read": [[1, 3]],
            "summary": "s", "observations": [{"title": "t"}], "provider": "p"});
        assert_eq!(
            forgotten_window_body(&body, "j"),
            json!({"outcome": "curated", "from_seq": 6, "to_seq": 7, "read": [[1, 3]],
                "forgotten": "j"})
        );
    }
